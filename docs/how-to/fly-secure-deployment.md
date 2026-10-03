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
export CP_CONFIG="$RUN/cp.fly.toml"
export RLS_CONFIG="$RUN/rls.fly.toml"
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
    config_path = pathlib.Path(os.environ['CP_CONFIG' if role == 'cp' else 'RLS_CONFIG'])
    config = tomllib.loads(config_path.read_text())
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

After successful initialization, remove the bootstrap token from normal runtime delivery and establish the active post-bootstrap config. Retain the original as first-boot provenance, not the config for future imports/deployments. This local transformation changes only the token env/file binding and preserves every other field.

```bash
python3 - <<'PY'
import copy, os, pathlib, re, tomllib
source = pathlib.Path(os.environ['CP_CONFIG'])
text = source.read_text()
before = tomllib.loads(text)
expected = copy.deepcopy(before)
expected['env'].pop('FLOWPLANE_BOOTSTRAP_TOKEN_FILE', None)
expected['files'] = [f for f in expected['files'] if f['secret_name'] != 'FLOWPLANE_BOOTSTRAP_TOKEN_B64']
text = re.sub(r'(?m)^\s*FLOWPLANE_BOOTSTRAP_TOKEN_FILE\s*=.*\n', '', text)
text = re.sub(r'(?ms)^\[\[files\]\]\n(?:(?!^\[).)*?secret_name = "FLOWPLANE_BOOTSTRAP_TOKEN_B64"\n', '', text)
assert tomllib.loads(text) == expected
assert 'FLOWPLANE_BOOTSTRAP_TOKEN_B64' not in text
assert 'FLOWPLANE_BOOTSTRAP_TOKEN_FILE' not in text
output = pathlib.Path(os.environ['RUN']) / 'cp.initialized.fly.toml'
fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, 'w') as f:
    f.write(text)
PY
export CP_CONFIG="$RUN/cp.initialized.fly.toml"
flyctl deploy . --config "$CP_CONFIG" --dockerfile deploy/fly/Containerfile \
  --app "$CP_APP" --remote-only --ha=false
flyctl checks list --app "$CP_APP"
# Only after this deployed config passes readiness, retire its unused secret.
flyctl secrets unset FLOWPLANE_BOOTSTRAP_TOKEN_B64 --app "$CP_APP"
```

Do **not** unset a secret still referenced by a deployed file binding. Initialized CP restarts do not need that token. Remove the local token according to your secret-retention policy; if initialization failed, stop and diagnose before removal. All later imports, renewal, upgrades and rollback use the active `$CP_CONFIG` and `$RLS_CONFIG`. When rendering a new upgrade workspace for this initialized database, perform this same transformation before deployment; do not bootstrap again.

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

## 5. Create the tenant org and team

The platform admin from section 4 governs the platform; it cannot host or see tenant resources. Follow [create a tenant org and a team](create-tenant-org-and-team.md) to create a tenant org, add its first owner by immutable OIDC subject, and create a team. Use [manage users, teams, and grants](manage-users-teams-and-grants.md) for any further members. The commands below are that page's sequence with this runbook's variables.

```bash
export ORG="your-tenant-org"
export TEAM="your-team"
export OWNER_SUB="immutable-oidc-sub-of-the-currently-authenticated-admin"
flowplane org create "$ORG"
flowplane org member add "$ORG" --role owner --subject "$OWNER_SUB"
export FLOWPLANE_ORG="$ORG"
export FLOWPLANE_TEAM="$TEAM"
flowplane team create "$TEAM" --org "$ORG"
flowplane team list --org "$ORG"
```

This worked path deliberately assigns the current administrator as tenant owner: obtain that same login’s immutable subject, not an arbitrary different owner. If ownership belongs to someone else, have that owner authenticate and verify `flowplane auth whoami` before creating the team. The exported tenant org/team selectors apply to every subsequent command.

Expected: the team is listed under the tenant org. A request that selects the platform org for tenant work fails with `org_selector_required`; that is correct behaviour, not a deployment fault. Run every later command as a tenant identity with the tenant org and team selected.

## 6. Register the dataplane and install its identity

