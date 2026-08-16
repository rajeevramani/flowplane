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
VER=3.1.1
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

Use `shasum -a 256 -c` instead of `sha256sum -c` on systems that do not provide `sha256sum`. The release archive also includes `bin/fp-agent` as a deprecated compatibility alias, but public operator commands use `flowplane-agent`.

When `v3.1.1` is published, its binary archives are Linux `amd64` and Linux `arm64`. Use a Linux host for the installed CLI, or run the published container image with an entrypoint override when your operator workstation is not Linux.

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
| CP to RLS admin | `FLOWPLANE_RLS_ADMIN_URL`, commonly `https://rls.example:8081` | CP -> RLS | Policy push. Production is HTTPS + bearer token: the RLS refuses a non-loopback admin bind without `FLOWPLANE_RLS_ADMIN_TLS_*` + `FLOWPLANE_RLS_ADMIN_TOKEN[_FILE]`, and the CP refuses to start if its token would cross a plaintext non-loopback URL. |
| Envoy to RLS gRPC | `FLOWPLANE_RLS_GRPC_URL`, commonly `rls.example:50051` | Envoy -> RLS | Production is mTLS-or-off, like xDS: the RLS refuses a non-loopback gRPC bind without its `FLOWPLANE_RLS_GRPC_TLS_*` server triad; pair it with the `FLOWPLANE_DATAPLANE_TLS_*` client triad on the CP. |
| Envoy listener traffic | listener config | clients -> Envoy | Request traffic never passes through the control plane. |
| Envoy admin | `127.0.0.1:9901` | dataplane-local only | Keep loopback-only. Product diagnostics go through `flowplane-agent`. |
| Agent health | `127.0.0.1:19902` | dataplane-local only | Use for local process health checks on the dataplane host. |

## Tenant TLS Material: Prefer SDS Over File Paths

Listener and cluster TLS can be sourced two ways, and they are mutually exclusive per field
(supplying both is rejected `400` at config time):

- **SDS secret references (preferred):** `tls_certificate_sds_secret_name` and
  `validation_context_sds_secret_name` on a listener, and `validation_context_sds_secret_name`
  on an upstream cluster. The named secret is a team-owned, write-only secret created through
  `flowplane secret create` (`POST /api/v1/teams/{team}/secrets`) and rotated with
  `flowplane secret rotate`. The material is delivered to Envoy over the authenticated
  xDS/SDS channel — it is never an inline value in the gateway spec and never a host file the
  control plane resolves.
- **File paths (explicit deployment integration):** `cert_chain_file` / `private_key_file` /
  `ca_cert_file` on a listener and `ca_cert_file` on a cluster, plus `access_logs.path`. These
  strings are passed through to Envoy verbatim and **resolved by Envoy on the dataplane host,
  under the Envoy process's OS identity** — the control plane does not (and on a remote host
  cannot) canonicalize or confine them.

Prefer SDS. File-backed TLS remains supported as an explicit deployment integration, but it is
a **shared-responsibility boundary**: because a file path resolves on the team-operated
dataplane host, whoever holds `listeners`/`clusters` write authority can direct the Envoy
process at any path its OS user can read or write. Where you use file-backed material, the
deployer owns the containing control — run Envoy with least privilege on each dataplane host:
a dedicated non-root user, minimal **read-only** secret mounts, and a restricted **writable**
directory for access logs. Do not grant gateway-config write authority to principals you would
not also trust with filesystem access on that dataplane host. (This boundary is a standing
project invariant; a control-plane path guard becomes mandatory only if a future deployment
model has Flowplane operate the dataplane filesystem or co-locate multiple tenants on one host.)

## Upgrade, Rollback, And Version Skew

The `3.1.2` to `3.1.3` credential-lifecycle transition is stop-the-world for control-plane
instances. Mixed-version or rolling control-plane operation is unsupported: every old control
plane must stop before a `3.1.3` binary applies migrations. Keep the CLI, control plane,
`flowplane-agent`, and `flowplane-rls` on one verified release artifact set.

Upgrade:

1. Download and verify the complete `3.1.3` artifact set while `3.1.2` remains active.
2. Run `flowplane db preflight`. A `blocked` report names only stable `FP_*` codes,
   dataplane UUIDs, and certificate UUIDs; resolve every blocker before continuing.
3. Stop **all** old control-plane instances and verify none can drain or migrate the database.
4. Back up PostgreSQL plus active/retired secret-encryption keys, then verify the backup by
   restoring it into an isolated database before migration.
