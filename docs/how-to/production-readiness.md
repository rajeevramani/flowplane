# Production Readiness

> Audience: operators, platform-engineers · Status: stable

This is the operator entry point for a production-shaped Flowplane deployment. It describes the control plane, dataplane, identity, bootstrap, the Rate Limit Service (`flowplane-rls`), and operating checks using public docs only.

## Evidence

- Secret KEK rotation: [`secret-kek-rotation.md`](secret-kek-rotation.md)
- OIDC setup and first-admin subject discovery: [`configure-oidc-provider.md`](configure-oidc-provider.md)
- First platform admin bootstrap: [`bootstrap-platform.md`](bootstrap-platform.md)
- Platform evaluation sequence: [`evaluate-platform.md`](evaluate-platform.md)
- Observability baseline: [`../reference/observability-alerts.md`](../reference/observability-alerts.md)
- Configuration reference: [`../reference/configuration.md`](../reference/configuration.md)
- **Dataplane egress / SSRF posture (required)**: [`dataplane-egress-security.md`](dataplane-egress-security.md)

## Deployment Shape

Deploy the control plane and dataplane bundle separately.

Install the published operator binaries on the control-plane host, operator workstation, and dataplane host. Pick the archive for the host architecture:

```bash
VER=2.1.1
ARCH=linux-amd64   # or linux-arm64
BASE="https://github.com/rajeevramani/flowplane/releases/download/v${VER}"

curl -fLO "${BASE}/flowplane-${VER}-${ARCH}.tar.gz"
curl -fLO "${BASE}/SHA256SUMS"
grep "flowplane-${VER}-${ARCH}.tar.gz" SHA256SUMS | sha256sum -c -
tar -xzf "flowplane-${VER}-${ARCH}.tar.gz"

sudo install -m 0755 "flowplane-${VER}-${ARCH}/bin/flowplane" /usr/local/bin/flowplane
sudo install -m 0755 "flowplane-${VER}-${ARCH}/bin/flowplane-agent" /usr/local/bin/flowplane-agent
sudo install -m 0755 "flowplane-${VER}-${ARCH}/bin/flowplane-rls" /usr/local/bin/flowplane-rls
```

Use `shasum -a 256 -c` instead of `sha256sum -c` on systems that do not provide `sha256sum`. The release archive also includes `bin/fp-agent` as a one-release compatibility alias, but public operator commands use `flowplane-agent`.

The published `2.1.1` binary archives are Linux `amd64` and Linux `arm64`. Use a Linux host for the installed CLI, or run the published container image with an entrypoint override when your operator workstation is not Linux.

Control plane:

```bash
export FLOWPLANE_DATABASE_URL=postgres://user:pass@postgres/flowplane
export FLOWPLANE_SECRET_ENCRYPTION_KEY=<32-byte-or-base64-key>
export FLOWPLANE_BOOTSTRAP_TOKEN_FILE=/run/secrets/flowplane-bootstrap-token
export FLOWPLANE_API_TLS_CERT=/etc/flowplane/tls/api.crt
export FLOWPLANE_API_TLS_KEY=/etc/flowplane/tls/api.key
export FLOWPLANE_XDS_TLS_CERT=/etc/flowplane/tls/xds.crt
export FLOWPLANE_XDS_TLS_KEY=/etc/flowplane/tls/xds.key
export FLOWPLANE_XDS_TLS_CLIENT_CA=/etc/flowplane/tls/dp-ca.crt
export FLOWPLANE_OIDC_ISSUER=https://issuer.example
export FLOWPLANE_OIDC_AUDIENCE=flowplane

flowplane db migrate
flowplane serve
```

On the first boot of an uninitialized non-dev control plane, provide a high-entropy bootstrap token with `FLOWPLANE_BOOTSTRAP_TOKEN_FILE` (preferred) or `FLOWPLANE_BOOTSTRAP_TOKEN`. The server stores only a hash, does not log the value, and fails closed if no token is supplied. Use [Bootstrap the first platform admin](bootstrap-platform.md) to consume it once.

Production authentication requires a real OIDC issuer/audience pair. `FLOWPLANE_DEV_MODE` and `FLOWPLANE_ALLOW_LOGGED_BOOTSTRAP_TOKEN` are local-only escape hatches and must not be set in production.

