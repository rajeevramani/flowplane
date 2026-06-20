# Configuration & environment variables

> Audience: operators, platform-engineers · Status: stable

Every `FLOWPLANE_*` variable read by the control plane, the dataplane agent, and the
CLI. Each variable appears once; the **Component** column says which process reads it.

**Precedence**

| Surface | Resolution order |
|---------|------------------|
| Server (`flowplane serve`) | env var > TOML file (`FLOWPLANE_CONFIG`) > built-in default |
| CLI | flag > env var > CLI config file |

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
| `FLOWPLANE_OTLP_ENDPOINT` | server | — | no | OTLP trace export endpoint; unset disables export. |
| `FLOWPLANE_DEV_MODE` | server | `false` | no ⁴ | In-process OIDC issuer + seeded resources; needs the `dev-oidc` build feature. |
| `FLOWPLANE_DEV_MODE_ACK` | server | — | no ⁴ | In release builds, dev mode requires this to equal `yes-this-is-not-production`. |
| `FLOWPLANE_OIDC_AUDIENCE` | server | — | no ⁵ | Expected JWT `aud` claim. |
| `FLOWPLANE_OIDC_JWKS_URI` | server | — | no | JWKS endpoint override (optional even with OIDC set). |
| `FLOWPLANE_TENANT_WRITE_LIMIT_PER_MIN` | server | `120` | no | Per-tenant mutating-request budget per minute; must be ≥ 1. |
| `FLOWPLANE_SECRET_ENCRYPTION_KEY` | server | — | for secrets | Active key-encryption key; 32 raw bytes or base64. ⁷ |
| `FLOWPLANE_SECRET_ENCRYPTION_KEY_ID` | server | `default` | no | Identifier for the active KEK, used for rotation. ⁸ |
| `FLOWPLANE_SECRET_ENCRYPTION_KEYS` | server | — | no | Retired-key keyring so encrypted secrets stay decryptable during rotation. ⁹ |
| `FLOWPLANE_CERT_ISSUER_CA_CERT_PATH` | server | — | for issuance | Issuing CA certificate PEM path. |
| `FLOWPLANE_CERT_ISSUER_CA_KEY_PATH` | server | — | for issuance | Issuing CA private key PEM path. |
| `FLOWPLANE_CERT_ISSUER_TRUST_DOMAIN` | server | `flowplane.local` | no | SPIFFE trust domain for issued dataplane identities. |
| `FLOWPLANE_DISCOVERY_ALLOWED_DESTINATIONS` | server | — | no | Comma-separated `host:port` allowlist for traffic discovery. |
| `FLOWPLANE_MCP_ALLOWED_ORIGINS` | server | `http://localhost,http://127.0.0.1,http://[::1]` | no | Comma-separated allowed `Origin` values for the MCP endpoint. |
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
| `FLOWPLANE_TOKEN` | cli | — | no | Bearer token for API requests. |
| `FLOWPLANE_ORG` | cli | — | no | Active organization (name or UUID). |
| `FLOWPLANE_TEAM` | cli | — | no | Active team. |
| `FLOWPLANE_OIDC_CLIENT_ID` | cli | — | no | OIDC client id for CLI login. |
| `FLOWPLANE_OIDC_SCOPE` | cli | `openid email profile` | no | OIDC scopes requested at CLI login. |
| `FLOWPLANE_OIDC_CALLBACK_URL` | cli | `http://127.0.0.1:8976/callback` | no | OAuth callback URL for CLI login. |
| `FLOWPLANE_CONFIG` | server, cli | CLI: `~/.flowplane/config.toml` | no | Server: path to the TOML config file (no file if unset). CLI: path to the CLI config file. |
| `FLOWPLANE_OIDC_ISSUER` | server, cli | — | no ⁵ | Server: OIDC issuer URL for auth. CLI: issuer for CLI login. |

## Constraints

Enforcement timing varies: rows ¹–⁵ are validated at **server startup** (`flowplane serve`);
rows ⁶, ¹⁰–¹² at **agent startup** (`fp-agent`); rows ⁷–⁹ at **use time** when the secret
encrypt/decrypt/snapshot paths run (not at server startup). A violation yields an
`invalid_config` (or, for a missing key, `unavailable`) error.

| # | Variable(s) | Constraint |
|---|-------------|------------|
| ¹ | `FLOWPLANE_API_TLS_CERT`, `FLOWPLANE_API_TLS_KEY` | Set together or not at all. |
| ² | `FLOWPLANE_API_INSECURE` | Required `=true` when the API listener has no TLS material (D-008); otherwise startup fails. |
| ³ | `FLOWPLANE_XDS_TLS_CERT`, `FLOWPLANE_XDS_TLS_KEY`, `FLOWPLANE_XDS_TLS_CLIENT_CA` | All-or-none triad. |
| ⁴ | `FLOWPLANE_DEV_MODE`, `FLOWPLANE_OIDC_*` | Mutually exclusive. |
| ⁵ | `FLOWPLANE_OIDC_ISSUER`, `FLOWPLANE_OIDC_AUDIENCE` | Set together or not at all (server). With neither set and dev mode off, authenticated endpoints answer `503`. |
| ⁶ | `FLOWPLANE_AGENT_TLS_CERT_PATH`, `_KEY_PATH`, `_CA_PATH` | All-or-none. |
| ⁷ | `FLOWPLANE_SECRET_ENCRYPTION_KEY` | Must decode to exactly 32 bytes (raw or base64). |
| ⁸ | `FLOWPLANE_SECRET_ENCRYPTION_KEY_ID` | 1..=128 characters, no control/null characters. |
| ⁹ | `FLOWPLANE_SECRET_ENCRYPTION_KEYS` | JSON object mapping `key_id` → key string; each value must decode to 32 bytes (raw or base64). |
| ¹⁰ | `FLOWPLANE_AGENT_CP_ENDPOINT` | Plaintext (`http`/non-`https`) allowed only for loopback hosts; non-loopback requires agent TLS (⁶). |
| ¹¹ | `FLOWPLANE_AGENT_POLL_INTERVAL_SECS` | Coerced to a minimum of 1 second. |
| ¹² | `FLOWPLANE_AGENT_QUEUE_CAP` | Clamped to `1..=16384`. |

## TOML config file keys (server)

Accepted keys when `FLOWPLANE_CONFIG` points at a TOML file (unknown keys rejected; env overrides file):

```
api_addr            xds_addr             database_url        db_max_connections
api_tls_cert        api_tls_key          xds_tls_cert        xds_tls_key
xds_tls_client_ca   api_insecure         dev_mode            oidc_issuer
oidc_audience       oidc_jwks_uri        log_format          log_filter
otlp_endpoint
```
