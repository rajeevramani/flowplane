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

- `aws_region` and matching `availability_zones`.
- `control_plane_image`: Flowplane release image in ECR.
- `api_certificate_arn`: ACM certificate for the public API hostname.
- `oidc_issuer` and `oidc_audience`.
- `xds_ingress_cidrs`: your local dataplane/operator public IP CIDR, for example `["1.2.3.4/32"]`.
- Secrets Manager ARNs for Flowplane KEK and PEM material.

Set the OIDC values from your identity provider:

```bash
export TF_VAR_oidc_issuer="https://your-issuer.example.com"   # OIDC issuer URL
export TF_VAR_oidc_audience="your-api-audience"               # expected JWT aud claim
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

Create the dataplane and issue a one-time cert response:

```bash
flowplane dataplane create edge-local --team <team>
flowplane --out .local/aws-dp-cert.json dataplane cert issue edge-local --team <team>
```

Write the PEM values to files:

```bash
jq -r '.data.certificate_pem' .local/aws-dp-cert.json > .local/aws-dp.crt
jq -r '.data.private_key_pem' .local/aws-dp-cert.json > .local/aws-dp.key
jq -r '.data.ca_certificate_pem' .local/aws-dp-cert.json > .local/aws-dp-client-chain-ca.crt
chmod 600 .local/aws-dp.key
```

`ca_certificate_pem` is the dataplane **client-chain CA** from the issue response. It is the CA the control plane trusts for the dataplane client certificate; it is not the CA Envoy uses to verify the control plane's xDS server certificate.

Write the xDS **server-trust CA** to a separate file. This is the CA bundle that validates the certificate served by `xds.getflowplane.io`, from the same PKI material that produced your xDS server certificate secret:

```bash
printf '%s' "$CP_XDS_SERVER_CA_PEM" > .local/aws-xds-server-ca.crt
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
