# AWS Secure Deployment Runbook

> Audience: platform-engineers, operators · Status: stable

This runbook is the concrete AWS packaging of Flowplane's provider-agnostic deployment invariants; it is self-contained — every step and value needed to stand up the environment is here.

The target is a strict secure smoke environment:

- API: client HTTPS -> AWS ALB HTTPS -> Flowplane CP HTTPS on port 8080.
- xDS: dataplane mTLS -> AWS NLB TCP passthrough -> Flowplane CP xDS mTLS on port 18000.
- ECS/Fargate tasks and RDS are private.
- ECS tasks use NAT egress for external OIDC/JWKS access.
- No `FLOWPLANE_API_INSECURE=true`.

## Inputs

Use `deploy/aws/local.auto.tfvars` for local operator values. This file is ignored.

Required high-level values:

- An AWS CLI authenticated to the target account and configured for the same region as `aws_region`.
- `aws_region` and matching `availability_zones`.
- `control_plane_image`: Flowplane release image in ECR.
- `api_certificate_arn`: ACM certificate for the public API hostname.
- `oidc_issuer` and `oidc_audience`.
- `xds_ingress_cidrs`: your local dataplane/operator public IP CIDR, for example `["1.2.3.4/32"]`.
- Secrets Manager ARNs for Flowplane KEK and PEM material.
- `XDS_SERVER_CA_SECRET_ID`: the Secrets Manager name or ARN containing, as its raw
  `SecretString`, the CA certificate that issued the xDS server certificate.

Set the OIDC values from your identity provider:

```bash
export TF_VAR_oidc_issuer="https://your-issuer.example.com"   # OIDC issuer URL
export TF_VAR_oidc_audience="your-api-audience"               # expected JWT aud claim
export FLOWPLANE_OIDC_ISSUER="$TF_VAR_oidc_issuer"
export FLOWPLANE_OIDC_CLIENT_ID="<public-cli-client-id-from-your-idp>"
```

The default region is `us-east-1` with `availability_zones = ["us-east-1a", "us-east-1b"]`. If you change regions, set AZs from the same region explicitly; this keeps planning usable with narrower IAM policies that do not allow availability-zone discovery.

## Secret Setup

Create Secrets Manager secrets for:

- `FLOWPLANE_SECRET_ENCRYPTION_KEY`
- API backend TLS certificate PEM
- API backend TLS private key PEM
- xDS server certificate PEM
- xDS server private key PEM
- dataplane client CA certificate PEM
- dataplane certificate issuer CA certificate PEM
- dataplane certificate issuer CA private key PEM
- xDS server-trust CA certificate PEM for the local workstation smoke test

