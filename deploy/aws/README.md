# Flowplane AWS Secure Deployment

This OpenTofu module provisions a strict-secure AWS smoke environment for the Flowplane control
plane:

- ECS/Fargate control plane in private subnets.
- RDS PostgreSQL in private subnets.
- Public API through an ALB on HTTPS 443, forwarding to the task over HTTPS 8080.
- Public xDS through an NLB on TCP 18000, preserving mTLS through to the Flowplane process.
- NAT egress for private ECS tasks to reach external OIDC/JWKS providers.
- No `FLOWPLANE_API_INSECURE=true`.

The module intentionally does not manage Cloudflare DNS. Use the outputs to create records under
`getflowplane.io`.

## Prerequisites

- OpenTofu.
- AWS credentials for the target account.
- A Flowplane release image pushed to ECR in the selected region.
- An ACM certificate for the API hostname.
- Secrets Manager secrets containing:
  - `FLOWPLANE_SECRET_ENCRYPTION_KEY`
  - API backend TLS certificate PEM
  - API backend TLS private key PEM
  - xDS server certificate PEM
  - xDS server private key PEM
  - dataplane client CA certificate PEM
  - dataplane certificate issuer CA certificate PEM
  - dataplane certificate issuer CA private key PEM

The control-plane container receives PEM values as ECS secrets, writes them to `/tmp/flowplane/tls`,
and starts `flowplane serve` with file-path environment variables.

The module creates NAT egress by default because prod OIDC needs outbound HTTPS to the configured
issuer/JWKS provider. For the smoke environment it uses one NAT gateway by default; set
`single_nat_gateway = false` for one NAT gateway per AZ.

If the Secrets Manager secrets use a customer-managed KMS key, set `secret_kms_key_arns` so the ECS
task execution role can decrypt them.

## Variables

Create an ignored tfvars file, for example `deploy/aws/local.auto.tfvars`:

```hcl
aws_region = "us-east-1"
availability_zones = ["us-east-1a", "us-east-1b"]

control_plane_image = "<account>.dkr.ecr.us-east-1.amazonaws.com/flowplane:<tag>"

api_certificate_arn = "arn:aws:acm:us-east-1:<account>:certificate/..."

oidc_issuer   = "https://your-issuer.example.com"
oidc_audience = "your-api-audience"

xds_ingress_cidrs = ["<your-public-ip>/32"]

secret_encryption_key_secret_arn = "arn:aws:secretsmanager:..."
api_tls_cert_secret_arn          = "arn:aws:secretsmanager:..."
api_tls_key_secret_arn           = "arn:aws:secretsmanager:..."
xds_tls_cert_secret_arn          = "arn:aws:secretsmanager:..."
xds_tls_key_secret_arn           = "arn:aws:secretsmanager:..."
xds_tls_client_ca_secret_arn     = "arn:aws:secretsmanager:..."
cert_issuer_ca_cert_secret_arn   = "arn:aws:secretsmanager:..."
cert_issuer_ca_key_secret_arn    = "arn:aws:secretsmanager:..."
```

Set OIDC values from your identity provider. These are operator-provided deployment inputs, not
repo-local files:

```bash
export TF_VAR_oidc_issuer="https://your-issuer.example.com"
export TF_VAR_oidc_audience="your-api-audience"
export FLOWPLANE_OIDC_ISSUER="$TF_VAR_oidc_issuer"
export FLOWPLANE_OIDC_CLIENT_ID="<public-cli-client-id-from-your-idp>"
```

`TF_VAR_oidc_issuer` must match the issuer in accepted tokens. `TF_VAR_oidc_audience` must match
the Flowplane API audience configured in your IdP. For the full provider-neutral OIDC contract,
including CLI client, callback URL, device-code support, scopes, JWKS override, CA bundle, and
first-admin subject discovery, see
[`docs/how-to/configure-oidc-provider.md`](../../docs/how-to/configure-oidc-provider.md).

If you change `aws_region`, also change `availability_zones` to AZ names in that region. The module
keeps AZs explicit so planning does not require `ec2:DescribeAvailabilityZones`.

## Commands

```bash
tofu -chdir=deploy/aws init
tofu -chdir=deploy/aws validate
tofu -chdir=deploy/aws plan
tofu -chdir=deploy/aws apply
```

## DNS

Create these records in Cloudflare:

```text
cp.getflowplane.io  -> api_alb_dns_name
xds.getflowplane.io  -> xds_nlb_dns_name
```

Keep the xDS record DNS-only. Do not proxy xDS through Cloudflare for this test; it is raw TCP with
mTLS terminated by Flowplane.

## Bootstrap Token

The control plane is operator-seeded — it never generates or logs a bootstrap token, and an uninitialized non-dev instance with no token fails closed at startup. Generate a token, store it in Secrets Manager, and pass its ARN as `bootstrap_token_secret_arn`; the module injects it as `FLOWPLANE_BOOTSTRAP_TOKEN`:

```bash
BOOTSTRAP_TOKEN="$(openssl rand -hex 32)"
aws secretsmanager create-secret \
  --name /flowplane/prod/bootstrap-token \
  --secret-string "$BOOTSTRAP_TOKEN"
```

If the secret uses a customer-managed KMS key, add that key to `secret_kms_key_arns`. Use the same token to initialize the platform admin with the verified OIDC subject (`POST /api/v1/bootstrap/initialize`).

## Local Dataplane Smoke Test

After bootstrapping and logging in:

```bash
export FLOWPLANE_SERVER=https://cp.getflowplane.io
flowplane auth login --device-code \
  --issuer "$FLOWPLANE_OIDC_ISSUER" \
  --client-id "$FLOWPLANE_OIDC_CLIENT_ID" \
  --scope "openid email profile"

flowplane dataplane create edge-local --team <team>
flowplane --out .local/aws-dp-cert.json dataplane cert issue edge-local --team <team>

jq -r '.data.certificate_pem' .local/aws-dp-cert.json > .local/aws-dp.crt
jq -r '.data.private_key_pem' .local/aws-dp-cert.json > .local/aws-dp.key
jq -r '.data.ca_certificate_pem' .local/aws-dp-cert.json > .local/aws-dp-client-chain-ca.crt
chmod 600 .local/aws-dp.key

# `data.ca_certificate_pem` is the dataplane client-chain CA. Use a separate xDS
# server-trust CA bundle for `--ca-path`: the CA that validates xds.getflowplane.io.
printf '%s' "$CP_XDS_SERVER_CA_PEM" > .local/aws-xds-server-ca.crt

flowplane --out .local/aws-envoy.yaml dataplane bootstrap edge-local \
  --team <team> \
  --mode mtls \
  --xds-host xds.getflowplane.io \
  --xds-port 18000 \
  --cert-path "$PWD/.local/aws-dp.crt" \
  --key-path "$PWD/.local/aws-dp.key" \
  --ca-path "$PWD/.local/aws-xds-server-ca.crt"
```

Then run Envoy locally with the generated bootstrap and apply a simple route to verify xDS delivery.
