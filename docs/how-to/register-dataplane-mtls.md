# Register a dataplane and connect its agent over mTLS

> Audience: operators · Status: stable

This how-to walks one task end to end: **register a dataplane, issue its mTLS client certificate, and connect `flowplane-agent`.** It assumes you already run Flowplane day to day and have a working CLI context (server URL, org, team, token).

It assumes the control plane is already running with xDS mTLS configured. The xDS listener is **always** mTLS in production — there is no plaintext mode off loopback. If you have not stood that up yet, start with [Production Readiness](production-readiness.md) and set the `FLOWPLANE_XDS_TLS_*` triad as described in the [configuration reference](../reference/configuration.md). For local from-source practice only, use the [Getting started tutorial](../tutorials/getting-started.md).

## Prerequisites

The control plane is up with the xDS mTLS triad set (`FLOWPLANE_XDS_TLS_CERT` / `_KEY` / `_CLIENT_CA`), and — for the `issue` step below — the cert-issuer triad `FLOWPLANE_CERT_ISSUER_CA_CERT_PATH` and `FLOWPLANE_CERT_ISSUER_CA_KEY_PATH` (optionally `FLOWPLANE_CERT_ISSUER_TRUST_DOMAIN`, default `flowplane.local`) is set **on the control-plane process**. See the [configuration reference](../reference/configuration.md).