Run sections 6–7 on one isolated Linux dataplane host that the team owns. Before starting, install the CLI there, authenticate the tenant operator in the same CLI context used by these commands, and re-export `FLOWPLANE_SERVER`, `ORG`, `TEAM`, `FLOWPLANE_ORG`, `FLOWPLANE_TEAM` and `CP_FQDN` with the values from sections 1–5. Use an approved privileged operator shell for `/etc` file delivery and ownership changes; this does not change the authenticated tenant identity and is not the runtime service account. Create the dedicated `flowplane-dp` service user through your host provisioning procedure first. The canonical procedure, file modes and the reasons for them are in [register a dataplane and connect its agent over mTLS](register-dataplane-mtls.md); this section adds only what is specific to this topology. Do not share one host, service user or certificate directory between teams.

Join the host to the tailnet with its own approved dataplane tag, using your tailnet's enrollment procedure, and confirm it resolves and reaches only the two Service endpoints the policy grants. The host needs no public inbound port for xDS or RLS.

Register the dataplane, then issue its client certificate into a file that is private at creation. `--json` is required with `--out`; without it the CLI writes nothing to the file.

```bash
export DP_NAME="your-dataplane"
export DP_DIR="/etc/flowplane/dp"
umask 077
install -d -m 0700 "$DP_DIR"
install -m 0600 /dev/null "$DP_DIR/dataplane.json"
install -m 0600 /dev/null "$DP_DIR/issue.json"
flowplane --json --out "$DP_DIR/dataplane.json" dataplane create "$DP_NAME" --team "$TEAM"
flowplane --json --out "$DP_DIR/issue.json" dataplane cert issue "$DP_NAME" --team "$TEAM" --ttl-hours 24
```

Split the one-time response into runtime files and delete it only after both outputs parse. The private key exists nowhere else; Flowplane does not store it.

```bash
(
  set -eu
  umask 077
  install -m 0600 /dev/null "$DP_DIR/client.key"
  install -m 0644 /dev/null "$DP_DIR/client.crt"
  jq -er '.data.private_key_pem' "$DP_DIR/issue.json" > "$DP_DIR/client.key"
  jq -er '.data.certificate_pem' "$DP_DIR/issue.json" > "$DP_DIR/client.crt"
  openssl pkey -noout -in "$DP_DIR/client.key"
  openssl x509 -noout -in "$DP_DIR/client.crt"
  rm -f "$DP_DIR/issue.json"
)
```

Install the two **server-trust** CA certificates. These are separate inputs from the issue response: the response's `ca_certificate_pem` is the client-chain CA that the control plane and RLS use to verify this dataplane, not the CA this dataplane uses to verify them. Obtain the CA that signed the CP xDS server certificate and the CA that signed the RLS gRPC server certificate from whoever provisioned section 1, and transfer them over a channel you trust.

```bash
install -m 0644 /dev/null "$DP_DIR/server-ca.crt"
cat path/to/cp-xds-server-ca.crt > "$DP_DIR/server-ca.crt"
openssl x509 -noout -in "$DP_DIR/server-ca.crt"
```

The packaging also fixes three paths that Envoy reads **on the dataplane host** for the Envoy-to-RLS hop. They are set by `FLOWPLANE_DATAPLANE_TLS_CERT`, `FLOWPLANE_DATAPLANE_TLS_KEY` and `FLOWPLANE_DATAPLANE_TLS_CLIENT_CA` in the CP manifest and are delivered to Envoy in its cluster configuration; they are not files on the control plane. The client certificate presented to RLS must chain to the CA that RLS trusts in `$RUN/rls/dataplane-ca.crt`. When that is the same issuer that signed this dataplane's certificate, reuse the issued pair:

```bash
umask 077
install -d -m 0700 /etc/flowplane/certs
# install copies each file and sets its mode as it is created.
install -m 0600 "$DP_DIR/client.key" /etc/flowplane/certs/dataplane.key
install -m 0644 "$DP_DIR/client.crt" /etc/flowplane/certs/dataplane.crt
install -m 0644 path/to/rls-grpc-server-ca.crt /etc/flowplane/certs/dataplane-ca.crt
openssl x509 -noout -in /etc/flowplane/certs/dataplane-ca.crt
```

Despite its variable name, `dataplane-ca.crt` on the dataplane host is the CA that Envoy uses to verify the **RLS server** certificate. At this source revision the injected rate-limit cluster verifies that the RLS certificate chains to that CA and presents the client certificate; it does not set an SNI value or pin the RLS hostname. Keep that CA dedicated to RLS server certificates so that chain validation alone identifies the service, and keep the `$RLS_FQDN` SAN on the certificate so that other clients and future revisions can verify the name.