Dataplane:

```bash
flowplane --out /etc/flowplane/envoy/envoy.yaml \
  dataplane bootstrap edge-1 --team <team> --mode mtls \
  --xds-host cp.example --xds-port 18000 \
  --cert-path /etc/flowplane/dp/client.crt \
  --key-path /etc/flowplane/dp/client.key \
  --ca-path /etc/flowplane/dp/server-ca.crt
```

Run Envoy with that bootstrap and run `flowplane-agent` beside it in the dataplane network. The dataplane dials the control plane over xDS/diagnostics; the control plane must not dial Envoy admin as a product path. Envoy admin stays loopback-only and is a manual diagnostic fallback. The canonical certificate issuance and agent flags are in [Register a dataplane and connect its agent over mTLS](register-dataplane-mtls.md).

## Ports And Network Paths

| Path | Default | Direction | Notes |
| --- | --- | --- | --- |
| Operator/API traffic to CP | `FLOWPLANE_API_ADDR`, usually `:8080` behind TLS/load balancer | operators/API teams -> CP | Terminate public TLS before or at the CP API. Do not expose plaintext production API. |
| Dataplane xDS/diagnostics to CP | `FLOWPLANE_XDS_ADDR`, default `:18000` | Envoy/agent -> CP | Production is mTLS-or-off. Non-loopback deployments must use the xDS TLS triad. |
| CP to PostgreSQL | database URL | CP -> PostgreSQL | Database must not be directly internet-addressable. |
| CP to RLS admin | `FLOWPLANE_RLS_ADMIN_URL`, commonly `http://rls.example:8081` | CP -> RLS | Used by the RLS reconcile worker to push policy. |
| Envoy to RLS gRPC | `FLOWPLANE_RLS_GRPC_URL`, commonly `rls.example:50051` | Envoy -> RLS | Use the dataplane TLS triad for production Envoy-to-RLS mTLS when enabled. |
| Envoy listener traffic | listener config | clients -> Envoy | Request traffic never passes through the control plane. |
| Envoy admin | `127.0.0.1:9901` | dataplane-local only | Keep loopback-only. Product diagnostics go through `flowplane-agent`. |
| Agent health | `127.0.0.1:19902` | dataplane-local only | Use for local process health checks on the dataplane host. |

## Upgrade, Rollback, And Version Skew

For the `2.1.1` split-node release path, run the control plane, CLI, `flowplane-agent`, and `flowplane-rls` from the same `2.1.1` artifact set. Short-lived skew during a rolling restart is acceptable for replacing one process at a time, but do not plan a long-lived mixed-version CP/agent/RLS deployment until a later compatibility policy says so.

Upgrade:

1. Back up PostgreSQL and active/retired secret-encryption keys.
2. Download and verify the new release archive and image digests.
3. Upgrade the control plane and run `flowplane db migrate`.
4. Upgrade `flowplane-agent` and `flowplane-rls` on dataplane/RLS hosts.
5. Regenerate dataplane bootstrap only when CP xDS host, port, mode, or cert paths changed.
6. Verify `/healthz`, `/readyz`, `flowplane stats overview`, `flowplane ops xds status`, agent `/healthz`, and RLS `/healthz`/`/readyz`.

Rollback:

1. Keep the prior release archive/image available before upgrading.
2. If no database migration ran, roll back CP/agent/RLS binaries together and restart.
3. If a database migration ran and rollback is required, restore the matching database backup and KEK material before starting the old CP.
4. Existing dataplanes keep serving their last-applied Envoy config during a CP restart; new dataplanes cannot join until the CP is back.
5. If xDS host, port, mode, or cert paths changed during the failed upgrade, regenerate bootstrap before reconnecting the dataplane.

## Configuration Reference

Server process:

