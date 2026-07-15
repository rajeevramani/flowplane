# Configuration & environment variables

> Audience: operators, platform-engineers · Status: stable

Every `FLOWPLANE_*` variable read by the control plane (`server`), the rate-limit service (`rls`, the `flowplane-rls` binary), the dataplane agent (`agent`, the `flowplane-agent` binary), and the CLI. Each variable appears once; the **Component** column says which process reads it.

**Precedence**

| Surface | Resolution order |
|---------|------------------|
| Server (`flowplane serve`) | env var > TOML file (`FLOWPLANE_CONFIG`) > built-in default |
| CLI | flag > env var > selected context > CLI config file > built-in default (uniform for every value, including the bearer token) |

Booleans accept `true`/`1`/`yes` and `false`/`0`/`no`. Invalid server values fail startup with an `invalid_config` error.

## Variables

| Variable | Component | Default | Required | Meaning |
|----------|-----------|---------|----------|---------|
| `FLOWPLANE_API_ADDR` | server | `0.0.0.0:8080` | no | REST + MCP listen address. |
| `FLOWPLANE_XDS_ADDR` | server | `0.0.0.0:18000` | no | xDS gRPC listen address (ADS/SDS + capture). |
| `FLOWPLANE_DATABASE_URL` | server | — | yes | PostgreSQL URL; falls back to `DATABASE_URL` if unset. |
| `FLOWPLANE_DB_MAX_CONNECTIONS` | server | `10` | no | Max DB pool connections; must be ≥ 1. |
| `FLOWPLANE_API_TLS_CERT` | server | — | no ¹ | API listener certificate path. |
| `FLOWPLANE_API_TLS_KEY` | server | — | no ¹ | API listener private key path. |
| `FLOWPLANE_API_INSECURE` | server | `false` | no ² | Serve the API over plaintext; logs a startup warning. |
| `FLOWPLANE_XDS_TLS_CERT` | server | — | no ³ | xDS server certificate path. |
| `FLOWPLANE_XDS_TLS_KEY` | server | — | no ³ | xDS server private key path. |
| `FLOWPLANE_XDS_TLS_CLIENT_CA` | server | — | no ³ | CA bundle dataplane client certs must chain to. |
| `FLOWPLANE_LOG_FORMAT` | server | `json` | no | Log format: `json` or `pretty`. |
| `FLOWPLANE_LOG` | server | `info` | no | `tracing` env-filter directive. |
| `FLOWPLANE_OTLP_ENDPOINT` | server | — | no | OTLP trace export endpoint; unset disables export. When set, AI gateway hop timelines are also exported as spans (one span per hop, nested under a per-request span); export is best-effort and never affects request handling or trace-row persistence. |
| `FLOWPLANE_DEV_MODE` | server | `false` | no ⁴ | In-process OIDC issuer + seeded resources; needs the `dev-oidc` build feature. |
| `FLOWPLANE_DEV_MODE_ACK` | server | — | no ⁴ | In release builds, dev mode requires this to equal `yes-this-is-not-production`. |
| `FLOWPLANE_DEV_TOKEN_PATH` | server | `~/.flowplane/dev-token` | no | Dev mode only: write the per-boot dev bearer token to this file (0600) so a sibling/init container can read it. When unset, the token is written best-effort to the well-known `~/.flowplane/dev-token` (WARN on failure, boot continues) so the local CLI can auto-discover it; a set path — env **or** the config file's `dev_token_path`, both are explicit — is written fail-loud and suppresses the default. Ignored outside dev mode. |
| `FLOWPLANE_DEV_TOKEN_TTL` | server | `86400` | no | Dev mode only: lifetime in **seconds** of the per-boot dev bearer token. Defaults to 24h so a local session does not expire mid-use; a non-numeric or non-positive value falls back to the default. Ignored outside dev mode; never affects production token lifetimes. |
| `FLOWPLANE_BOOTSTRAP_TOKEN` | server | — | first boot ¹³ | Operator-supplied one-shot bootstrap token (≥ 32 chars after trimming). Seeds first-admin setup; the value is never logged. |
| `FLOWPLANE_BOOTSTRAP_TOKEN_FILE` | server | — | first boot ¹³ | Path to read the bootstrap token from; **takes precedence** over `FLOWPLANE_BOOTSTRAP_TOKEN`. File delivery is safer than env. |
| `FLOWPLANE_ALLOW_LOGGED_BOOTSTRAP_TOKEN` | server | — | no | Local-only escape hatch: the exact value `yes-this-is-local-only` re-enables generating and **logging** a token; it is also written best-effort to `~/.flowplane/bootstrap-token` (0600) so `curl -H "Authorization: Bearer $(cat ~/.flowplane/bootstrap-token)"` works without log surgery. Never set in production. |
| `FLOWPLANE_OIDC_AUDIENCE` | server | — | no ⁵ | Expected JWT `aud` claim. |
| `FLOWPLANE_OIDC_JWKS_URI` | server | — | no | JWKS endpoint override (optional even with OIDC set). |
| `FLOWPLANE_OIDC_CA_BUNDLE` | server | — | no ¹⁴ | PEM file (one or more CA certs) the control plane trusts **in addition to** its bundled roots when fetching OIDC discovery + JWKS. Needed when the IdP is reachable only through a **TLS-intercepting egress proxy** (the outbound fetch otherwise fails `invalid peer certificate: UnknownIssuer`). Takes effect only when OIDC is configured (issuer + audience set); ignored in dev mode. |
| `FLOWPLANE_TENANT_WRITE_LIMIT_PER_MIN` | server | `120` | no | Per-tenant mutating-request budget per minute; must be ≥ 1. |
| `FLOWPLANE_SECRET_ENCRYPTION_KEY` | server | — | for secrets | Active key-encryption key; 32 raw bytes or base64. ⁷ |
| `FLOWPLANE_SECRET_ENCRYPTION_KEY_ID` | server | `default` | no | Identifier for the active KEK, used for rotation. ⁸ |
| `FLOWPLANE_SECRET_ENCRYPTION_KEYS` | server | — | no | Retired-key keyring so encrypted secrets stay decryptable during rotation. ⁹ |
| `FLOWPLANE_CERT_ISSUER_CA_CERT_PATH` | server | — | for issuance | Issuing CA certificate PEM path. |
| `FLOWPLANE_CERT_ISSUER_CA_KEY_PATH` | server | — | for issuance | Issuing CA private key PEM path. |
| `FLOWPLANE_CERT_ISSUER_TRUST_DOMAIN` | server | `flowplane.local` | no | SPIFFE trust domain for issued dataplane identities. |
| `FLOWPLANE_UPSTREAM_CA_BUNDLE` | server | `/etc/ssl/certs/ca-certificates.crt` | no | CA bundle path Envoy uses to verify materialized TLS upstreams that name neither an SDS validation secret nor an explicit `ca_cert_file`. The control plane reads this value at xDS-translation time, but the file itself is read by Envoy/dataplane (it must exist on the dataplane host), not by the control plane. Per-cluster `insecure_skip_verify` opts a cluster out of verification. |
| `FLOWPLANE_DISCOVERY_ALLOWED_DESTINATIONS` | server | — | no | Comma-separated `IP:port` allowlist for traffic discovery (for example `10.0.0.1:8080` or `[2001:db8::1]:443`); entries that are not a valid `IP:port` are ignored. |
| `FLOWPLANE_MCP_ALLOWED_ORIGINS` | server | `http://localhost,http://127.0.0.1,http://[::1]` | no | Comma-separated allowed `Origin` values for the MCP endpoint. |
| `FLOWPLANE_EGRESS_ADVISORY_ENABLED` | server | `true` (`false` in dev mode) | no | Write-time egress advisory: tenant-authored upstream hosts (clusters, AI providers, expose, route-generation apply) that currently resolve to a protected destination (cloud metadata endpoints, loopback, link-local, CP infra addresses) are rejected at create/update. **Defaults off when `FLOWPLANE_DEV_MODE=true`** — dev mode is single-host, so loopback upstreams are legitimate; set this explicitly to override in either direction. Operator-only; disabling logs a startup warning. Defense-in-depth — cannot stop a post-write DNS rebind; see [`../how-to/dataplane-egress-security.md`](../how-to/dataplane-egress-security.md). TOML section `[egress_advisory] enabled`. |
| `FLOWPLANE_EGRESS_ADVISORY_DENIED_CIDRS` | server | — | production posture | Comma-separated CIDRs/IPs added to the advisory deny set (for example `10.20.0.0/16,fd00:1::/64`). **The authoritative source for the control plane's and xDS endpoint's routable ranges** — listener binds are usually `0.0.0.0` and cannot provide them; without this the advisory covers metadata/loopback/DB/RLS but not the CP's own addresses. TOML `[egress_advisory] infra_denied_cidrs`. |
| `FLOWPLANE_RLS_GRPC_URL` | server | — | no ¹⁵ | gRPC `host:port` of the rate-limit service. When set, the CP injects the built-in `rate_limit_cluster` into CDS and `global_rate_limit` filters may use the default `service_cluster`. Unset ⇒ a built-in-path `global_rate_limit` filter is rejected `400` at config time. |
| `FLOWPLANE_RLS_ADMIN_URL` | server | — | no | HTTP admin URL of the rate-limit service. When set, the CP starts the `rls_sync` worker that pushes the full policy set to the RLS on the reconcile loop. |
| `FLOWPLANE_RLS_RECONCILE_SECS` | server | `60` | no ¹⁶ | Seconds between CP→RLS reconcile pushes. Clamped to `1..=60`. |
| `FLOWPLANE_DATAPLANE_TLS_CERT` | server | — | no ¹⁷ | Client certificate PEM the injected `rate_limit_cluster` presents to the RLS (Envoy→RLS mTLS). |
| `FLOWPLANE_DATAPLANE_TLS_KEY` | server | — | no ¹⁷ | Client private key PEM for the Envoy→RLS hop. |
| `FLOWPLANE_DATAPLANE_TLS_CLIENT_CA` | server | — | no ¹⁷ | CA bundle the injected cluster verifies the RLS server certificate against. |
| `FLOWPLANE_RLS_GRPC_LISTEN` | rls | `127.0.0.1:50051` | no ¹⁸ | `flowplane-rls`: Envoy-facing gRPC `RateLimitService` bind address (IP literal). **Fail-closed**: a non-loopback bind refuses to start without the `FLOWPLANE_RLS_GRPC_TLS_*` triad. |
| `FLOWPLANE_RLS_ADMIN_LISTEN` | rls | `127.0.0.1:8081` | no ¹⁹ | `flowplane-rls`: CP-facing HTTP admin bind address (`/api/v1/admin/rls/policies`, `/healthz`, `/readyz`). **Fail-closed**: a non-loopback bind refuses to start without admin TLS + the admin token. |
| `FLOWPLANE_RLS_ADMIN_TLS_CERT` | rls | — | no ¹⁹ | `flowplane-rls`: server certificate PEM for the admin HTTPS listener. |
| `FLOWPLANE_RLS_ADMIN_TLS_KEY` | rls | — | no ¹⁹ | `flowplane-rls`: server private key PEM for the admin HTTPS listener. |
| `FLOWPLANE_RLS_ADMIN_TOKEN` | rls | — | no ¹⁹ | `flowplane-rls`: bearer credential every non-health admin request must present (`Authorization: Bearer …`, constant-time compare). Mutually exclusive with `_FILE`. |
| `FLOWPLANE_RLS_ADMIN_TOKEN_FILE` | rls | — | no ¹⁹ | `flowplane-rls`: path to a file holding the admin bearer credential (trailing newline trimmed). Mutually exclusive with `FLOWPLANE_RLS_ADMIN_TOKEN`. |
| `FLOWPLANE_RLS_ALLOW_INSECURE_ADMIN` | rls | — | no ¹⁹ | `flowplane-rls`: explicit acknowledgement (`yes-this-is-local-only`) that the admin listener may serve **plaintext and unauthenticated on a loopback bind** (dev only). Never unlocks a non-loopback bind. |
| `FLOWPLANE_RLS_GRPC_TLS_CERT` | rls | — | no ¹⁸ | `flowplane-rls`: server certificate PEM the gRPC listener presents to Envoy (mTLS server half). |
| `FLOWPLANE_RLS_GRPC_TLS_KEY` | rls | — | no ¹⁸ | `flowplane-rls`: server private key PEM for the gRPC listener. |
| `FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA` | rls | — | no ¹⁸ | `flowplane-rls`: CA bundle the Envoy **client** certificate must chain to. Client certs are required at the TLS layer when the triad is set. |
| `FLOWPLANE_RLS_ALLOW_INSECURE_GRPC` | rls | — | no ¹⁸ | `flowplane-rls`: explicit acknowledgement (`yes-this-is-local-only`) that the gRPC listener may serve **plaintext on a loopback bind** (dev only). Never unlocks a non-loopback bind. |
| `FLOWPLANE_AGENT_ENVOY_ADMIN_URL` | agent | `http://127.0.0.1:9901` | no | Envoy admin base URL (usually loopback). |
| `FLOWPLANE_AGENT_CP_ENDPOINT` | agent | — | yes | Control-plane diagnostics gRPC endpoint. ¹⁰ |
| `FLOWPLANE_AGENT_DATAPLANE_ID` | agent | — | yes | Dataplane UUID registered in Flowplane. |
| `FLOWPLANE_AGENT_POLL_INTERVAL_SECS` | agent | `10` | no | Envoy admin stats poll interval (seconds). ¹¹ |
| `FLOWPLANE_AGENT_QUEUE_CAP` | agent | `256` | no | Max queued reports before backpressure. ¹² |
| `FLOWPLANE_AGENT_TLS_CERT_PATH` | agent | — | no ⁶ | Client certificate PEM for mTLS to the CP. |
| `FLOWPLANE_AGENT_TLS_KEY_PATH` | agent | — | no ⁶ | Client key PEM for mTLS. |
| `FLOWPLANE_AGENT_TLS_CA_PATH` | agent | — | no ⁶ | CP/server CA PEM. |
| `FLOWPLANE_AGENT_TLS_SERVER_NAME` | agent | `localhost` | no | Server name for TLS verification. |
| `FLOWPLANE_AGENT_HEALTH_BIND_ADDR` | agent | `127.0.0.1:19902` | no | Local health endpoint bind address. |
| `FLOWPLANE_SERVER` | cli | `http://127.0.0.1:8080` | no | API base URL the CLI targets. |
| `FLOWPLANE_TOKEN` | cli | — | no | Bearer token for API requests (env tier; below `--token`, above context/file/credentials). |
| `FLOWPLANE_ORG` | cli | — | no | Active organization (name or UUID). |
| `FLOWPLANE_TEAM` | cli | — | no | Active team. |
| `FLOWPLANE_TIMEOUT` | cli | `30` | no | HTTP request timeout in seconds (env tier; below `--timeout`, above the default). |
| `FLOWPLANE_OIDC_CLIENT_ID` | cli | — | no | OIDC client id for CLI login. |
| `FLOWPLANE_OIDC_SCOPE` | cli | `openid email profile` | no | OIDC scopes requested at CLI login. |
| `FLOWPLANE_OIDC_CALLBACK_URL` | cli | `http://127.0.0.1:8976/callback` | no | OAuth callback URL for CLI login. |
| `FLOWPLANE_CONFIG` | server, cli | CLI: `~/.flowplane/config.toml` | no | Server: path to the TOML config file (no file if unset). CLI: path to the CLI config file. |
| `FLOWPLANE_OIDC_ISSUER` | server, cli | — | no ⁵ | Server: OIDC issuer URL for auth. CLI: issuer for CLI login. |