Keep both directories under the privileged operator during bootstrap generation in section 7; hand them over only after `envoy.yaml` exists. Envoy and the agent must not run as root or as a shared login account.

Expected: `client.key` and `dataplane.key` are `0600`, certificates are `0644`, and `issue.json` is absent. If a key or the issue response was ever readable by another user, treat the key as exposed: issue a replacement, connect it, then revoke the old serial.

## 7. Start Envoy and the agent

Generate the Envoy bootstrap against the **Service FQDN**, never a node name or address. The generated bootstrap carries that name as the TLS server name and as the exact DNS SAN that the CP certificate must match.

```bash
flowplane --out "$DP_DIR/envoy.yaml" dataplane bootstrap "$DP_NAME" \
  --team "$TEAM" \
  --mode mtls \
  --xds-host "$CP_FQDN" \
  --xds-port 18000 \
  --cert-path "$DP_DIR/client.crt" \
  --key-path "$DP_DIR/client.key" \
  --ca-path "$DP_DIR/server-ca.crt"
grep -n "$CP_FQDN" "$DP_DIR/envoy.yaml"
chmod 0600 "$DP_DIR/envoy.yaml"
chown -R flowplane-dp:flowplane-dp "$DP_DIR" /etc/flowplane/certs
ls -ln "$DP_DIR" /etc/flowplane/certs
```

Start Envoy with that bootstrap under your service manager as the dataplane service user, then start the agent with the dataplane UUID from the registration response. The three agent TLS paths are all-or-none, and the server name must be the Service FQDN.

```bash
DP_ID="$(jq -er '.data.id' "$DP_DIR/dataplane.json")"
envoy -c "$DP_DIR/envoy.yaml" --log-level info &
flowplane-agent \
  --cp-endpoint "https://$CP_FQDN:18000" \
  --dataplane-id "$DP_ID" \
  --tls-cert-path "$DP_DIR/client.crt" \
  --tls-key-path "$DP_DIR/client.key" \
  --tls-ca-path "$DP_DIR/server-ca.crt" \
  --tls-server-name "$CP_FQDN"
```

The commands above show the invocation shape; in service use, run each process as its own supervised unit. Envoy admin stays on loopback inside the dataplane unit and is not exposed to the tailnet or to operators as a workflow.

## 8. Verify with control-plane diagnostics

Use control-plane diagnostics as the operator view of Envoy and the agent, rather than Envoy admin. The agent reads Envoy admin on loopback and reports curated diagnostics to the CP; the commands below read what the CP recorded for this team.

```bash
# Control-plane diagnostics for Envoy and the agent; no Envoy admin access needed.
flowplane dataplane get "$DP_NAME" --team "$TEAM"
flowplane stats overview --team "$TEAM"
flowplane ops xds status --team "$TEAM"
flowplane ops xds nacks --team "$TEAM"
```

Expected: `dataplane get` shows `last_heartbeat_at` advancing, `stats overview` counts the dataplane as live, `ops xds status` reports heartbeat-derived liveness and the persisted counters for it, and `ops xds nacks` shows no rejection for the configuration you applied. On the dataplane host only, `curl -fsS http://127.0.0.1:19902/healthz` returns `ok` once the agent has scraped Envoy and received an acknowledgment. That endpoint reports readiness, not process liveness: it returns `503` during a CP outage while the agent stays alive and Envoy keeps serving.

A live dataplane proves fresh accepted diagnostics heartbeats, not an active Envoy ADS stream: the agent can report independently of ADS, and this command has no connection-state field. Establish the separate configuration-delivery checkpoint by applying a new listener/route and verifying new marked traffic in section 9; then exercise RLS. No NACK entry alone is not proof of delivery.

## 9. Send tagged traffic through Envoy

Application traffic goes from clients to Envoy to the upstream. It never passes through the control plane, so a healthy API tells you nothing about this path.

Use an upstream you own that returns the request path or a header in its response. Expose it through the team's gateway, then send a request carrying a unique marker and verify that the same marker comes back.

```bash
export UPSTREAM="http://your-upstream-host:8080"
export LISTENER_PORT="10001"
flowplane expose "$UPSTREAM" --name runbook-probe --team "$TEAM" --port "$LISTENER_PORT"
MARKER="probe-$(date +%s)-$$"
curl --fail --silent --show-error -H "x-runbook-marker: $MARKER" \
  "http://127.0.0.1:$LISTENER_PORT/$MARKER"
```