| Area | Variables |
| --- | --- |
| Config file | `FLOWPLANE_CONFIG` |
| API bind/TLS | `FLOWPLANE_API_ADDR`, `FLOWPLANE_API_TLS_CERT`, `FLOWPLANE_API_TLS_KEY`, `FLOWPLANE_API_INSECURE` |
| xDS bind/mTLS | `FLOWPLANE_XDS_ADDR`, `FLOWPLANE_XDS_TLS_CERT`, `FLOWPLANE_XDS_TLS_KEY`, `FLOWPLANE_XDS_TLS_CLIENT_CA` |
| Database | `FLOWPLANE_DATABASE_URL` or `DATABASE_URL`, `FLOWPLANE_DB_MAX_CONNECTIONS` |
| Secret encryption | `FLOWPLANE_SECRET_ENCRYPTION_KEY`, `FLOWPLANE_SECRET_ENCRYPTION_KEY_ID`, `FLOWPLANE_SECRET_ENCRYPTION_KEYS` |
| Auth | `FLOWPLANE_OIDC_ISSUER`, `FLOWPLANE_OIDC_AUDIENCE`, `FLOWPLANE_OIDC_JWKS_URI`, `FLOWPLANE_OIDC_CA_BUNDLE` (operator CA for an IdP behind a TLS-intercepting proxy; additive trust, fail-closed at startup) |
| Bootstrap | `FLOWPLANE_BOOTSTRAP_TOKEN_FILE` (preferred) or `FLOWPLANE_BOOTSTRAP_TOKEN` for first boot only |
| Dev only | `FLOWPLANE_DEV_MODE`, `FLOWPLANE_DEV_MODE_ACK` |
| Observability | `FLOWPLANE_LOG`, `FLOWPLANE_LOG_FORMAT`, `FLOWPLANE_OTLP_ENDPOINT` |
| MCP | `FLOWPLANE_MCP_ALLOWED_ORIGINS` |
| Throttling/discovery | `FLOWPLANE_TENANT_WRITE_LIMIT_PER_MIN`, `FLOWPLANE_DISCOVERY_ALLOWED_DESTINATIONS` |
| Rate Limit Service (`flowplane-rls`) | `FLOWPLANE_RLS_GRPC_URL`, `FLOWPLANE_RLS_ADMIN_URL`, `FLOWPLANE_RLS_RECONCILE_SECS`; in production also set the `FLOWPLANE_DATAPLANE_TLS_*` triad for the Envoy-to-RLS hop |
| Dataplane cert issuer | `FLOWPLANE_CERT_ISSUER_CA_CERT_PATH`, `FLOWPLANE_CERT_ISSUER_CA_KEY_PATH`, `FLOWPLANE_CERT_ISSUER_TRUST_DOMAIN` |
| Upstream TLS trust | `FLOWPLANE_UPSTREAM_CA_BUNDLE` (CA bundle path **in the Envoy/dataplane container** used to verify materialized TLS upstreams; default `/etc/ssl/certs/ca-certificates.crt`) |

