# Enable global rate limiting

> Audience: platform-engineers, operators · Status: stable

This recipe turns on **global** (cross-Envoy) rate limiting end to end using Flowplane's
first-party rate-limit service, `flowplane-rls`. By the end, a route descriptor is capped at N
requests per window and the (N+1)-th request gets `429` — enforced centrally, no external limiter
and no hand-written RLS cluster.

If you only need a per-Envoy cap on one route, you want the **local** rate limit instead — see
[Add JWT auth and a local rate limit](jwt-auth-rate-limit-route.md). For *why* the global path is
shaped this way (separate process, namespaced counters, fail modes), read
[Global rate limiting](../concepts/global-rate-limiting.md).

**Prerequisites**

- A running control plane and a real Envoy joined over xDS, with a listener, route-config, and cluster that already route traffic. The [getting-started tutorial](../tutorials/getting-started.md) gets you here with the `local` resource set on listener port `10001`; the examples below use sample names such as `edge`, `api-routes`, and `httpbin`, so substitute your actual resource names.
- The CLI authenticated against your control plane (`flowplane auth …` or `FLOWPLANE_SERVER`/`FLOWPLANE_TOKEN`) — see [CLI auth & contexts](cli-auth-and-contexts.md). Examples below use team `default`.
- The `flowplane-rls` binary installed from a published Flowplane release artifact, as shown in [Production Readiness](production-readiness.md).

The descriptor key in this guide is `api_key`, derived from an `x-api-key` request header; the
limit is **100 requests/minute per distinct `api_key`**.

## 1. Run the rate-limit service

`flowplane-rls` is a **separate process** from the control plane. It listens on two ports: a gRPC
port Envoy calls, and an HTTP admin port the control plane pushes policy to.

```bash
FLOWPLANE_RLS_GRPC_LISTEN=127.0.0.1:50051 \
FLOWPLANE_RLS_ADMIN_LISTEN=127.0.0.1:8081 \
FLOWPLANE_RLS_ALLOW_INSECURE_GRPC=yes-this-is-local-only \
FLOWPLANE_RLS_ALLOW_INSECURE_ADMIN=yes-this-is-local-only \
  flowplane-rls
```

The two `…_ALLOW_INSECURE_*` acknowledgements are required for the plaintext dev path:
`flowplane-rls` **fails closed** — a non-loopback gRPC bind refuses to start without the
`FLOWPLANE_RLS_GRPC_TLS_*` mTLS triad, a non-loopback admin bind refuses to start without
`FLOWPLANE_RLS_ADMIN_TLS_*` + the admin bearer token, and even loopback binds serve plaintext
only behind these explicit per-listener opt-ins (see the
[configuration reference](../reference/configuration.md)).