Run the `curl` on the dataplane host or from a client that is allowed to reach the listener. Expected: HTTP `200` and a response body containing the marker value. A response without the marker means you reached something other than your upstream; a connection refusal can mean listener/config delivery, process startup or bind/firewall failure. Heartbeat-derived `ops xds status` cannot identify which; correlate CP diagnostics/NACKs, supervised process status and the actual marked traffic result. Repeat `ops xds nacks` after the change and expect no new entry.

## 10. Verify rate-limit enforcement

Rate-limit decisions are made by RLS over the private gRPC mTLS Service; the control plane only pushes policy to the RLS admin endpoint over Fly private HTTPS. Follow [enable global rate limiting](global-rate-limit.md) from its step 3 for the domain, policy, route descriptor and listener filter; do not start a second RLS, because this deployment already runs one. A low limit makes the expected result easy to assert. Substitute its domain `checkout` with `runbook`, policy `per-client` with `probe`, descriptor value `acme` with `runbook`, and team `default` with `$TEAM`. Use the actual listener/route-config names returned by `expose`, not that page’s sample `edge`/`api-routes` names; inspect them through `flowplane listener list --team "$TEAM"` and `flowplane route list --team "$TEAM"`. Attach the filter and route descriptor to this exact resource set.

```bash
umask 077
printf '%s\n' '{"name":"runbook"}' > "$DP_DIR/rl-domain.json"
printf '%s\n' '{"name":"probe","spec":{"descriptors":{"api_key":"runbook"},"requests_per_unit":3,"unit":"minute"}}' > "$DP_DIR/rl-policy.json"
flowplane rate-limit domain create --team "$TEAM" --file "$DP_DIR/rl-domain.json"
flowplane rate-limit policy create --team "$TEAM" --domain runbook --file "$DP_DIR/rl-policy.json"
```

For the exact resources created above (`runbook-probe` listener, `runbook-probe-routes` route config and `runbook-probe-upstream` cluster), retrieve the current specs, add the canonical descriptor/filter without discarding the rest, and update with their current revisions. The following block is for the new runbook probe only, not existing application resources:

```bash
flowplane --json --out "$DP_DIR/route-before.json" route get runbook-probe-routes --team "$TEAM"
flowplane --json --out "$DP_DIR/listener-before.json" listener get runbook-probe --team "$TEAM"
jq -e '{spec: .data.spec} | .spec.virtual_hosts[0].routes[0].action.rate_limits = [{actions: [{type: "request_headers", header_name: "x-api-key", descriptor_key: "api_key"}]}]' \
  "$DP_DIR/route-before.json" > "$DP_DIR/route-update.json"
jq -e '{spec: .data.spec} | .spec.http_filters = ((.spec.http_filters // []) + [{filter: {type: "global_rate_limit", domain: "runbook", timeout_ms: 200, failure_mode_deny: false, request_type: "external"}}])' \
  "$DP_DIR/listener-before.json" > "$DP_DIR/listener-update.json"
flowplane route update runbook-probe-routes --team "$TEAM" \
  --revision "$(jq -er '.data.revision' "$DP_DIR/route-before.json")" --file "$DP_DIR/route-update.json"
flowplane listener update runbook-probe --team "$TEAM" \
  --revision "$(jq -er '.data.revision' "$DP_DIR/listener-before.json")" --file "$DP_DIR/listener-update.json"
flowplane route get runbook-probe-routes --team "$TEAM"
flowplane listener get runbook-probe --team "$TEAM"
```

Run the attach block once: on a conflict, re-read before rebuilding the update; do not replay the append on an already attached filter. After adding the route descriptor and attaching the `global_rate_limit` filter, allow up to 60 seconds for the reconcile loop. With no competing requests and a fresh counter window, send five requests in the same minute window with the limited descriptor value:

```bash
for i in 1 2 3 4 5; do
  curl --silent --output /dev/null --write-out '%{http_code}\n' \
    -H 'x-api-key: runbook' "http://127.0.0.1:$LISTENER_PORT/"
done | sort | uniq -c
```