> **Upstream certificate verification (verify-by-default).** TLS upstreams that Flowplane materializes (AI providers, `flowplane expose https://…`, route-generation) verify the upstream server certificate against `FLOWPLANE_UPSTREAM_CA_BUNDLE`. A cluster may instead name an SDS validation secret (`validation_context_sds_secret_name`) or a per-cluster CA file (`ca_cert_file`). Verification can only be disabled per cluster by explicitly setting `insecure_skip_verify: true` — never the silent default (issue #125). The bundle path is resolved by **Envoy**, so the dataplane image must ship a CA bundle at that path (the default exists on Debian/Ubuntu via the `ca-certificates` package); otherwise Envoy rejects the cluster. The control plane cannot check the dataplane filesystem, so verify this when building/operating the dataplane image.

CLI client:

| Area | Variables |
| --- | --- |
| Target/context | `FLOWPLANE_SERVER`, `FLOWPLANE_ORG`, `FLOWPLANE_TEAM`, `FLOWPLANE_CONFIG` |
| Auth | `FLOWPLANE_TOKEN`, `FLOWPLANE_OIDC_ISSUER`, `FLOWPLANE_OIDC_CLIENT_ID`, `FLOWPLANE_OIDC_SCOPE`, `FLOWPLANE_OIDC_CALLBACK_URL` |

Dataplane agent:

| Area | Variables |
| --- | --- |
| Envoy/CP | `FLOWPLANE_AGENT_ENVOY_ADMIN_URL`, `FLOWPLANE_AGENT_CP_ENDPOINT`, `FLOWPLANE_AGENT_DATAPLANE_ID` |
| TLS | `FLOWPLANE_AGENT_TLS_CERT_PATH`, `FLOWPLANE_AGENT_TLS_KEY_PATH`, `FLOWPLANE_AGENT_TLS_CA_PATH`, `FLOWPLANE_AGENT_TLS_SERVER_NAME` |
| Runtime | `FLOWPLANE_AGENT_POLL_INTERVAL_SECS`, `FLOWPLANE_AGENT_QUEUE_CAP`, `FLOWPLANE_AGENT_HEALTH_BIND_ADDR` |

Packaging:

| Area | Variables |
| --- | --- |
| Artifact identity | `FLOWPLANE_RELEASE_TARGET`, `FLOWPLANE_RELEASE_VERSION`, `FLOWPLANE_RELEASE_HOST`, `FLOWPLANE_IMAGE_TAG` |
| Package outputs | `FLOWPLANE_PACKAGE_IMAGE`, `FLOWPLANE_PACKAGE_DATAPLANE` |
| Dataplane package | `FLOWPLANE_PACKAGE_TEAM`, `FLOWPLANE_PACKAGE_DATAPLANE_NAME`, `FLOWPLANE_PACKAGE_DATAPLANE_MODE`, `FLOWPLANE_PACKAGE_XDS_HOST`, `FLOWPLANE_PACKAGE_XDS_PORT`, `FLOWPLANE_PACKAGE_ADMIN_PORT`, `FLOWPLANE_PACKAGE_CA_PATH`, `FLOWPLANE_PACKAGE_CERT_PATH`, `FLOWPLANE_PACKAGE_KEY_PATH` |

AI providers, routes, budgets, and usage are runtime product config through the API/CLI, not deployment environment variables.

## Runbook

| Symptom | Signals | Action |
| --- | --- | --- |
| CP unavailable | `/healthz` fails, API unavailable | Check process logs, TLS material, listener bind, bootstrap token on first boot, OIDC config, and DB reachability. Restart CP. Existing DPs keep last-applied config. |
| DB degraded/down | `/readyz` fails, `fp_db_pool_*` saturation, DB connection errors | Restore DB connectivity. Expect REST mutations to fail while DB is down. Run `flowplane db migrate` after restore before serving traffic. |
| xDS NACK/quarantine | `fp_xds_nacks_total`, `fp_xds_quarantined_resources_total`, translation failure counters | Inspect the rejected resource in CP logs/audit. Fix the persisted CP resource and republish; do not patch Envoy admin directly. |
| Dataplane disconnect churn | `fp_xds_ads_streams_closed_total` rising faster than opens | Check DP network path to CP xDS, mTLS cert validity, and agent/Envoy process health. |
| Outbox lag/failures | `fp_outbox_pending_events`, `fp_outbox_oldest_pending_age_seconds`, `fp_outbox_handler_failures_total` | Check DB health and CP logs. Restart CP if the consumer is wedged; outbox redelivery is expected after recovery. |
| Auth spike | `fp_authn_failures_total`, `fp_authz_denied_total`, audit rows | For authn, check IdP/JWKS/audience/token expiry. For authz, check grants/team context and suspicious probing. |
| AI budget exhaustion | `fp_ai_budget_threshold_crossings_total{mode="enforcing",result="exhausted"}` | Compare expected usage to configured budget; raise budget or reduce traffic. |
| Capture drops | `fp_capture_dropped_total` | Check capture source health and configured discovery/capture constraints. |
| Release package validation | GitHub Release assets, image digests, `SHA256SUMS` | Verify published artifacts before deployment; rebuild only through the release process if any artifact is missing or inconsistent. |
| Split-node TLS failure | Agent/RLS logs show TLS name, CA, or client-certificate errors | Check CP xDS server cert SAN vs `FLOWPLANE_AGENT_TLS_SERVER_NAME`, server-trust CA, issued client cert/key, and CP `FLOWPLANE_XDS_TLS_CLIENT_CA`. |
| Remote bootstrap still points at localhost | Envoy bootstrap contains `127.0.0.1` or `localhost` for `xds_cluster` | Regenerate bootstrap with `--xds-host <cp-xds-host>` and verify the generated file before starting Envoy. |
| RLS unreachable | Route returns 5xx/429 behavior does not match policy, RLS health fails, CP RLS reconcile errors | Check CP `FLOWPLANE_RLS_ADMIN_URL`, Envoy-facing `FLOWPLANE_RLS_GRPC_URL`, firewall rules, and dataplane TLS settings for the Envoy-to-RLS hop. |

## Backup And Restore Drill

Back up together:

1. PostgreSQL database.
2. Active `FLOWPLANE_SECRET_ENCRYPTION_KEY`.
3. `FLOWPLANE_SECRET_ENCRYPTION_KEY_ID`.
4. Retired-key JSON in `FLOWPLANE_SECRET_ENCRYPTION_KEYS`.
5. CP xDS/API TLS files and dataplane CA material.

A database restore without the matching KEK material leaves encrypted secret rows undecryptable. Keep KEK escrow and rotation overlap aligned with [`secret-kek-rotation.md`](secret-kek-rotation.md).

Restore:

```bash
createdb flowplane_restored
pg_restore --clean --if-exists --dbname=flowplane_restored flowplane.dump

FLOWPLANE_DATABASE_URL=postgres://user:pass@postgres/flowplane_restored \
FLOWPLANE_SECRET_ENCRYPTION_KEY=<restored-active-key> \
FLOWPLANE_SECRET_ENCRYPTION_KEY_ID=<restored-active-key-id> \
FLOWPLANE_SECRET_ENCRYPTION_KEYS='<restored-retired-key-json>' \
FLOWPLANE_API_INSECURE=true \
flowplane db migrate
```

`FLOWPLANE_API_INSECURE=true` is acceptable here only for a local restore drill that does not
serve production traffic. In a production restore, provide the API TLS pair instead
(`FLOWPLANE_API_TLS_CERT` and `FLOWPLANE_API_TLS_KEY`) and omit the plaintext opt-in.

Post-restore pass signals:

```bash
FLOWPLANE_DATABASE_URL=postgres://user:pass@postgres/flowplane_restored \
FLOWPLANE_SECRET_ENCRYPTION_KEY=<restored-active-key> \
FLOWPLANE_SECRET_ENCRYPTION_KEY_ID=<restored-active-key-id> \
FLOWPLANE_SECRET_ENCRYPTION_KEYS='<restored-retired-key-json>' \
FLOWPLANE_API_TLS_CERT=/etc/flowplane/tls/api.crt \
FLOWPLANE_API_TLS_KEY=/etc/flowplane/tls/api.key \
flowplane serve
curl -fsS https://cp.example/healthz
curl -fsS https://cp.example/readyz
flowplane team list
flowplane dataplane list --team <team>
flowplane mcp status --team <team>
```

Then reconnect one non-production dataplane and confirm ADS opens without NACK/quarantine alerts.

## CLI Workflow

```bash
flowplane auth login --device-code --issuer https://issuer.example --client-id flowplane-cli
flowplane config set-context prod --server https://cp.example --org <org> --team <team>

flowplane org list
flowplane team list

flowplane learn discover start catalog-capture --team <team> \
  --upstream https://upstream.example --listener-port 8443
flowplane learn discover generate-spec <session-id> --team <team>

flowplane api create catalog --from-openapi openapi.json --team <team>
flowplane api spec publish catalog 1 --team <team> --reason "operator reviewed"
flowplane route generate --from-spec <api-spec-id> --listener-port 8443 --team <team>
flowplane route apply <plan-id> --team <team>

flowplane dataplane bootstrap edge-1 --team <team> --mode mtls \
  --xds-host cp.example --xds-port 18000 \
  --cert-path /etc/flowplane/dp/client.crt \
  --key-path /etc/flowplane/dp/client.key \
  --ca-path /etc/flowplane/dp/server-ca.crt

flowplane mcp status --team <team>
flowplane mcp connections --team <team>
flowplane mcp enable --api api_get-catalog --team <team>
```

For deployment-specific details, use the relevant public runbook such as [AWS secure deployment](aws-secure-deployment.md). Keep release evidence separate from day-to-day operator runbooks.