For a split-node deployment, bind these listeners on the RLS host interface that the control plane and Envoy can reach, and open the matching ports described in [Production Readiness](production-readiness.md#ports-and-network-paths). Keep the admin listener reachable only from the control-plane network. A non-loopback bind **requires** the TLS material below — see [Production: split-node with mTLS and an authenticated admin](#production-split-node-with-mtls-and-an-authenticated-admin).

Expected startup log — a `flowplane-rls starting` line carrying both bind addresses (the exact
prefix/format depends on the tracing setup; `grpc=` and `admin=` fields appear):

```
INFO flowplane_rls: flowplane-rls starting grpc=127.0.0.1:50051 admin=127.0.0.1:8081
```

Confirm the admin server is up:

```bash
curl -fsS http://127.0.0.1:8081/healthz && echo OK
# OK
```

The counter store is in-memory — no Redis, no database. (Counters reset on restart, and the
committed 1.1.0 topology is a **single** RLS instance; see
[Global rate limiting](../concepts/global-rate-limiting.md) for why.)

## 2. Point the control plane at it

Set two variables on the control plane and (re)start `flowplane serve`:

```bash
FLOWPLANE_RLS_GRPC_URL=127.0.0.1:50051 \
FLOWPLANE_RLS_ADMIN_URL=http://127.0.0.1:8081 \
  flowplane serve
```

- `FLOWPLANE_RLS_GRPC_URL` makes the control plane synthesize and inject a built-in
  `rate_limit_cluster` into every Envoy's CDS, so you never hand-write that cluster.
- `FLOWPLANE_RLS_ADMIN_URL` starts the `rls_sync` worker, which pushes the full policy set to the
  RLS on a 60 s reconcile loop (the self-healing backstop).

For production, also set the `FLOWPLANE_DATAPLANE_TLS_*` triad so the Envoy→RLS hop is mTLS; with
none set the injected cluster dials the RLS in plaintext h2c (dev only — and a production RLS
with its gRPC TLS triad configured will reject that plaintext dial at the handshake). The full
split-node setup is below; variable semantics are in the
[configuration reference](../reference/configuration.md).

## Production: split-node with mTLS and an authenticated admin

On a non-loopback bind, `flowplane-rls` refuses to start without its TLS material, and the CP
refuses to start if its push token would cross plaintext. The complete two-sided setup needs
one PKI decision: a CA (or two) that both sides cross-trust. The worked example below uses a
single private CA for the whole RLS triangle — the same pattern (and the same `openssl` recipe)
as [Register a dataplane over mTLS](register-dataplane-mtls.md).

**1. Mint the material** (on your CA host; one CA, three leaf certs):

```bash
# CA
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 825 \
  -keyout rls-ca.key -out rls-ca.pem -subj "/CN=flowplane-rls-ca"

mint() { # name subjectAltName
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
    -keyout "$1.key" -out "$1.csr" -subj "/CN=$1"
  openssl x509 -req -in "$1.csr" -CA rls-ca.pem -CAkey rls-ca.key -CAcreateserial \
    -days 365 -out "$1.pem" -extfile <(printf "subjectAltName=%s" "$2")
}
mint rls-grpc-server  "DNS:rls.example"          # RLS gRPC listener identity
mint rls-admin-server "DNS:rls.example"          # RLS admin HTTPS identity
mint envoy-client     "DNS:envoy-fleet.example"  # Envoy's client cert for the RLS hop
```

Generate the CP→RLS admin token once, e.g. `openssl rand -hex 32 > rls-admin.token`.

**2. RLS host** — both listeners bound on the reachable interface, fail-closed material set:

```bash
FLOWPLANE_RLS_GRPC_LISTEN=0.0.0.0:50051 \
FLOWPLANE_RLS_ADMIN_LISTEN=0.0.0.0:8081 \
FLOWPLANE_RLS_GRPC_TLS_CERT=/etc/flowplane/rls/rls-grpc-server.pem \
FLOWPLANE_RLS_GRPC_TLS_KEY=/etc/flowplane/rls/rls-grpc-server.key \
FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA=/etc/flowplane/rls/rls-ca.pem \
FLOWPLANE_RLS_ADMIN_TLS_CERT=/etc/flowplane/rls/rls-admin-server.pem \
FLOWPLANE_RLS_ADMIN_TLS_KEY=/etc/flowplane/rls/rls-admin-server.key \
FLOWPLANE_RLS_ADMIN_TOKEN_FILE=/etc/flowplane/rls/rls-admin.token \
  flowplane-rls
```

The startup log now shows `grpc_security=mtls` and `admin_security="https + bearer"`. An Envoy
without a client cert from `rls-ca.pem` fails the TLS handshake before any counter is touched;
an admin push without the exact bearer gets `401` and the enforced set is unchanged.

**3. Control plane** — dial https, present the same token, trust the private CA, and give the
Envoy fleet its client half:

```bash
FLOWPLANE_RLS_GRPC_URL=rls.example:50051 \
FLOWPLANE_RLS_ADMIN_URL=https://rls.example:8081 \
FLOWPLANE_RLS_ADMIN_TOKEN_FILE=/etc/flowplane/rls-admin.token \
FLOWPLANE_RLS_ADMIN_TLS_CA=/etc/flowplane/rls-ca.pem \
FLOWPLANE_DATAPLANE_TLS_CERT=/etc/envoy/certs/envoy-client.pem \
FLOWPLANE_DATAPLANE_TLS_KEY=/etc/envoy/certs/envoy-client.key \
FLOWPLANE_DATAPLANE_TLS_CLIENT_CA=/etc/envoy/certs/rls-ca.pem \
  flowplane serve
```

The `FLOWPLANE_DATAPLANE_TLS_*` paths are resolved **on the dataplane host by Envoy** (they ride
into CDS), so place the client cert/key + CA there; the admin token/CA paths are read by the CP
process. Health probes move to https: `curl --cacert rls-ca.pem https://rls.example:8081/healthz`.

Fail-closed checklist (each of these refuses startup rather than degrading): partial gRPC triad
or admin pair; non-loopback bind without its material; admin TLS without a token or a token
without admin TLS; CP token with a plaintext non-loopback admin URL; unreadable/empty/non-PEM
material in anything the `flowplane-rls` or `flowplane serve` process itself reads (the RLS
gRPC triad and admin pair, both sides' tokens, the CP's `FLOWPLANE_RLS_ADMIN_TLS_CA`). The
`FLOWPLANE_DATAPLANE_TLS_*` paths are the exception: they ride into CDS and are read by
**Envoy on the dataplane host**, so a bad path there surfaces as an Envoy cluster/TLS error on
the dataplane (and a failed Envoy→RLS handshake), not as a CP startup refusal.

Confirm the worker started (control-plane log; the CP logs JSON by default, so the message appears
as a structured field):

```
{"message":"rls_sync worker started","reconcile_secs":60}
```

## 3. Create a rate-limit domain and policy

A **domain** is a named limit group; a **policy** within it matches a descriptor set and sets the
limit. Create both with the CLI (each `create` reads its JSON body from `--file`):

```bash
echo '{"name":"checkout"}' > domain.json
flowplane rate-limit domain create --team default --file domain.json

echo '{"name":"per-client","spec":{"descriptors":{"api_key":"acme"},"requests_per_unit":100,"unit":"minute"}}' > policy.json
flowplane rate-limit policy create --team default --domain checkout --file policy.json
```

The REST equivalent:

```bash
curl -fsS -H "Authorization: Bearer $FLOWPLANE_TOKEN" -H 'Content-Type: application/json' \
  -X POST "$FLOWPLANE_SERVER/api/v1/teams/default/rate-limit-domains" \
  -d '{"name":"checkout"}'

curl -fsS -H "Authorization: Bearer $FLOWPLANE_TOKEN" -H 'Content-Type: application/json' \
  -X POST "$FLOWPLANE_SERVER/api/v1/teams/default/rate-limit-domains/checkout/policies" \
  -d '{"name":"per-client","spec":{"descriptors":{"api_key":"acme"},"requests_per_unit":100,"unit":"minute"}}'
```

`unit` is one of `second`, `minute`, `hour`, `day`. The `descriptors` map is the exact set the
route must emit for this policy to match (next step). See the
[rate-limit CLI](../reference/cli.md#rate-limit) and [REST reference](../reference/rest-api.md#rate-limiting).

## 4. Make the route emit the descriptor

The route that should be limited must emit a descriptor whose key matches the policy. Here the
`api_key` descriptor is taken from the `x-api-key` header. Add a `rate_limits` action to the route
in your route-config (`PATCH` the existing `api-routes`, or include it at create):

```json
{
  "action": {
    "cluster": "httpbin",
    "timeout_secs": 10,
    "rate_limits": [
      { "actions": [ { "type": "request_headers", "header_name": "x-api-key", "descriptor_key": "api_key" } ] }
    ]
  }
}
```

## 5. Attach the `global_rate_limit` filter to the listener

Add the filter to the listener's HTTP filter chain. **Omit `service_cluster`** — it defaults to the
built-in `rate_limit_cluster`, and you supply only the short policy domain (`checkout`). The control
plane composes the tenant-namespaced Envoy domain for you.

```json
{
  "http_filters": [
    {
      "filter": {
        "type": "global_rate_limit",
        "domain": "checkout",
        "timeout_ms": 200,
        "failure_mode_deny": false,
        "request_type": "external"
      }
    }
  ]
}
```

`failure_mode_deny: false` fails **open** if the RLS is unreachable (requests proceed); set it
`true` to fail **closed** (reject with `500`). If `FLOWPLANE_RLS_GRPC_URL` is **not** set, attaching
this filter is rejected `400` at config time — the control plane never emits a filter pointing at a
missing cluster. See the [`global_rate_limit` reference](../reference/filters.md).

## 6. Verify

First confirm Envoy has the filter and the **composed** domain. The emitted domain is the
tenant-namespaced form `{namespaceUUID(org)}|{namespaceUUID(team)}|checkout`, not the raw
`checkout`. The two leading segments are **SHA-256-derived** UUIDs — a stable, opaque namespace
of your org and team ids, **not** the raw org/team UUIDs — so match them by shape, not by your
own ids:

```bash
curl -fsS http://127.0.0.1:9901/config_dump | grep -oE '[0-9a-f-]{36}\|[0-9a-f-]{36}\|checkout'
# 16d8d7aa-7842-8166-88ac-c72392a9d771|04c9f1e3-3f4f-8e5d-8f75-5c5bb671fd58|checkout
```

(`9901` is the Envoy admin port from your bootstrap.) Then send traffic through the gateway listener. Replace `10000` with your listener port; the getting-started tutorial uses `10001`. The
first 100 requests in the minute pass; the 101st is rate-limited:

```bash
for i in $(seq 1 101); do
  curl -s -o /dev/null -w '%{http_code}\n' -H 'x-api-key: acme' http://127.0.0.1:10000/
done | sort | uniq -c
#  100 200
#    1 429
```

A different `x-api-key` value has its own counter (the descriptor value differs), and a different
team's identical `checkout` policy never shares this counter (the namespace prefix differs).

## Lifecycle and fail modes

Update and delete are concurrency-controlled. To pin the mutation to a specific version, pass
the current revision with the global `--revision <N>` flag (the `If-Match` value). Get the
current revision from a `GET`, e.g. `flowplane rate-limit policy get --team default --domain checkout per-client`
(the `revision` field). If you omit `--revision`, the CLI performs a read-modify-write: it reads
the current resource, sends that revision as `If-Match`, and still fails on a concurrent edit.

- **Update a limit:** write an update body with `spec` only (the policy name is the
  positional `per-client`), then pass the current revision:

  ```bash
  echo '{"spec":{"descriptors":{"api_key":"acme"},"requests_per_unit":200,"unit":"minute"}}' > policy-update.json
  flowplane rate-limit policy update --team default --domain checkout per-client --revision 1 --file policy-update.json
  ```

  The change reaches the RLS within the reconcile window (≤ 60 s).
- **Per-team override:** raise/lower one team's limit without touching the policy. The `--file`
  flag takes a **path**, so write the body first:

  ```bash
  echo '{"spec":{"requests_per_unit":500}}' > override.json
  flowplane rate-limit override set --team default --domain checkout --policy per-client --file override.json
  ```

- **Stop enforcing:** `flowplane rate-limit policy delete --team default --domain checkout per-client --revision 2` — enforcement stops within the reconcile window.
- **Force an immediate sync** (platform-admin only): `flowplane rate-limit force-repush`. The 60 s loop is the backstop, so this is only a fast path; an org/team token gets `403`.
- **RLS down:** behavior follows each filter's `failure_mode_deny` (open vs closed).

## Further reading

- [Global rate limiting](../concepts/global-rate-limiting.md) — architecture, tenancy, topology limits.
- [`global_rate_limit` filter reference](../reference/filters.md) — every field.
- [Configuration reference](../reference/configuration.md) — the `FLOWPLANE_RLS_*` / `FLOWPLANE_DATAPLANE_TLS_*` variables.