Expected within one minute window: three `200` responses and two `429` responses. All `200` means the policy has not reached RLS, the route does not emit the descriptor, or the filter is not attached; with `failure_mode_deny: false` it can also mean Envoy could not reach RLS and failed open, so check the RLS path rather than assuming enforcement. A `5xx` with `failure_mode_deny: true` may indicate an RLS call failure, but upstream/application failures also produce `5xx`; distinguish these using CP diagnostics and upstream observations. For a suspected RLS path failure, check the three files under `/etc/flowplane/certs`, the RLS Service approval and the tailnet policy.

## 11. Run negative probes

A deployment that only passes positive checks has not shown its boundaries. Run each probe and record the observed result; an unexpected success is a finding to fix before use.

| Probe | Where | Expected result |
|---|---|---|
| TCP connect to `$API_HOST` ports `18000` and `50051` | Host outside the tailnet | Refused or timed out; only `443` is public |
| TCP connect to the RLS and PostgreSQL apps on any public address | Host outside the tailnet | No public address or service exists |
| xDS handshake with no client certificate | Dataplane host | No usable session: TLS alert or immediate close |
| xDS handshake verifying a wrong server name | Dataplane host | Certificate verification fails on hostname mismatch |
| xDS or agent connection with a valid-CA certificate that is not registered for this dataplane | Dataplane host | Rejected by the control plane; the dataplane does not become live |
| Team read with the separate unauthorized identity from section 1 | Operator workstation | Authorization error and non-zero exit; never an empty success |
| Tenant command while selecting the platform org | Operator workstation | `org_selector_required` |
| Connection to `$CP_FQDN:18000` from a tailnet device the policy does not grant | Ungranted tailnet device | Blocked by tailnet policy before TLS |

```bash
# From the dataplane host. Neither command sends application data.
openssl s_client -connect "$CP_FQDN:18000" -servername "$CP_FQDN" \
  -CAfile "$DP_DIR/server-ca.crt" -verify_return_error </dev/null
openssl s_client -connect "$CP_FQDN:18000" -servername "$CP_FQDN" \
  -CAfile "$DP_DIR/server-ca.crt" -verify_hostname wrong.invalid -verify_return_error </dev/null
```

The first command presents no client certificate and must not produce a session the server keeps open. The second must report a hostname verification error. Never add an option that disables verification to make a probe pass.

## 12. Recover from restarts and expiry

**Control-plane or RLS outage.** Envoy keeps serving its last accepted configuration while the control plane is unreachable. New dataplanes cannot join and configuration changes do not propagate until it returns. Agent readiness returns `503` and recovers by itself after the CP accepts reports again; do not restart a surviving agent. RLS counters are in memory and reset when RLS restarts.

**Same-Machine restart.** The deployment must tolerate disposable root-filesystem/Tailscale state on Machine replacement or redeployment. Do not assume every same-Machine restart necessarily erases state: verify the actual restart behavior and Service host readback. The entrypoint invokes enrollment and re-advertisement at boot regardless. Dataplanes reconnect to the unchanged Service FQDN without a new bootstrap.

```bash
flyctl machine list --app "$CP_APP"
flyctl machine restart "CP-MACHINE-ID-FROM-THE-LIST" --app "$CP_APP"
flyctl checks list --app "$CP_APP"
flowplane ops xds status --team "$TEAM"
```

Expected: the readiness check passes again, fresh agent heartbeats resume, and new marked traffic confirms configuration delivery. Heartbeat liveness alone does not prove an ADS stream. If the Machine does not become ready, suspect the enrollment key before anything else.

**Expired enrollment key.** The role key is read at every boot. Once it expires, the running node keeps working, but the next restart cannot enroll and the Machine fails before readiness. Rotate before expiry, and always before a planned restart. Create a replacement key with the same role tag and properties, write it to the same private input path, stage it over stdin and restart:

```bash
python3 - <<'PY'
import base64, os, pathlib, stat, subprocess
path = pathlib.Path(os.environ['RUN']) / 'cp' / 'tailscale-authkey'
st = path.lstat()
assert stat.S_ISREG(st.st_mode) and stat.S_IMODE(st.st_mode) == 0o600
value = base64.b64encode(path.read_bytes()).decode()
assert value and '\n' not in value
with open(pathlib.Path(os.environ['RUN']) / 'evidence' / 'cp-key-rotation.txt', 'ab') as capture:
    subprocess.run(['flyctl', 'secrets', 'import', '--stage', '--app', os.environ['CP_APP']],
                   input=('TAILSCALE_AUTHKEY_B64=' + value + '\n').encode(),
                   stdout=capture, stderr=capture, check=True)
PY
flyctl secrets deploy --app "$CP_APP"
flyctl checks list --app "$CP_APP"
```