5. Start exactly one `3.1.3` control plane or run `flowplane db migrate`; wait for readiness.
6. Verify an existing legacy dataplane reconnects through exact certificate binding before
   reopening API/xDS traffic or adding replicas.
7. Upgrade the CLI, `flowplane-agent`, and `flowplane-rls`; regenerate bootstrap only when CP xDS
   host, port, mode, or certificate paths changed.
8. Verify `/healthz`, `/readyz`, `flowplane stats overview`, `flowplane ops xds status`, agent
   `/healthz`, and RLS `/healthz`/`/readyz`.

Historical audit rows and outbox payloads keep their original serial text. Migration canonicalizes
the live certificate registry only; correlate historical evidence by certificate UUID and
request/event identity, not by textual serial equality.

Rollback:

1. Keep the verified prior release archive/image and the pre-migration backup available.
2. Before migration, roll back CP/agent/RLS binaries together normally.
3. After migration, the prior binary rejects the database's newer sqlx migration and cannot be
   used as a binary-only rollback.
4. The pre-migration backup may be restored only while the upgraded system has accepted **no**
   certificate issue/registration, legacy fingerprint pin, revocation, dataplane retirement, or
   same-name recreation. Verify that condition before starting the old control plane.
5. After the first such lifecycle write, recovery is roll-forward only. Restoring the old backup
   could resurrect revoked credentials, un-retire identities, or erase replacement history. Do not
   reopen API/xDS traffic from that backup without a separately approved incident plan that
   re-establishes trust and every post-backup revocation.
6. Existing dataplanes keep serving their last-applied Envoy config during a CP outage; new
   dataplanes cannot join until the upgraded CP is ready.

Before authorizing a pre-migration backup restore, run this read-only check against the upgraded
database. **Any returned row means restore is forbidden and recovery is roll-forward-only.** The
query emits action names and counts only; it does not expose certificate identity or material.

```bash
psql "$FLOWPLANE_DATABASE_URL" -v ON_ERROR_STOP=1 <<'SQL'
WITH migration_cutoff AS (
  SELECT installed_on
  FROM _sqlx_migrations
  WHERE version = 34
), write_counts AS (
  SELECT action, count(*) AS writes_after_migration
  FROM audit_log, migration_cutoff
  WHERE occurred_at >= migration_cutoff.installed_on
    AND outcome = 'success'
    AND action IN (
      'dataplane.create',
      'dataplane.retire',
      'proxy-certificate.issue',
      'proxy-certificate.register',
      'proxy-certificate.revoke',
      'proxy_certificate.fingerprint_pin'
    )
  GROUP BY action
)
SELECT action, writes_after_migration FROM write_counts
UNION ALL
SELECT 'FP_MIGRATION_0034_MISSING', 1
WHERE NOT EXISTS (SELECT 1 FROM migration_cutoff)
ORDER BY action;
SQL
```

This is a necessary eligibility check, not proof that a restore is safe: also prove that API/xDS
traffic stayed closed since migration and reconcile deployment/incident logs. Missing audit
evidence or an uncertain traffic boundary requires roll-forward recovery.

## Issuer CA compatibility and upgrade

Dataplane certificate issuance requires the configured issuer certificate to be currently valid,
match its private key, contain `CA:TRUE`, `keyCertSign`, and a Subject Key Identifier, and not
restrict extended key usage away from `clientAuth`. A standards-complete intermediate certificate
is supported when it is deliberately configured as the explicit trust anchor.

Inspect existing material without printing private key bytes:

```bash
openssl x509 -in issuer-ca.pem -noout -dates -ext basicConstraints -ext keyUsage -ext subjectKeyIdentifier
openssl x509 -in issuer-ca.pem -noout -pubkey | sha256sum
openssl pkey -in issuer-ca.key -pubout | sha256sum
```

The two public-key hashes must match. If the certificate profile is incomplete, reissue a
standards-complete CA certificate. Reusing the same CA private key may be possible under your CA
policy; that is a **CA-certificate reissue**, not a CA-key rotation. If the key changes, treat it as
a full CA rotation.

Roll out in this order: first redistribute the corrected **public CA certificate** to every issuer
and verifier location, including `FLOWPLANE_CERT_ISSUER_CA_CERT_PATH`,
`FLOWPLANE_XDS_TLS_CLIENT_CA`, dataplane/RLS trust bundles, and provider secret stores; then restart
the processes that load those files; only then issue replacement leaves. The product upgrade does
not rewrite trust stores, reissue leaves, terminate existing mTLS sessions, or rotate CA keys.

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
| Rate Limit Service (CP side) | `FLOWPLANE_RLS_GRPC_URL`, `FLOWPLANE_RLS_ADMIN_URL` (https in production), `FLOWPLANE_RLS_ADMIN_TOKEN` or `FLOWPLANE_RLS_ADMIN_TOKEN_FILE`, `FLOWPLANE_RLS_ADMIN_TLS_CA` (private-CA trust for the RLS admin cert), `FLOWPLANE_RLS_RECONCILE_SECS`, plus the `FLOWPLANE_DATAPLANE_TLS_*` client triad for the Envoy-to-RLS mTLS hop |