## Constraints

Enforcement timing varies: rows ¹–⁵ are validated at **server startup** (`flowplane serve`); rows ⁶, ¹⁰–¹² at **agent startup** (`flowplane-agent`); rows ⁷–⁹ at **use time** when the secret encrypt/decrypt/snapshot paths run (not at server startup). A violation yields an `invalid_config` (or, for a missing key, `unavailable`) error.

| # | Variable(s) | Constraint |
|---|-------------|------------|
| ¹ | `FLOWPLANE_API_TLS_CERT`, `FLOWPLANE_API_TLS_KEY` | Set together or not at all. |
| ² | `FLOWPLANE_API_INSECURE` | Required `=true` when the API listener has no TLS material (D-008); otherwise startup fails. |
| ³ | `FLOWPLANE_XDS_TLS_CERT`, `FLOWPLANE_XDS_TLS_KEY`, `FLOWPLANE_XDS_TLS_CLIENT_CA` | All-or-none triad. |
| ⁴ | `FLOWPLANE_DEV_MODE`, `FLOWPLANE_OIDC_*` | Mutually exclusive: dev mode is rejected when a full OIDC issuer + audience pair is configured. |
| ⁵ | `FLOWPLANE_OIDC_ISSUER`, `FLOWPLANE_OIDC_AUDIENCE` | Set together or not at all (server). With neither set and dev mode off, authenticated endpoints answer `503`. |
| ⁶ | `FLOWPLANE_AGENT_TLS_CERT_PATH`, `_KEY_PATH`, `_CA_PATH` | All-or-none. |
| ⁷ | `FLOWPLANE_SECRET_ENCRYPTION_KEY` | Must decode to exactly 32 bytes (raw or base64). |
| ⁸ | `FLOWPLANE_SECRET_ENCRYPTION_KEY_ID` | 1..=128 characters, no control/null characters. |
| ⁹ | `FLOWPLANE_SECRET_ENCRYPTION_KEYS` | JSON object mapping `key_id` → key string; each value must decode to 32 bytes (raw or base64). |
| ¹⁰ | `FLOWPLANE_AGENT_CP_ENDPOINT` | Plaintext (`http`/non-`https`) allowed only for loopback hosts; non-loopback requires agent TLS (⁶). |
| ¹¹ | `FLOWPLANE_AGENT_POLL_INTERVAL_SECS` | Coerced to a minimum of 1 second. |
| ¹² | `FLOWPLANE_AGENT_QUEUE_CAP` | Clamped to `1..=16384`. |
| ¹³ | `FLOWPLANE_BOOTSTRAP_TOKEN`, `FLOWPLANE_BOOTSTRAP_TOKEN_FILE` | Required on the **first** boot of an uninitialized, non-dev instance: with no token (and without the `yes-this-is-local-only` opt-in) the server **fails closed** and does not start. Already-initialized instances ignore these. Supply the **same** token to every replica. See [How-to: bootstrap the first admin](../how-to/bootstrap-platform.md). |
| ¹⁴ | `FLOWPLANE_OIDC_CA_BUNDLE` | When set, the file must exist and parse as one or more PEM certificates; an unreadable, non-PEM, or zero-certificate bundle **fails server startup closed** (`invalid_config`) rather than silently falling back to bundled-roots-only trust. Trust is additive — bundled webpki roots are never disabled. |
| ¹⁵ | `FLOWPLANE_RLS_GRPC_URL` | Validated at server startup: `host:port` where host is an IP literal or DNS name and port is `1..=65535`. A malformed value **fails startup closed**. |
| ¹⁶ | `FLOWPLANE_RLS_RECONCILE_SECS` | Parsed as a positive integer and **clamped to `1..=60`**; zero/invalid/unset fall back to `60`. The knob may only *lower* the cadence (e.g. for tests) — it can never raise the reconcile interval past the documented 60 s convergence backstop. |
| ¹⁷ | `FLOWPLANE_DATAPLANE_TLS_CERT`, `_KEY`, `_CLIENT_CA` | All-or-none triad. With none set, the injected `rate_limit_cluster` dials the RLS in **plaintext h2c (dev only)**; in production set all three so the Envoy→RLS hop is mTLS. |
| ¹⁸ | `FLOWPLANE_RLS_GRPC_TLS_CERT`, `_KEY`, `_CLIENT_CA`, `FLOWPLANE_RLS_ALLOW_INSECURE_GRPC` | **Fail-closed, all-or-none** (server half of the Envoy→RLS mTLS hop; pair with ¹⁷ on the CP). A partial triad **fails startup**. With no triad, `flowplane-rls` starts only when the gRPC bind is a loopback literal (`127.0.0.0/8`, `::1`, `::ffff:127.0.0.0/8`) **and** `FLOWPLANE_RLS_ALLOW_INSECURE_GRPC=yes-this-is-local-only` is set; a non-loopback plaintext bind is a startup error. Unreadable/empty/malformed PEM fails startup naming the offending variable. Loopback is decided from the literal bind value — never DNS. |
| ¹⁹ | `FLOWPLANE_RLS_ADMIN_TLS_CERT`, `_KEY`, `FLOWPLANE_RLS_ADMIN_TOKEN[_FILE]`, `FLOWPLANE_RLS_ALLOW_INSECURE_ADMIN` | **Fail-closed, all-or-none**: the secure admin set is TLS pair **+** bearer credential together — TLS without a credential, a credential without TLS, or a partial cert/key pair each **fail startup** (a bearer must never travel plaintext; an admin endpoint must never accept unauthenticated policy replacement over TLS). `_TOKEN` and `_TOKEN_FILE` are mutually exclusive; an empty/whitespace-only token fails startup. With no admin security, `flowplane-rls` starts only on a loopback-literal admin bind with `FLOWPLANE_RLS_ALLOW_INSECURE_ADMIN=yes-this-is-local-only`. `/healthz` and `/readyz` stay open in all modes. |

## TOML config file keys (server)

Accepted keys when `FLOWPLANE_CONFIG` points at a TOML file (unknown keys rejected; env overrides file):

```
api_addr            xds_addr             database_url        db_max_connections
api_tls_cert        api_tls_key          xds_tls_cert        xds_tls_key
xds_tls_client_ca   api_insecure         dev_mode            oidc_issuer
oidc_audience       oidc_jwks_uri        oidc_ca_bundle      log_format
log_filter          otlp_endpoint        dev_token_path      rls_admin_url
rls_grpc_url        dataplane_tls_cert   dataplane_tls_key   dataplane_tls_client_ca
```

`FLOWPLANE_RLS_RECONCILE_SECS` is **env-only** (no TOML key).