Repeat for RLS with `$RUN/rls/tailscale-authkey` and `$RLS_APP`. Then confirm in the tailnet console that each Service has an approved host and remove the stale offline node records. Revoke the superseded key.

**Leaf certificate renewal.** Dataplane client certificates are short-lived. Renew before expiry in this order: issue a replacement, install it, verify the dataplane is live on the replacement, then revoke the old serial. Never revoke first. Up to two unrevoked certificates may overlap for one dataplane.

```bash
install -m 0600 /dev/null "$DP_DIR/issue.json"
flowplane --json --out "$DP_DIR/issue.json" dataplane cert issue "$DP_NAME" --team "$TEAM" --ttl-hours 24
```

Do not reuse section 6’s initial installation block on running units: it truncates live files. Set `ENVOY_UNIT` and `AGENT_UNIT` to the exact owned Linux systemd unit names (or perform equivalent operations with your supervisor). Stage and validate the replacement in the same private filesystem; record the old serial privately from `dataplane cert list`. Stopping Envoy interrupts application traffic on this dataplane: schedule a maintenance window or shift traffic to another independently configured dataplane first. Stop both units during pair replacement so no reader observes a mixed cert/key generation, install through atomic per-file renames, then start both. This controlled reconnect forces the agent and Envoy to reread the new credentials; an existing healthy connection can otherwise continue using the old certificate.

```bash
export ENVOY_UNIT="your-owned-envoy.service"
export AGENT_UNIT="your-owned-flowplane-agent.service"
(
set -eu
umask 077
install -d -m 0700 "$DP_DIR/renewal"
install -m 0600 /dev/null "$DP_DIR/renewal/client.key"
install -m 0644 /dev/null "$DP_DIR/renewal/client.crt"
jq -er '.data.private_key_pem' "$DP_DIR/issue.json" > "$DP_DIR/renewal/client.key"
jq -er '.data.certificate_pem' "$DP_DIR/issue.json" > "$DP_DIR/renewal/client.crt"
openssl pkey -noout -in "$DP_DIR/renewal/client.key"
openssl x509 -noout -in "$DP_DIR/renewal/client.crt"
# Verify key match, clientAuth, issuer trust, validity and the registered serial first.
# Never overwrite a backup left from an unresolved prior renewal.
test ! -e "$DP_DIR/previous"
install -d -m 0700 "$DP_DIR/previous"
install -m 0600 "$DP_DIR/client.key" "$DP_DIR/previous/client.key"
install -m 0644 "$DP_DIR/client.crt" "$DP_DIR/previous/client.crt"
install -m 0600 /etc/flowplane/certs/dataplane.key "$DP_DIR/previous/dataplane.key"
install -m 0644 /etc/flowplane/certs/dataplane.crt "$DP_DIR/previous/dataplane.crt"
sudo systemctl stop "$ENVOY_UNIT" "$AGENT_UNIT"
chown flowplane-dp:flowplane-dp "$DP_DIR/renewal/client.key" "$DP_DIR/renewal/client.crt"
mv "$DP_DIR/renewal/client.key" "$DP_DIR/client.key"
mv "$DP_DIR/renewal/client.crt" "$DP_DIR/client.crt"
install -m 0600 "$DP_DIR/client.key" /etc/flowplane/certs/dataplane.key.new
install -m 0644 "$DP_DIR/client.crt" /etc/flowplane/certs/dataplane.crt.new
chown flowplane-dp:flowplane-dp /etc/flowplane/certs/dataplane.key.new /etc/flowplane/certs/dataplane.crt.new
mv /etc/flowplane/certs/dataplane.key.new /etc/flowplane/certs/dataplane.key
mv /etc/flowplane/certs/dataplane.crt.new /etc/flowplane/certs/dataplane.crt
sudo systemctl start "$ENVOY_UNIT" "$AGENT_UNIT"
rm -f "$DP_DIR/issue.json"
)
```

