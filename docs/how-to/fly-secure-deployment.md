# Deploy a secure Flowplane control plane on Fly.io with Tailscale dataplanes

> Audience: operators, platform-engineers · Status: draft — local checks only; fresh live walkthrough pending

Deploy the control plane (CP), PostgreSQL and rate-limit service (RLS) on Fly.io, with Envoy and `flowplane-agent` on isolated Linux dataplane hosts. Fly and Tailscale provide delivery and connectivity, not tenant authority. OIDC access-token authentication establishes user identity; Flowplane memberships/grants authorize tenant access; registered client certificates authorize dataplanes. This page is standalone: private design/spec artifacts are not required inputs. Native Flowplane API TLS passes through Fly raw TCP; xDS and RLS require strict mTLS behind tailnet Services. Never disable TLS certificate verification to complete a step.

This runbook is checked against source revision `ada18f1390bf4a607d5d1bfe0324aeae529e8bb3` (`v3.1.4`). It is a post-release documentation candidate, not documentation shipped in that tag. Earlier live qualification established this topology **with limitations** and ended in teardown; it is not evidence of a currently running service. Local syntax/CLI checks are not a fresh deployment. Do not claim HA, capacity, SLOs or production operational ownership from this single-Machine/single-node topology.

**Approval boundary:** obtain explicit resource, budget, time-limit, identity/network and destructive-cleanup approval before performing provider actions. Preparing private files locally is not approval to deploy. Never modify the checked-in manifests, Containerfile or entrypoint to follow this page; render operator-owned copies.

## 1. Prepare the operator inputs

Use a checkout of the source revision above, Python 3.11+ (`tomllib`), OpenSSL, `jq`, `curl`, Fly CLI, a Docker-compatible AMD64 build path and the matching Flowplane CLI. Record actual versions. On each Linux dataplane install compatible Envoy and the matching `flowplane-agent` from the release archive as described in [production readiness](production-readiness.md). The Fly image is **linux/amd64**; a local ARM CLI build does not prove that image works.

You need ownership of a Fly organization, public DNS zone, a TLS-capable OIDC provider, a Tailscale tailnet with Services, and isolated Linux dataplane hosts. Supply an OIDC **access-token** audience accepted by Flowplane; an ID token is not interchangeable with an API access token. Register the CLI callback/client as described in [OIDC configuration](configure-oidc-provider.md) and [CLI auth and contexts](cli-auth-and-contexts.md). Obtain the intended first administrator's immutable `sub` from trusted IdP records. Obtain a separate unauthorized identity for negative checks.

From the repository root, create a private workspace **outside** the checkout and build context. Fill all example values with your own approved names; do not deploy these literal examples.

```bash
set -eu
set +x
umask 077
export RUN="$HOME/.flowplane/fly-run"
install -d -m 0700 "$RUN" "$RUN/cp" "$RUN/rls" "$RUN/evidence"
export FLY_ORG="your-fly-org"
export REGION="your-fly-region"
export CP_APP="your-unique-cp-app"
export RLS_APP="your-unique-rls-app"
export DB_APP="your-unique-db-app"
export API_HOST="cp.example.com"
export CP_SERVICE="your-cp-service"
export RLS_SERVICE="your-rls-service"
export TAILNET_DOMAIN="your-tailnet.ts.net"
export CP_HOSTNAME="your-cp-node"
export RLS_HOSTNAME="your-rls-node"
export CP_FQDN="$CP_SERVICE.$TAILNET_DOMAIN"
export RLS_FQDN="$RLS_SERVICE.$TAILNET_DOMAIN"
export OIDC_ISSUER="https://your-issuer.example.com/"
export OIDC_AUDIENCE="your-api-audience"
```

Keep a private owned-resource ledger: exact Fly app/Machine/volume/database IDs, DNS records and prior values, tailnet Service/tag/node/key IDs, IdP client/identity IDs, and Linux unit/container IDs. Record whether each item is newly created, shared or pre-existing. Define an end time and monetary ceiling. Shared artifacts must be preserved or restored, not deleted by pattern.