`flowplane-rls` process (fail-closed: a non-loopback listener refuses to start without its
security material; loopback plaintext requires the explicit `yes-this-is-local-only`
acknowledgements — dev only):

| Area | Variables |
| --- | --- |
| Listeners | `FLOWPLANE_RLS_GRPC_LISTEN`, `FLOWPLANE_RLS_ADMIN_LISTEN` (defaults `127.0.0.1:50051` / `127.0.0.1:8081`) |
| gRPC mTLS (server half) | `FLOWPLANE_RLS_GRPC_TLS_CERT`, `FLOWPLANE_RLS_GRPC_TLS_KEY`, `FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA` (all-or-none; Envoy client certs required at the TLS layer) |
| Admin HTTPS + auth | `FLOWPLANE_RLS_ADMIN_TLS_CERT`, `FLOWPLANE_RLS_ADMIN_TLS_KEY`, `FLOWPLANE_RLS_ADMIN_TOKEN` or `FLOWPLANE_RLS_ADMIN_TOKEN_FILE` (TLS pair + token are all-or-none; same token value as the CP side) |
| Dev-only escape hatches | `FLOWPLANE_RLS_ALLOW_INSECURE_GRPC`, `FLOWPLANE_RLS_ALLOW_INSECURE_ADMIN` (RLS, loopback binds only), `FLOWPLANE_RLS_ALLOW_INSECURE_ADMIN_PUSH` (CP, loopback URL only) |
| Dataplane cert issuer | `FLOWPLANE_CERT_ISSUER_CA_CERT_PATH`, `FLOWPLANE_CERT_ISSUER_CA_KEY_PATH`, `FLOWPLANE_CERT_ISSUER_TRUST_DOMAIN` (issuer must match its key and contain current `CA:TRUE`, `keyCertSign`, and Subject Key Identifier metadata; explicit intermediate trust anchors are supported) |
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
| CP unavailable | CP `/healthz` fails, API unavailable, agent `/healthz` returns `503` while diagnostics acknowledgments are stale | Check CP process logs, TLS material, listener bind, bootstrap token on first boot, OIDC config, and DB reachability, then restore the CP; do not restart a surviving agent. The surviving agent reconnects automatically after the control plane is restored. Envoy continues serving its last-good configuration, and agent readiness returns only after the control plane accepts and commits a diagnostics report. |
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
| RLS unreachable | Route returns 5xx/429 behavior does not match policy, RLS health fails, CP RLS reconcile errors | Check CP `FLOWPLANE_RLS_ADMIN_URL`, Envoy-facing `FLOWPLANE_RLS_GRPC_URL`, firewall rules, and both halves of the Envoy-to-RLS mTLS material (`FLOWPLANE_DATAPLANE_TLS_*` on the CP vs `FLOWPLANE_RLS_GRPC_TLS_*` on the RLS — the CAs must cross-trust). |
| RLS policy push rejected | CP log shows `rls policy reconcile failed … HTTP 401` repeatedly; policies never reach the RLS (fresh RLS enforces nothing) | Token mismatch between CP `FLOWPLANE_RLS_ADMIN_TOKEN[_FILE]` and RLS `FLOWPLANE_RLS_ADMIN_TOKEN[_FILE]` — set the same value on both sides. A TLS trust failure instead shows a connection/certificate error: set CP `FLOWPLANE_RLS_ADMIN_TLS_CA` to the CA that signed the RLS admin cert. The RLS keeps serving its last good policy set; the loop retries every reconcile tick. |
| RLS refuses to start | Startup error naming a `FLOWPLANE_RLS_*` variable | Fail-closed by design: a non-loopback bind needs its full TLS material (gRPC triad / admin pair + token); partial material never starts. For local dev bind loopback and set the explicit `…_ALLOW_INSECURE_*=yes-this-is-local-only` acknowledgements. |

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

For a `3.1.2` to `3.1.3` rollback rehearsal, this restore is eligible only before the upgraded
system performs any credential/dataplane lifecycle write listed in the rollback section. After
that cutoff, use roll-forward recovery instead.

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
