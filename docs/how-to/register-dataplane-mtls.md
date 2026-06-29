# Register a dataplane and connect its agent over mTLS

> Audience: operators · Status: stable

This how-to walks one task end to end: **register a dataplane, issue its mTLS client certificate, and connect `fp-agent`.** It assumes you already run Flowplane day to day and have a working CLI context (server URL, org, team, token).

It assumes the control plane is already running with xDS mTLS configured. The xDS listener is **always** mTLS in production — there is no plaintext mode off loopback. If you have not stood that up yet, start with [Production Readiness](production-readiness.md) and set the `FLOWPLANE_XDS_TLS_*` triad as described in the [configuration reference](../reference/configuration.md). For local from-source practice only, use the [Getting started tutorial](../tutorials/getting-started.md).

## Prerequisites

The control plane is up with the xDS mTLS triad set (`FLOWPLANE_XDS_TLS_CERT` / `_KEY` / `_CLIENT_CA`), and — for the `issue` step below — the cert-issuer triad `FLOWPLANE_CERT_ISSUER_CA_CERT_PATH` and `FLOWPLANE_CERT_ISSUER_CA_KEY_PATH` (optionally `FLOWPLANE_CERT_ISSUER_TRUST_DOMAIN`, default `flowplane.local`) is set **on the control-plane process**. See the [configuration reference](../reference/configuration.md).

A **tenant org and a team must already exist** — a dataplane is registered under a team (`--team payments` below), and the platform org cannot host one. If you have only just bootstrapped the platform admin, first [create a tenant org and a team](create-tenant-org-and-team.md). Note that selecting the platform org for a tenant operation (`--org platform`) is rejected with `org_selector_required` (D-014): use your tenant org.

## 1. Register the dataplane

A dataplane is a named record under a team. The only required field is the name; `description` is optional.

CLI:

```bash
flowplane dataplane create edge-gateway-1 \
  --team payments \
  --description "Edge gateway, us-east"
```

REST (`POST /api/v1/teams/{team}/dataplanes`):

```bash
curl -sS -X POST \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  https://cp.example.com/api/v1/teams/payments/dataplanes \
  -d '{"name":"edge-gateway-1","description":"Edge gateway, us-east"}'
```

Body fields: `name` (required), `description` (optional, defaults to empty). Unknown fields are rejected. The response includes the dataplane `id` (a UUID) — **note it**, you need it for the agent in step 4.

## 2. Issue its mTLS client certificate

`issue` mints a leaf certificate from the configured Flowplane CA, registers its SPIFFE URI binding, and returns the certificate, private key, and CA bundle **once**. Flowplane never stores the private key — write it to your dataplane secret store immediately.

CLI:

```bash
flowplane dataplane cert issue edge-gateway-1 \
  --team payments \
  --ttl-hours 24
```

`ttl_hours` defaults to `24` and must be between `1` and `8760`.

REST (`POST /api/v1/teams/{team}/proxy-certificates/issue`):

```bash
curl -sS -X POST \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  https://cp.example.com/api/v1/teams/payments/proxy-certificates/issue \
  -d '{"dataplane":"edge-gateway-1","ttl_hours":24}'
```

The response (`IssuedProxyCertificateView`) contains:

- `certificate_pem` — the leaf **client** certificate the agent presents to the CP
- `private_key_pem` — the matching private key (not stored by Flowplane)
- `ca_certificate_pem` — the **issuer/trust CA** that signed the client cert above. This is the *client-cert chain* CA: it is what the control plane is configured to trust as its xDS `FLOWPLANE_XDS_TLS_CLIENT_CA` so it can verify the agent. It is **not** the agent's `--tls-ca-path` (see step 3).
- `certificate.spiffe_uri` — the identity that binds this stream to the team/dataplane

The SPIFFE identity is `spiffe://<trust-domain>/org/<org-id>/team/<team-id>/proxy/<dataplane-id>`, where `<trust-domain>` is `FLOWPLANE_CERT_ISSUER_TRUST_DOMAIN` (default `flowplane.local`). At runtime the control plane trusts the full registered SPIFFE URI as the binding key, not the path segments inside the cert.

Write the client cert and key to files the agent host can read:

```bash
# from the issue response
echo "$CERT_PEM" > /etc/flowplane/dp/client.crt
echo "$KEY_PEM"  > /etc/flowplane/dp/client.key
chmod 600 /etc/flowplane/dp/client.key
```