### Certificates and secret files

Obtain certificates through your organization's PKI. The [issuer compatibility procedure](production-readiness.md#issuer-ca-compatibility-and-upgrade) specifies CA requirements. Do not use a quick self-signed leaf in place of a valid issuing CA. Separate **server-trust CA bundles** from the **client-identity issuing CA**; the issued client `ca_certificate_pem` is not automatically the CP/RLS server-trust CA.

| Local operator input | Destination/use | Required names and trust |
|---|---|---|
| `$RUN/cp/api.crt`, `api.key` | CP native API TLS | SAN `$API_HOST`; chain trusted by public clients and Fly's HTTPS readiness checker |
| `$RUN/cp/xds.crt`, `xds.key` | CP xDS + diagnostics server | SAN `$CP_FQDN`; serverAuth; dataplanes separately receive its server-trust CA |
| `$RUN/cp/issuer-ca.crt`, `issuer-ca.key` | Client certificate issuance | Valid CA with matching private key, `CA:TRUE`, `keyCertSign`, SKI; not an EKU restriction excluding clientAuth |
| `$RUN/cp/dataplane-ca.crt` and `$RUN/rls/dataplane-ca.crt` | xDS/RLS client verification | Trust the issuer of registered dataplane clients; normally copy the issuer trust bundle, not a server CA |
| `$RUN/rls/rls-grpc.crt`, `rls-grpc.key` | RLS gRPC server | SAN `$RLS_FQDN`; serverAuth; dataplanes receive its server-trust CA |
| `$RUN/rls/rls-admin.crt`, `rls-admin.key` | RLS admin HTTPS | SAN `$RLS_APP.internal`; serverAuth; CP trusts `$RUN/cp/rls-admin-ca.crt` |
| `$RUN/cp/rls-admin-token` and `$RUN/rls/rls-admin-token` | CP policy pushes / RLS authorization | Same high-entropy bearer credential on both sides |
| `$RUN/cp/bootstrap-token` | First initialization only | Operator-generated token ≥32 characters; same token for every CP replica |
| `$RUN/cp/tailscale-authkey`, `$RUN/rls/tailscale-authkey` | Role-scoped node enrollment | Distinct short-lived, reusable, ephemeral, pre-authorized keys with only the approved role tag |
| `$RUN/cp/kek` | `FLOWPLANE_SECRET_ENCRYPTION_KEY` | Base64 encoding of exactly 32 random bytes; preserve for restore/decryption |

All input files must be owner-only regular files (`0600`) in `0700` directories **at creation**. Populate them using your secret manager/private file workflow; do not print tokens, keys or database URLs to a terminal. Base64 is encoding, not protection. Disable shell tracing; do not dump environment, Fly secret values, or unrestricted machine environment inspection into evidence.

CP and RLS Service FQDNs must exactly match their server certificate SAN and client SNI/server-name settings. API readiness uses a separate public SNI `$API_HOST`. Check leaf validity, SAN and chain before deployment, for example:

```bash
# SAN and SNI: the Service FQDN is the client server-name, not a node name.
openssl x509 -in "$RUN/cp/xds.crt" -noout -checkhost "$CP_FQDN"
openssl x509 -in "$RUN/rls/rls-grpc.crt" -noout -checkhost "$RLS_FQDN"
openssl x509 -in "$RUN/rls/rls-admin.crt" -noout -checkhost "$RLS_APP.internal"
openssl x509 -in "$RUN/cp/api.crt" -noout -checkhost "$API_HOST"
```

Also verify each certificate chain, validity interval, key match and intended EKU using your PKI's approved tooling. The hostname checks alone do not prove any of those.

### Configure Tailscale Services before boot