The dataplane issuer CA certificate must be currently valid, match its private key, contain
`CA:TRUE`, `keyCertSign`, and a Subject Key Identifier, and allow `clientAuth`. A standards-complete
intermediate is supported as the explicit trust anchor. Detect and migrate existing issuer
material with the provider-neutral
[issuer CA compatibility procedure](production-readiness.md#issuer-ca-compatibility-and-upgrade).
Update the corrected public CA certificate in every Secrets Manager/verifier location and restart
those consumers **before** reissuing leaves. A same-key CA-certificate reissue is not a CA-key
rotation; the Flowplane upgrade itself does not rotate keys, rewrite trust stores, replace leaves,
or terminate running mTLS sessions.

Create the workstation-only xDS server-trust CA secret once, or export the name/ARN of an existing
secret with the same raw-PEM content:

```bash
export XDS_SERVER_CA_SECRET_ID="/flowplane/prod/xds-server-ca"
(
  set -eu
  openssl x509 -noout -in path/to/xds-server-ca.crt
  aws secretsmanager create-secret \
    --name "$XDS_SERVER_CA_SECRET_ID" \
    --secret-string file://path/to/xds-server-ca.crt
)
```

`XDS_SERVER_CA_SECRET_ID` is a local-operator input, not an OpenTofu module variable or output. The
module consumes the xDS leaf certificate through `xds_tls_cert_secret_arn`; it does not return that
certificate's issuing CA to the workstation.

If this workstation secret uses a customer-managed KMS key, the AWS CLI operator identity also
needs `kms:Decrypt` for that key. This is separate from module-side `secret_kms_key_arns` access.

The OpenTofu module passes secret ARNs into ECS. The container writes PEM values to files under `/tmp/flowplane/tls` before running `flowplane serve`.

The module generates the RDS password and stores it in Secrets Manager. Protect the OpenTofu state backend because generated secret material is present in state.

## Network Egress

Auth0/OIDC discovery and JWKS fetches require outbound HTTPS from the private ECS task. The module creates NAT egress by default. For the smoke environment it defaults to one NAT gateway to control cost; set `single_nat_gateway = false` if you want per-AZ NAT gateways.

## OpenTofu

```bash
tofu -chdir=deploy/aws init
tofu -chdir=deploy/aws validate
tofu -chdir=deploy/aws plan
tofu -chdir=deploy/aws apply
```

## Cloudflare DNS

Create records in `getflowplane.io` from module outputs:

```text
cp.getflowplane.io  -> api_alb_dns_name
xds.getflowplane.io  -> xds_nlb_dns_name
```

Keep `xds.getflowplane.io` DNS-only. Do not proxy xDS through Cloudflare for this smoke path; Flowplane must terminate the dataplane mTLS connection itself.

## Bootstrap

The control plane is **operator-seeded**: it never generates or logs a bootstrap token. You supply one, and an uninitialized non-dev instance started with no token refuses to start (fails closed). See [Bootstrap the platform](bootstrap-platform.md) for the full model.

Generate a token and store it in AWS Secrets Manager:

```bash
BOOTSTRAP_TOKEN="$(openssl rand -hex 32)"
aws secretsmanager create-secret \
  --name /flowplane/prod/bootstrap-token \
  --secret-string "$BOOTSTRAP_TOKEN"
```

Pass the secret's ARN to the module via `bootstrap_token_secret_arn`; it is injected into the CP task as `FLOWPLANE_BOOTSTRAP_TOKEN`. (If the secret uses a customer-managed KMS key, also add that key to `secret_kms_key_arns`.) On first boot the CP seeds the token's hash and logs a confirmation **without** the value.

Then initialize the platform admin once, using the **same** token and your verified OIDC subject:

```bash
curl -fsS -X POST https://cp.getflowplane.io/api/v1/bootstrap/initialize \
  -H "Authorization: Bearer $BOOTSTRAP_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
        "org_name": "platform",
        "org_display_name": "Platform",
        "admin_subject": "<oidc-sub-of-first-admin>",
        "admin_email": "you@example.com"
      }'
```

## CLI Login

Auth0 must have Device Code grant enabled.

```bash
export FLOWPLANE_SERVER=https://cp.getflowplane.io

flowplane auth login --device-code \
  --issuer "$FLOWPLANE_OIDC_ISSUER" \
  --client-id "$FLOWPLANE_OIDC_CLIENT_ID" \
  --scope "openid email profile"

flowplane auth whoami
```

## Local Dataplane Smoke

A dataplane is registered under a team, so a **tenant org and team must already exist**
(the platform org cannot host one). If you have only bootstrapped the platform admin,
first [create a tenant org and a team](create-tenant-org-and-team.md), then use that
org+team below.

Create the dataplane and issue a one-time cert response. The response contains
`private_key_pem`, and the CLI's global `--out` writes it under the **process umask** with no
mode of its own (`0644` under the common `umask 022`) — and if the file already exists it is
truncated in place and keeps its old mode. So make the file private *at creation*: `.local` is
`0700` (nobody else can traverse into it), the JSON target is pre-created at `0600` (safe even
when re-running over a leftover file), and `umask 077` backstops anything else created below.
`--json` is required: on an interactive terminal the CLI's default `table` format prints only a
one-line `created …` summary for a mutation and writes nothing to `--out`.

```bash
umask 077
install -d -m 0700 .local
install -m 0600 /dev/null .local/aws-dp-cert.json
flowplane dataplane create edge-local --team <team>
flowplane --json --out .local/aws-dp-cert.json dataplane cert issue edge-local --team <team>
```

Split the PEM values into files pre-created at `0600` (a redirect into an existing file keeps
its mode, so this is also re-run safe), then delete the one-time JSON. The key is not
recoverable from Flowplane, so the block only deletes the JSON after all three outputs have
been extracted (`jq -e` fails on a missing path instead of writing the string `null`) and parsed
by OpenSSL; `set -e` stops the subshell at the first failure, leaving the JSON in place:

```bash
(
  set -eu
  umask 077
  install -m 0600 /dev/null .local/aws-dp.key                    # one target per install: with several
  install -m 0600 /dev/null .local/aws-dp.crt                    # arguments, install treats the last one
  install -m 0600 /dev/null .local/aws-dp-client-chain-ca.crt    # as a directory
  jq -er '.data.private_key_pem'    .local/aws-dp-cert.json > .local/aws-dp.key                  # secret, 0600
  jq -er '.data.certificate_pem'    .local/aws-dp-cert.json > .local/aws-dp.crt                  # public leaf
  jq -er '.data.ca_certificate_pem' .local/aws-dp-cert.json > .local/aws-dp-client-chain-ca.crt  # public CA
  openssl pkey -noout -in .local/aws-dp.key
  openssl x509 -noout -in .local/aws-dp.crt
  openssl x509 -noout -in .local/aws-dp-client-chain-ca.crt
  rm -f .local/aws-dp-cert.json
)
```

Intended modes on the workstation: `aws-dp.key` `0600` (secret); the two certificate files are
public material and are also `0600` here only because the same user runs Envoy in this smoke
test — widen them to `0644` if a different local user needs to read them, never the key. Do
not copy `.local` into a shared drive, backup set, or ticket; if the JSON or key ever lands
outside `.local`, issue a replacement certificate and revoke this one.

`data.ca_certificate_pem` is the dataplane **client-chain CA** from the issue response. It is the CA the control plane trusts for the dataplane client certificate; it is not the CA Envoy uses to verify the control plane's xDS server certificate.

`XDS_SERVER_CA_SECRET_ID` names a workstation-only xDS **server-trust CA** secret that is separate from `data.ca_certificate_pem`; its `SecretString` contains the issuing CA that validates the certificate served by `xds.getflowplane.io`.

The block below is safe regardless of the shell's umask: `install -m 0600 /dev/null` pre-creates an
empty owner-only file, and the redirect that follows truncates that existing file in place, so it
inherits `0600` rather than the umask default. Use this shape whenever the process writing a file
does not let you control its mode. (The server-trust CA is public material, so `0600` is stricter
than it needs to be; it is harmless here and keeps every file in `.local` at one mode.)

```bash
(
  set -eu
  : "${XDS_SERVER_CA_SECRET_ID:?set this to the xDS server-trust CA secret name or ARN}"
  install -m 0600 /dev/null .local/aws-xds-server-ca.crt
  aws secretsmanager get-secret-value \
    --secret-id "$XDS_SERVER_CA_SECRET_ID" \
    --query SecretString \
    --output text \
    > .local/aws-xds-server-ca.crt
  test -s .local/aws-xds-server-ca.crt
  openssl x509 -noout -in .local/aws-xds-server-ca.crt
)
```

Generate the local Envoy bootstrap:

```bash
flowplane --out .local/aws-envoy.yaml dataplane bootstrap edge-local \
  --team <team> \
  --mode mtls \
  --xds-host xds.getflowplane.io \
  --xds-port 18000 \
  --cert-path "$PWD/.local/aws-dp.crt" \
  --key-path "$PWD/.local/aws-dp.key" \
  --ca-path "$PWD/.local/aws-xds-server-ca.crt"
```

Run Envoy locally with `.local/aws-envoy.yaml`, then apply a simple route/listener and confirm the dataplane receives xDS without NACKs.

## Teardown

```bash
tofu -chdir=deploy/aws destroy
```

If `deletion_protection=true`, disable it before destroy or keep the RDS instance intentionally.