You also need a **server-trust CA** for the agent — the CA that signed the control plane's xDS *server* certificate (the cert the CP serves from `FLOWPLANE_XDS_TLS_CERT`). The agent uses it to verify the CP during the TLS handshake, so it is a separate input from the issue response. Obtain it from whoever provisioned the CP's xDS TLS and write it out:

```bash
echo "$CP_XDS_SERVER_CA_PEM" > /etc/flowplane/dp/server-ca.crt
```

> These are two different CAs. `ca_certificate_pem` from the issue response is the *client* chain CA (used by the CP to verify the agent). The agent's `--tls-ca-path` is the *server* CA (used by the agent to verify the CP). They are the **same file only if** the CP's xDS server certificate happens to be signed by that same issuer CA — the code does not require it.

> If your dataplane already has externally-issued certs, use `dataplane cert register` / `POST /api/v1/teams/{team}/proxy-certificates` instead to register the SPIFFE binding without minting a key. (Optional background — not needed to finish this task: the certificate lifecycle design, SPIFFE format, and revocation internals are in the design records linked under Further reading.)

## 3. Run `fp-agent`

Point the agent at the control-plane diagnostics gRPC endpoint, give it the dataplane UUID from step 1, pass its client cert and key from step 2, and pass the **server-trust CA** (the CA for the CP's xDS server cert). The TLS cert/key/CA flags are **all-or-none** — supply all three or none.

```bash
fp-agent \
  --cp-endpoint https://cp.example.com:18000 \
  --dataplane-id 7b1f0a2c-... \
  --tls-cert-path /etc/flowplane/dp/client.crt \
  --tls-key-path  /etc/flowplane/dp/client.key \
  --tls-ca-path   /etc/flowplane/dp/server-ca.crt \
  --tls-server-name cp.example.com
```

Each flag has an env-var equivalent:

| Flag | Env | Notes |
|------|-----|-------|
| `--cp-endpoint` | `FLOWPLANE_AGENT_CP_ENDPOINT` | Use `https://` for any non-loopback host; plaintext is allowed only for loopback. |
| `--dataplane-id` | `FLOWPLANE_AGENT_DATAPLANE_ID` | The UUID from step 1. |
| `--tls-cert-path` | `FLOWPLANE_AGENT_TLS_CERT_PATH` | The agent's **client cert** — issued `certificate_pem` (`client.crt`). |
| `--tls-key-path` | `FLOWPLANE_AGENT_TLS_KEY_PATH` | The agent's **client key** — issued `private_key_pem` (`client.key`). |
| `--tls-ca-path` | `FLOWPLANE_AGENT_TLS_CA_PATH` | The **server-trust CA** that signed the CP's xDS server cert (`server-ca.crt`). Not the issued `ca_certificate_pem` unless the same CA signs both. |
| `--tls-server-name` | `FLOWPLANE_AGENT_TLS_SERVER_NAME` | Name verified against the CP server cert (default `localhost`). Set this to match your CP cert SAN. |

The agent also exposes a local health endpoint on `127.0.0.1:19902` (`--health-bind-addr`). Full flag/env list is in the [configuration reference](../reference/configuration.md) and [CLI reference](../reference/cli.md).

## 4. Verify the dataplane is connected

The agent serves `/healthz` once it has scraped Envoy admin and received a diagnostics ack from the control plane:

```bash
curl -fsS http://127.0.0.1:19902/healthz   # "ok" when polling + acks are fresh
```

On the control-plane side, confirm telemetry is landing and the stream is healthy:

```bash
# heartbeat / counters for this dataplane
flowplane dataplane get edge-gateway-1 --team payments

# team rollup: live vs stale dataplane counts
flowplane stats overview --team payments

# xDS stream status
flowplane ops xds status --team payments
```

`dataplane get` (`GET /api/v1/teams/{team}/dataplanes/{name}`) shows `last_heartbeat_at` advancing once the agent is reporting; `stats overview` reflects the dataplane under `live_dataplanes`.

## Further reading

- [Getting started tutorial](../tutorials/getting-started.md) — stand up the control plane and xDS mTLS.
- [Configuration reference](../reference/configuration.md) — every env var, including the cert-issuer and xDS TLS triads.
- [CLI reference](../reference/cli.md) — full `dataplane` and `dataplane cert` command surface.
- Design references (optional): [spec/04-xds.md](../../spec/04-xds.md), [spec/05-auth.md](../../spec/05-auth.md) — SPIFFE binding and certificate revocation internals.