Use the [Tailscale Services guide](https://tailscale.com/kb/1552/tailscale-services) and your tailnet's admin console/policy. Create `svc:$CP_SERVICE` and `svc:$RLS_SERVICE` with their respective `tcp:18000` and `tcp:50051` endpoints. Confirm their DNS names exactly equal `$CP_FQDN` / `$RLS_FQDN`; use the actual console-reported names, not an assumed node hostname or TailVIP. Set role tag ownership and Service host approval (manual approval or narrowly scoped `autoApprovers.services`). Permit only approved dataplane sources to reach CP 18000 and RLS 50051, using the current Services/policy syntax in that guide; record the policy and its tests before enrollment. Do not grant blanket tailnet access just to clear a readiness failure.

Create the short-lived reusable, ephemeral role keys described above, scoped to those tags; record expiry without recording secret values. The image starts Tailscale with userspace networking, `--accept-dns=false`, `--accept-routes=false`, then advertises a raw TCP Service to loopback. The CP/RLS processes, **not Tailscale**, terminate mTLS. The node's state on Fly's disposable filesystem is not durable identity. Every boot reenrolls and re-advertises. A stable Service name fixes node-name churn; it does **not** fix an expired enrollment key. Rotation is required before the next restart after expiry. See [auth-key lifecycle](https://tailscale.com/kb/1085/auth-keys).

## 2. Render both manifests privately

The checked-in packaging contains qualification-specific values, not placeholder tokens. In particular, there is no `API_TLS_SERVER_NAME_REQUIRED` token to replace. Render both manifest inputs from the repository root; the generated output files are `$RUN/cp.fly.toml` and `$RUN/rls.fly.toml`. This block changes only the explicit delivery values below and asserts that all remaining structure, security flags, file bindings and ports remain identical.

| Override | CP | RLS |
|---|---|---|
| `app`, `primary_region` | `$CP_APP`, `$REGION` | `$RLS_APP`, `$REGION` |
| `[build].dockerfile` | `deploy/fly/Containerfile` | Same; repository-root context |
| `TAILSCALE_HOSTNAME` | `$CP_HOSTNAME` (nonempty) | `$RLS_HOSTNAME` (nonempty) |
| `TAILSCALE_SERVICE_NAME` | `svc:$CP_SERVICE` | `svc:$RLS_SERVICE` |
| `FLOWPLANE_RLS_GRPC_URL` | `$RLS_FQDN:50051` | Not used |
| `FLOWPLANE_RLS_ADMIN_URL` | `https://$RLS_APP.internal:8081` | Not used |
| HTTPS readiness `tls_server_name` | `$API_HOST` | No public Fly service |

```bash
python3 - <<'PY'
# Render manifest overrides into private output files.
import copy, json, os, pathlib, re, tomllib
run = pathlib.Path(os.environ['RUN'])
required = ['CP_APP','RLS_APP','REGION','CP_HOSTNAME','RLS_HOSTNAME',
            'CP_SERVICE','RLS_SERVICE','CP_FQDN','RLS_FQDN','API_HOST']
for key in required:
    value = os.environ[key]
    assert value and re.fullmatch(r'[A-Za-z0-9.-]+', value), key
assert os.environ['CP_FQDN'].startswith(os.environ['CP_SERVICE'] + '.')
assert os.environ['RLS_FQDN'].startswith(os.environ['RLS_SERVICE'] + '.')
for role, source in [('cp','fly.toml'), ('rls','rls.fly.toml')]:
    text = pathlib.Path('deploy/fly', source).read_text()
    before = tomllib.loads(text)
    expected = copy.deepcopy(before)
    values = {
        'app': os.environ['CP_APP' if role == 'cp' else 'RLS_APP'],
        'primary_region': os.environ['REGION'],
        'dockerfile': 'deploy/fly/Containerfile',
        'TAILSCALE_HOSTNAME': os.environ['CP_HOSTNAME' if role == 'cp' else 'RLS_HOSTNAME'],
        'TAILSCALE_SERVICE_NAME': 'svc:' + os.environ['CP_SERVICE' if role == 'cp' else 'RLS_SERVICE'],
    }
    expected['app'] = values['app']
    expected['primary_region'] = values['primary_region']
    expected['build']['dockerfile'] = values['dockerfile']
    for key in ['TAILSCALE_HOSTNAME','TAILSCALE_SERVICE_NAME']:
        expected['env'][key] = values[key]
    if role == 'cp':
        values.update(FLOWPLANE_RLS_GRPC_URL=os.environ['RLS_FQDN'] + ':50051',
                      FLOWPLANE_RLS_ADMIN_URL='https://' + os.environ['RLS_APP'] + '.internal:8081',
                      tls_server_name=os.environ['API_HOST'])
        for key in ['FLOWPLANE_RLS_GRPC_URL','FLOWPLANE_RLS_ADMIN_URL']:
            expected['env'][key] = values[key]
        expected['services'][0]['http_checks'][0]['tls_server_name'] = values['tls_server_name']
    for key, value in values.items():
        text, count = re.subn(r'(?m)^(\s*' + re.escape(key) + r'\s*=\s*)"[^"]*"',
                             lambda m: m[1] + json.dumps(value), text)
        assert count == 1, (role, key, count)
    assert tomllib.loads(text) == expected
    target = run / ('cp.fly.toml' if role == 'cp' else 'rls.fly.toml')
    assert not target.exists(), 'use a new private workspace rather than overwrite'
    fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as f:
        f.write(text)
PY
```

The CP must still have only public TCP `443 → 8080`, `handlers = []`, `FLOWPLANE_API_INSECURE=false`, HTTPS readiness `/readyz`, and `tls_skip_verify=false`. No Fly HTTP/TLS handler may terminate API TLS. xDS remains loopback 18000; RLS gRPC loopback 50051; RLS admin private Fly 6PN `[::]:8081` HTTPS plus bearer. RLS must have **no public service**. PostgreSQL, Envoy admin 9901 and agent health 19902 are not public services. Preserve every `[[files]]` binding; never substitute an insecure flag to get a green check.

Record the exact source SHA and SHA-256 hashes of the rendered configs, Containerfile, `.dockerignore`, Cargo lockfile and source input set privately. The existing root `.dockerignore` is an allowlist: do not widen it to include credentials or your private run directory. The Containerfile copies `Cargo.toml`, `Cargo.lock`, `crates/` and `deploy/fly/entrypoint.sh` from the **repository root**; running with `deploy/fly` as the build context is wrong. Record both built image digest and binary/release provenance at deployment time; a source SHA alone is not an image digest. To exercise the exact packaging locally with an AMD64-capable Docker builder before provider actions, use:

```bash
docker build --platform linux/amd64 --file deploy/fly/Containerfile \
  --tag flowplane-fly-operator:3.1.4 .
```

This local build can consume substantial time/disk and downloads dependencies. Its success is build evidence, not Fly readiness or live qualification. The remote-build alternative below uses the same pinned AMD64 packaging.

## 3. Create the private Fly delivery

The following commands incur cost or mutate provider state. Run only within approved scope. Read [Fly Managed Postgres](https://fly.io/docs/postgres/) and [Fly Postgres (unmanaged)](https://fly.io/docs/postgres/getting-started/what-you-should-know/) before selecting a database: the self-managed `flyctl postgres` path below is the unmanaged product, mirrors the evaluated topology and is **not** Managed Postgres or a production support/HA claim. For production, choose a supported DB operating model and verify backup/restore, transport security and connection details rather than silently substituting a different product. Private Fly 6PN reachability is not proof of PostgreSQL TLS encryption.

```bash
flyctl apps create "$CP_APP" --org "$FLY_ORG"
flyctl apps create "$RLS_APP" --org "$FLY_ORG"
flyctl postgres create --name "$DB_APP" --org "$FLY_ORG" --region "$REGION" \
  --vm-size shared-cpu-1x --volume-size 10 --initial-cluster-size 1 \
  > "$RUN/evidence/postgres-create.txt" 2>&1
flyctl postgres attach "$DB_APP" --app "$CP_APP" \
  --variable-name FLOWPLANE_DATABASE_URL \
  > "$RUN/evidence/postgres-attach.txt" 2>&1
```

The PostgreSQL commands may emit credentials; the files above are secret-bearing, private captures, not publishable evidence. On command failure inspect privately and stop; do not proceed from an assumed success. Attachment creates the CP database URL secret; do not echo or paste it into the manifest. Record exact app/database/role/volume IDs privately and verify region/network attachment with readback before deploying. Do not reuse an existing DB without scoped approval and a tested restore plan.

Import the runtime/file secrets using the manifests' actual `secret_name` mappings. Every CP/RLS file in the input table must exist. This block sends secret values over stdin, never argv/stdout; do not enable debug logging. Check Fly's current [secret-file bindings](https://fly.io/docs/reference/configuration/#the-files-section): file contents are base64 encoded in Fly secrets and decoded by the runtime binding.

```bash
python3 - <<'PY'
import base64, os, pathlib, stat, subprocess, tomllib
run = pathlib.Path(os.environ['RUN'])
def private_bytes(path):
    st = path.lstat()
    assert stat.S_ISREG(st.st_mode) and stat.S_IMODE(st.st_mode) == 0o600
    assert st.st_uid == os.getuid() and path.parent.stat().st_mode & 0o077 == 0
    value = path.read_bytes()
    assert value
    return value
for role, app in [('cp', 'CP_APP'), ('rls', 'RLS_APP')]:
    name = 'cp.fly.toml' if role == 'cp' else 'rls.fly.toml'
    config = tomllib.loads((run / name).read_text())
    secrets = {f['secret_name']: base64.b64encode(private_bytes(
        run / role / pathlib.Path(f['guest_path']).name)).decode() for f in config['files']}
    if role == 'cp':
        secrets['FLOWPLANE_OIDC_ISSUER'] = os.environ['OIDC_ISSUER']
        secrets['FLOWPLANE_OIDC_AUDIENCE'] = os.environ['OIDC_AUDIENCE']
        kek = private_bytes(run / 'cp' / 'kek').decode().strip()
        assert len(base64.b64decode(kek, validate=True)) == 32
        secrets['FLOWPLANE_SECRET_ENCRYPTION_KEY'] = kek
    assert all('\n' not in v and '\r' not in v for v in secrets.values())
    payload = ''.join(k + '=' + v + '\n' for k, v in secrets.items())
    with open(run / 'evidence' / (role + '-secret-import.txt'), 'ab') as capture:
        subprocess.run(['flyctl','secrets','import','--stage','--app',os.environ[app]],
                       input=payload.encode(), stdout=capture, stderr=capture, check=True)
PY
```

From the same repository root, deploy RLS first, verify its tailnet Service host approval, then deploy CP. For the pinned AMD64 image use the Fly remote builder (or a separately verified AMD64 builder), not an unverified native ARM image.

```bash
# Deployment executes the configured release/boot migration, not a local proof.
flyctl deploy . --config "$RUN/rls.fly.toml" --dockerfile deploy/fly/Containerfile \
  --app "$RLS_APP" --remote-only --ha=false
flyctl deploy . --config "$RUN/cp.fly.toml" --dockerfile deploy/fly/Containerfile \
  --app "$CP_APP" --remote-only --ha=false
flyctl status --app "$RLS_APP"
flyctl status --app "$CP_APP"
flyctl checks list --app "$CP_APP"
```

The packaging invokes CP migrations at **two** points: `[deploy].release_command = "flowplane db migrate"` on release, then `entrypoint.sh` runs `flowplane db migrate` at CP boot. `flowplane serve` also applies any pending migrations itself at startup; migrations are forward-only and safe to rerun on every boot. Release-command arguments bypass normal machine startup via `exec "$@"`; a release migration exits instead of launching a second CP/Tailscale node. These repeated migration attempts are intentional packaging behavior. Stop on failure; don't bypass migration or manually edit SQL migration state. RLS has no release migration.

## 4. Establish API readiness and bootstrap

Allocate an approved public IP for CP with `flyctl ips allocate-v4 --app "$CP_APP"` (Fly may charge for a dedicated IPv4) and, if needed, `flyctl ips allocate-v6 --app "$CP_APP"`. Read back addresses using `flyctl ips list --app "$CP_APP"`. Do not allocate public addresses/services for RLS or PostgreSQL.

Before changing DNS, verify the endpoint with the intended API hostname and normal CA validation:

```bash
export CP_PUBLIC_IP="the-approved-allocated-IPv4"
curl --fail --silent --show-error --resolve "$API_HOST:443:$CP_PUBLIC_IP" \
  "https://$API_HOST/readyz"
```

A successful `/readyz` is necessary, not sufficient. Confirm the CP Fly check is passing with matching SNI and certificate chain, native TLS is active, DB migrations succeeded, and both Tailscale Services have approved hosts. The RLS admin URL is private HTTPS and must validate against its own CA; health endpoints are intentionally unauthenticated, but policy replacement requires the matching bearer. A public readiness check does not prove CP→RLS policy sync.

Only then change the owned API DNS A/AAAA records to the verified Fly addresses. Preserve prior records and TTL for rollback; do not delete unrelated DNS. Verify `curl --fail --silent --show-error "https://$API_HOST/readyz"` from a normal client with CA verification enabled. There is no insecure TLS workaround in this runbook.

Initialize using the canonical [bootstrap procedure](bootstrap-platform.md): the same private token already mounted on CP, the intended immutable admin subject, and your platform organization name. That token is one-shot with a 24-hour expiry. Do not copy credential-bearing curl argv into a shared process/log capture. Authenticate with the real OIDC provider, set `FLOWPLANE_SERVER="https://$API_HOST"`, and verify `flowplane auth whoami` shows the intended platform admin. See [CLI auth and contexts](cli-auth-and-contexts.md) for token persistence and login inputs. Capture the identity result without credentials using explicit `--json --out`; pre-create the output at `0600` in the private directory because `--out` otherwise follows umask and preserves an existing file mode.

```bash
umask 077
install -m 0600 /dev/null "$RUN/evidence/whoami.json"
flowplane --json --out "$RUN/evidence/whoami.json" auth whoami
```

After initialization, remove the bootstrap token from normal runtime delivery. First render a second private CP config copy without `FLOWPLANE_BOOTSTRAP_TOKEN_FILE` and its associated `[[files]]` entry; retain every other field. Redeploy that reviewed copy, then unset the corresponding `FLOWPLANE_BOOTSTRAP_TOKEN_B64` Fly secret. Do **not** unset a secret still referenced by a deployed file binding. Initialized CP restarts do not need that token. Remove the local token according to your secret-retention policy; if initialization failed, stop and diagnose before removal.

### Delivery troubleshooting

| Symptom | Check / next action |
|---|---|
| Build cannot COPY source | Repository-root context, explicit `deploy/fly/Containerfile`, unchanged `.dockerignore` allowlist; do not copy private files into context |
| API certificate/readiness failure | SAN `$API_HOST`, full trusted chain, readiness SNI, raw TCP port mapping, matching cert/key; never `tls_skip_verify=true` |
| Migration/DB failure | Attached URL secret, DB region/network/reachability and migration result; inspect privately without printing URLs |
| Boot fails before readiness after restart | Enrollment key expiry/role/pre-authorization, Service host approval and advertisement; rotate scoped key rather than weaken auth |
| Agent cannot dial xDS / RLS cannot be reached | Approved Service endpoints, grants/ACL tests, DNS matching SAN, client/server CA separation; do not pin a suffixed node name |
| CP RLS pushes fail | Private `.internal` name/SAN, admin TLS CA, identical token files and private-network reachability; gRPC mTLS success does not prove admin push success |
| OIDC works but tenant operations fail | Identity is not membership; follow tenant ownership/team setup below, not a tailnet ACL change |

## 5. Continue to dataplane and lifecycle acceptance

The delivery portion above must be followed by tenant/dataplane setup, traffic/RLS and negative probes, recovery, backup/rollback and exact teardown before declaring the operator task complete. Those sections are the next documentation slice; this draft does not claim their execution or a fresh live pass.