Use your approved privileged file-delivery mechanism for these owner changes and do not leave processes running as root. After both units have new start times, verify fresh diagnostics acknowledgments, new marked configuration/traffic and RLS acceptance. Heartbeats on the pre-renewal channel are not replacement proof. Only after successful reconnection and traffic with the installed replacement should you revoke the old certificate by its serial. The private `previous/` directory now holds the old unrevoked pair for both hops. If replacement checks fail, run the failure-only restore below; do not revoke the old serial to force progress. Revoke the failed new serial before another issue attempt to free the two-certificate slot. After successful replacement, run only the success block:

```bash
(
set -eu
flowplane dataplane cert list --team "$TEAM"
flowplane dataplane cert revoke "OLD-SERIAL" --team "$TEAM" --reason "renewed"
# Only after supported revocation succeeds, dispose of these exact backup files.
rm -f "$DP_DIR/previous/client.key" "$DP_DIR/previous/client.crt" \
  "$DP_DIR/previous/dataplane.key" "$DP_DIR/previous/dataplane.crt"
rmdir "$DP_DIR/previous"
)
```

```bash
# FAILURE ONLY: restore under stopped units; preserve the old serial unrevoked.
(
set -eu
sudo systemctl stop "$ENVOY_UNIT" "$AGENT_UNIT"
install -m 0600 "$DP_DIR/previous/client.key" "$DP_DIR/client.key"
install -m 0644 "$DP_DIR/previous/client.crt" "$DP_DIR/client.crt"
install -m 0600 "$DP_DIR/previous/dataplane.key" /etc/flowplane/certs/dataplane.key
install -m 0644 "$DP_DIR/previous/dataplane.crt" /etc/flowplane/certs/dataplane.crt
chown flowplane-dp:flowplane-dp "$DP_DIR/client.key" "$DP_DIR/client.crt" \
  /etc/flowplane/certs/dataplane.key /etc/flowplane/certs/dataplane.crt
sudo systemctl start "$ENVOY_UNIT" "$AGENT_UNIT"
flowplane dataplane cert revoke "FAILED-NEW-SERIAL" --team "$TEAM" --reason "renewal rollback"
)
```

Verify restored traffic/diagnostics and resolve the failed renewal before retrying. Keep the old backup private until that recovery is verified; then dispose of its four exact files as above. Run these failure-only commands separately from the success commands, never sequentially.