The configured issuer certificate must be currently valid, match its private key, contain
`CA:TRUE`, `keyCertSign`, and a Subject Key Identifier, and must not restrict extended key usage
away from `clientAuth`. A standards-complete intermediate certificate is supported when it is
the explicit trust anchor. Before upgrading an existing issuer, run the detection and ordered
redistribution procedure in [Production Readiness](production-readiness.md#issuer-ca-compatibility-and-upgrade).

A **tenant org and a team must already exist** — a dataplane is registered under a team (`--team payments` below), and the platform org cannot host one. If you have only just bootstrapped the platform admin, first [create a tenant org and a team](create-tenant-org-and-team.md). Note that selecting the platform org for a tenant operation (`--org platform`) is rejected with `org_selector_required` (D-014): use your tenant org.

The dataplane host must have Envoy and `flowplane-agent` installed. Install `flowplane-agent` from the published Flowplane release archive as shown in [Production Readiness](production-readiness.md).

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

`issue` mints a leaf certificate from the configured Flowplane CA, registers its SPIFFE URI binding, and returns the certificate, private key, and CA bundle **once**. Flowplane never stores the private key — the response is the only copy, so treat the whole response as a secret from the moment it lands on disk.

The CLI's global `--out` flag writes the response under the **process umask** and sets no file mode of its own (`0644` under the common `umask 022`); if the file already exists it is truncated in place and **keeps its old mode** (see the [CLI reference](../reference/cli.md#global-options)). The recipe below therefore makes the file private *at creation* — a `0700` parent directory, `umask 077`, and the target pre-created at `0600` so a re-run over a leftover file is just as safe — rather than writing it and tightening it afterwards. Two more things it deliberately does: it passes `--json`, because on an interactive terminal the CLI's default `table` format prints only a one-line `created …` summary for a mutation and writes **nothing** to `--out`; and it runs as the operator user that will hand the files to the dataplane runtime (step 2b):

CLI:

```bash
umask 077                                                 # every file created below is 0600
install -d -m 0700 /etc/flowplane/dp                      # private parent; no other user can traverse it
install -m 0600 /dev/null /etc/flowplane/dp/issue.json    # (re)create the target at 0600, even if it already exists
flowplane --json --out /etc/flowplane/dp/issue.json \
  dataplane cert issue edge-gateway-1 \
  --team payments \
  --ttl-hours 24
```

`ttl_hours` defaults to `24` and must be between `1` and `8760`. `/etc/flowplane/dp/issue.json` now contains the private key and is `0600` inside a `0700` directory; it is a one-time file that you delete in step 2b. (`-o json` is equivalent to `--json`; piping stdout also selects JSON, but `--json` makes the recipe independent of how it is run.)

REST (`POST /api/v1/teams/{team}/proxy-certificates/issue`) — `curl -o` follows the umask in exactly the same way, so keep the same `umask 077` and `0700` directory:

```bash
umask 077
install -d -m 0700 /etc/flowplane/dp
install -m 0600 /dev/null /etc/flowplane/dp/issue.json
curl -sS -X POST \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  https://cp.example.com/api/v1/teams/payments/proxy-certificates/issue \
  -d '{"dataplane":"edge-gateway-1","ttl_hours":24}' \
  -o /etc/flowplane/dp/issue.json
```

The response (`IssuedProxyCertificateView`) contains the fields below. Note the two shapes on disk: the CLI file wraps the view in the CLI's usual `data` envelope (`{"schemaVersion":1,"kind":"proxyCertificate","data":{…}}`), while a direct REST call returns the view itself as the top-level object with no `data` wrapper. The `jq` paths in step 2b are written for the CLI file; if you saved the REST body, drop the leading `.data` (for example `jq -r '.private_key_pem'`).

- `certificate_pem` — the leaf **client** certificate the agent presents to the CP
- `private_key_pem` — the matching private key (not stored by Flowplane)
- `ca_certificate_pem` — the **issuer/trust CA** that signed the client cert above. This is the *client-cert chain* CA: it is what the control plane is configured to trust as its xDS `FLOWPLANE_XDS_TLS_CLIENT_CA` so it can verify the agent. It is **not** the agent's `--tls-ca-path` (see step 3).
- `certificate.spiffe_uri` — the identity that binds this stream to the team/dataplane

The SPIFFE identity is `spiffe://<trust-domain>/org/<org-id>/team/<team-id>/proxy/<dataplane-id>`, where `<trust-domain>` is `FLOWPLANE_CERT_ISSUER_TRUST_DOMAIN` (default `flowplane.local`). At runtime the control plane binds the verified leaf's SPIFFE URI **and SHA-256 fingerprint** to one exact active registry row. A different leaf with the same URI is rejected unless it is separately registered for bounded rotation overlap.

### 2b. Split the response into runtime files and dispose of the one-time JSON

Split the one-time response into the files Envoy and `flowplane-agent` read at runtime. Each target is pre-created at its intended mode with `install -m … /dev/null` and only then written by redirect: a redirect into an existing file keeps that file's mode, so the private key is `0600` from its first byte and the recipe stays safe when re-run over files from an earlier attempt. `umask 077` is kept as a backstop for anything created without a pre-creation line:

```bash
(
  set -eu
  umask 077
  install -m 0600 /dev/null /etc/flowplane/dp/client.key    # secret: (re)created at 0600
  install -m 0644 /dev/null /etc/flowplane/dp/client.crt    # public leaf: (re)created at 0644
  # CLI-written issue.json (data envelope). For a raw REST body use '.private_key_pem' / '.certificate_pem'.
  jq -er '.data.private_key_pem' /etc/flowplane/dp/issue.json > /etc/flowplane/dp/client.key
  jq -er '.data.certificate_pem' /etc/flowplane/dp/issue.json > /etc/flowplane/dp/client.crt
  openssl pkey -noout -in /etc/flowplane/dp/client.key
  openssl x509 -noout -in /etc/flowplane/dp/client.crt
  rm -f /etc/flowplane/dp/issue.json
)
```

`jq -e` fails on a missing path instead of writing the string `null`, and the two `openssl` lines parse what was written; with `set -e` the subshell stops at the first failure, so the `rm -f` of the one-time JSON on the last line runs only after both files have been validated. If the block fails, `issue.json` is still there: fix the cause (usually the wrong `jq` path for the shape you saved) and run the block again — the key is not recoverable from Flowplane, so never delete the JSON by hand before the block has succeeded.

You also need a **server-trust CA** for the agent — the CA that signed the control plane's xDS *server* certificate (the cert the CP serves from `FLOWPLANE_XDS_TLS_CERT`). The agent uses it to verify the CP during the TLS handshake, so it is a separate input from the issue response. Obtain it from whoever provisioned the CP's xDS TLS and write it out. It is a public trust anchor, so `0644` is its intended mode:

```bash
install -m 0644 /dev/null /etc/flowplane/dp/server-ca.crt   # pre-create at the intended mode
cat path/to/cp-xds-server-ca.crt > /etc/flowplane/dp/server-ca.crt
```

Intended modes, so that a later audit can tell "as designed" from "drifted":

| File | Contains | Mode | Why |
|------|----------|------|-----|
| `/etc/flowplane/dp/` | everything below | `0700` | only the runtime user (or the operator handing over) can traverse or list it |
| `issue.json` | full issue response incl. `private_key_pem` | `0600`, then **deleted** | one-time secret; no reason for it to exist after the split |
| `client.key` | `private_key_pem` | `0600` | secret; only the process presenting the client certificate may read it |
| `client.crt` | `certificate_pem` (leaf) | `0644` | public; presented to the control plane on every handshake |
| `server-ca.crt` | xDS server-trust CA | `0644` | public trust anchor |

**Runtime ownership.** Envoy and `flowplane-agent` must run as a dedicated dataplane service user and read these files as that user — never as root and never as a shared login account. Hand the directory over once the split is done (`flowplane-dp` is a placeholder for your service user):

```bash
chown -R flowplane-dp:flowplane-dp /etc/flowplane/dp
```

If Envoy and the agent run as *different* users, give them a common group and use `0750` on the directory and `0640` on `client.key` instead; do not fall back to world-readable modes.

**Secret-store ingestion.** If your platform has a secret store (Vault, AWS Secrets Manager, a Kubernetes/Nomad secret, and so on), ingest `client.key` into it straight after the split block above and let the store deliver the key to the dataplane host at `0600` through its own mechanism, then remove the local `client.key`, instead of keeping it on the operator machine at all. The [AWS runbook](aws-secure-deployment.md#local-dataplane-smoke) shows the same pattern in the other direction — pre-creating a `0600` file before a store writes into it. The `--out` file itself is never a substitute for a secret store: Flowplane cannot control host ownership, ACLs, mounts, backups, or snapshots of that path.

**Cleanup check.** The split block already deleted the one-time response on success. Confirm the directory holds exactly the runtime files at their intended modes and nothing else:

```bash
ls -ln /etc/flowplane/dp     # expect: client.key 0600, client.crt and server-ca.crt 0644, no issue.json
```

If the file ever landed somewhere permissive (for example you ran `--out` without the umask above), treat the key as exposed: rotate by issuing a replacement, connecting it, then revoking the old serial — see the rotation note at the end of this step.

> These are two different CAs. `ca_certificate_pem` from the issue response is the *client* chain CA (used by the CP to verify the agent). The agent's `--tls-ca-path` is the *server* CA (used by the agent to verify the CP). They are the **same file only if** the CP's xDS server certificate happens to be signed by that same issuer CA — the code does not require it.

> If your dataplane already has externally-issued certs, use `dataplane cert register` / `POST /api/v1/teams/{team}/proxy-certificates` instead to register the SPIFFE binding without minting a key. Registration accepts only the dataplane name and a PEM chain with the leaf first, followed by intermediates:
>
> ```json
> {
>   "dataplane": "edge-1",
>   "certificate_chain_pem": "-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----\n"
> }
> ```
>
> The control-plane verifies the chain against `FLOWPLANE_XDS_TLS_CLIENT_CA`, requires a client-auth leaf whose SPIFFE URI identifies that dataplane UUID, and derives serial, fingerprint, and validity from the leaf. It rejects caller-asserted identity metadata, untrusted or ambiguous chains, and registration when xDS client trust is not configured.

> Rotation is issue/register replacement → connect and verify replacement health → revoke old.
> Up to two unrevoked exact credentials may overlap for one dataplane. Do not revoke first.

## 3. Generate and run the Envoy bootstrap

Envoy must connect to the control plane over xDS mTLS before `flowplane-agent`
can report a healthy dataplane. Generate the bootstrap on the dataplane host
with the xDS address and the same certificate paths Envoy will see at runtime:

```bash
flowplane --out /etc/flowplane/dp/envoy.yaml dataplane bootstrap edge-gateway-1 \
  --team payments \
  --mode mtls \
  --xds-host cp.example.com \
  --xds-port 18000 \
  --cert-path /etc/flowplane/dp/client.crt \
  --key-path /etc/flowplane/dp/client.key \
  --ca-path /etc/flowplane/dp/server-ca.crt
```

`--xds-host` must be the address the dataplane can reach for the control
plane's xDS listener. It is often different from the REST/API hostname. The
certificate paths are dataplane-local paths; if Envoy runs in a container, mount
the files at those exact paths or generate the bootstrap with the container
paths.

Start Envoy with the generated bootstrap. The exact service manager is up to
your platform, but the command shape is:

```bash
envoy -c /etc/flowplane/dp/envoy.yaml --log-level info
```

Envoy admin should stay local to the dataplane unit. `flowplane-agent` reads
Envoy admin through loopback and sends curated diagnostics to the control plane;
operators should use Flowplane's `stats` / `ops xds` commands instead of direct
Envoy admin as the product workflow.

## 4. Run `flowplane-agent`

Point the agent at the control-plane diagnostics gRPC endpoint, give it the dataplane UUID from step 1, pass its client cert and key from step 2, and pass the **server-trust CA** (the CA for the CP's xDS server cert). The TLS cert/key/CA flags are **all-or-none** — supply all three or none.

```bash
flowplane-agent \
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

## 5. Verify the dataplane is connected

The agent serves `/healthz` once it has scraped Envoy admin and received a diagnostics ack from the control plane:

```bash
curl -fsS http://127.0.0.1:19902/healthz   # "ok" when polling + acks are fresh
```

`/healthz` is a readiness signal, not a process-liveness signal. During a temporary diagnostics-stream or control-plane outage the long-running agent remains alive, keeps its bounded Envoy-scrape queue, and returns `503`; Envoy continues serving its last-good configuration. Daemon mode reconnects automatically with bounded exponential backoff, while `--once` and explicit invalid or unauthorized acknowledgments remain fail-fast. Each reconnect rebuilds the diagnostics channel and rereads the CA, client certificate, and private key files, so an atomically replaced valid identity is used on the next attempt and an invalid replacement remains rejected.

After the control plane is functional and accepts and commits reports, use this conservative readiness-recovery bound: the remaining current report-attempt deadline, plus the maximum jittered backoff of 6 seconds, plus one complete successful report-attempt deadline, plus at most one poll interval for a newly committed heartbeat after an idempotent replay. A listener that is up while its database cannot commit reports may continue returning transport errors, so this bound does not start until acceptance and commit are possible.

When the agent uses a shared-container namespace such as Compose `network_mode: service:envoy`, its lifecycle is coupled to that Envoy container. If Envoy is recreated, recreate the namespace-sharing agent container as well; an agent restart policy is defense in depth and is not the diagnostics recovery mechanism. See [Production readiness](production-readiness.md) for the outage runbook and the [configuration reference](../reference/configuration.md) for exact defaults and constraints.

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

To retire an identity without deleting its audit/certificate history, run
`flowplane dataplane delete <name> --team <team> --reason <reason> --revision <revision> --yes`.
Default get/list calls omit retired rows; authorized operators can inspect blockers with
`flowplane dataplane list --team <team> --include-retired`. Reusing the retired name creates a new
dataplane UUID and SPIFFE URI; credentials from the retired incarnation cannot authenticate it.

## Further reading

- [Getting started tutorial](../tutorials/getting-started.md) — stand up the control plane and xDS mTLS.
- [Configuration reference](../reference/configuration.md) — every env var, including the cert-issuer and xDS TLS triads.
- [CLI reference](../reference/cli.md) — full `dataplane` and `dataplane cert` command surface.
- Design references (optional): [spec/04-xds.md](../../spec/04-xds.md), [spec/05-auth.md](../../spec/05-auth.md) — SPIFFE binding and certificate revocation internals.