CP and RLS server certificates are operator-issued inputs. To renew one, replace its file under `$RUN`, rerun section 3’s importer using the active `$CP_CONFIG`/`$RLS_CONFIG`, and redeploy those same configs; do not restore a bootstrap binding. API renewal preserves SAN `$API_HOST`; CP xDS renewal preserves `$CP_FQDN`; RLS gRPC preserves `$RLS_FQDN`; RLS admin preserves `$RLS_APP.internal`. Redistribute changed server-trust CAs first with an overlap window. A CP Service name change requires updated bootstrap and agent server-name, not merely a new certificate. An issuer CA change follows the ordered procedure in [production readiness](production-readiness.md#issuer-ca-compatibility-and-upgrade).

## 13. Back up, upgrade and roll back

The product rules for these operations are in [production readiness](production-readiness.md): what to back up together, the restore drill, and when a restore is forbidden. This section adds only the provider commands. Back up PostgreSQL together with the `$RUN/cp/kek` key material and the TLS and CA inputs; a database restored without its matching key cannot decrypt stored secrets. See [secret KEK rotation](secret-kek-rotation.md).

```bash
# Provider-level backup of the self-managed database volume.
flyctl volumes list --app "$DB_APP"
flyctl volumes snapshots create "DB-VOLUME-ID" --app "$DB_APP"
flyctl volumes snapshots list "DB-VOLUME-ID"
```

A volume snapshot is not a tested restore. Restore it into an isolated database and run the restore drill before relying on it.

For an upgrade, check out the new source revision, render new private manifests into a new private workspace as in section 2, read the release's upgrade notes, and take a verified backup first. This topology runs one control-plane Machine, so an upgrade is a brief outage of the API and of configuration changes; dataplanes keep serving. It is not a zero-downtime procedure.

```bash
# Upgrade: render/strip bootstrap first, then use the new active configs.
flyctl deploy . --config "$RLS_CONFIG" --dockerfile deploy/fly/Containerfile \
  --app "$RLS_APP" --remote-only --ha=false
flyctl deploy . --config "$CP_CONFIG" --dockerfile deploy/fly/Containerfile \
  --app "$CP_APP" --remote-only --ha=false
flyctl checks list --app "$CP_APP"
```

Before an upgrade render into a new `$RUN` and apply section 4’s bootstrap-removal transformation there without rerunning platform initialization. Export the new active `$CP_CONFIG` and `$RLS_CONFIG`, securely copy the current `cp/` and `rls/` input directories into the new `$RUN` with directories `0700` and secret files `0600`, and rotate only inputs deliberately required by the upgrade. Import only the active configs’ bindings and preserve the deployed identity/KEK settings. Do not redeploy the original first-boot config. Verify readiness, marked traffic and RLS after the upgrade.

Migrations are forward-only. After a release has migrated the database, an older binary cannot be used as a binary-only rollback.

```bash
# Rollback: identify the previous image, and redeploy it only if no migration ran.
flyctl releases --app "$CP_APP" --image
flyctl releases --app "$RLS_APP" --image
flyctl deploy --image "PREVIOUS-RLS-IMAGE-REF" --config "$RLS_CONFIG" --app "$RLS_APP" --ha=false
flyctl deploy --image "PREVIOUS-CP-IMAGE-REF" --config "$CP_CONFIG" --app "$CP_APP" --ha=false
flyctl checks list --app "$CP_APP"
# Verify marked traffic and RLS again with the restored image pair.
```

If a migration ran, rollback means restoring the pre-upgrade backup, and that is allowed only under the conditions in the [rollback rules](production-readiness.md#upgrade-rollback-and-version-skew). Otherwise recover by rolling forward.

## 14. Tear down exactly what you created

Work from the owned-resource ledger from section 1. Delete by exact name or ID, never by pattern, and leave every shared or pre-existing item as it was. Freeze any evidence you need before deleting anything.

Retire product resources first, through supported commands, while the control plane is still running:

```bash
flowplane unexpose runbook-probe --team "$TEAM" --yes
flowplane rate-limit policy delete --team "$TEAM" --domain runbook probe --yes
flowplane rate-limit domain delete --team "$TEAM" runbook --yes
flowplane dataplane delete "$DP_NAME" --team "$TEAM" --reason "runbook teardown" --yes
flowplane dataplane list --team "$TEAM" --include-retired
```

On the dataplane host, stop the Envoy and agent units, remove `$DP_DIR` and `/etc/flowplane/certs`, and remove the host from the tailnet. Then restore DNS: put back the prior A/AAAA records and TTL you recorded, or remove only the records you created.

Remove the provider resources by exact app name. These commands are destructive and cannot be undone.

```bash
flyctl ips list --app "$CP_APP"
flyctl ips release "$CP_PUBLIC_IP" --app "$CP_APP"
flyctl postgres detach "$DB_APP" --app "$CP_APP"
flyctl apps destroy "$CP_APP"
flyctl apps destroy "$RLS_APP"
flyctl apps destroy "$DB_APP"
```

App destruction is destructive to its Machines and data-bearing resources; explicitly inventory remaining volumes/snapshots and billable backup storage instead of assuming the app command removed all storage. Skip the database destruction if it is shared or must be retained, and record that decision. Read back exact IDs in the ledger; delete only owned remaining artifacts through the provider’s supported volume/snapshot tools. Retention may keep snapshots or backup objects billable after app deletion.

In the tailnet console, remove the two Services, the role keys, the offline node records and the policy entries you added, and nothing else. In the IdP, remove only the client, API and test identities created for this deployment. Finally remove the private workspace under `$RUN` according to your secret-disposal policy, including the secret-bearing files under `$RUN/evidence`.

Verify by reading back, not by assuming:

```bash
flyctl apps list
flyctl ips list --app "$CP_APP"
```

Expected: none of the three app names appears in the list, and the second command fails because the app no longer exists. Confirm in the same way that the DNS records, tailnet Services, nodes and keys, and IdP objects on your ledger are gone, and that every item you marked shared or pre-existing is unchanged. Where you cannot read a provider back mechanically, record that the result is an operator attestation and not a machine check.

## What this page has not proven

This page is a draft. Its commands were checked against the source revision above, the installed command-line help and public provider documentation, and its embedded scripts were exercised with synthetic local inputs. No section has been run against live Fly, Tailscale, DNS or identity-provider resources at this revision. Treat every expected result in sections 3 through 14 as unverified until a fresh walkthrough has been completed, and report any step that does not behave as described.
