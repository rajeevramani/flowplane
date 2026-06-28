# How-to: evaluate a production-shaped platform setup

> Audience: platform-engineers, operators · Status: stable

This runbook is the platform evaluation spine. It links the canonical setup and reference pages in the order an outside platform team needs them: control plane, identity, bootstrap, tenant/team, dataplane mTLS, health/readiness/xDS, and observability.

Use this when you are evaluating whether Flowplane can be deployed, governed, and delegated inside an organization. It does not replace the canonical task pages it links to.

## Prerequisites

You need:

- a Flowplane release image or binary for the control plane;
- PostgreSQL and TLS material for the API and xDS listeners;
- OIDC provider values from [Configure an OIDC provider](configure-oidc-provider.md);
- a high-entropy bootstrap token delivered by `FLOWPLANE_BOOTSTRAP_TOKEN_FILE` or `FLOWPLANE_BOOTSTRAP_TOKEN`;
- a `flowplane` CLI installed on the operator workstation;
- a dataplane host that can run Envoy and `fp-agent`.

Use placeholders in examples until you substitute your own values. Do not put real tokens, private keys, or certificate PEM bodies in public docs, issue comments, or shell history.

## 1. Start the control plane in production shape

Follow [Production Readiness](production-readiness.md) for the control-plane environment. At minimum the first boot needs:

- database URL;
- secret-encryption key material;
- API TLS certificate and key;
- xDS server certificate, key, and dataplane client CA;
- OIDC issuer and audience;
- bootstrap token file or token value.

After migration and startup, verify the root operational endpoints:

```bash
curl -fsS https://cp.example/healthz
curl -fsS https://cp.example/readyz
curl -fsS https://cp.example/metrics | head
```

`/healthz`, `/readyz`, and `/metrics` are public operational endpoints. They prove the process is reachable; they do not prove tenant access or xDS delivery.

## 2. Connect OIDC and bootstrap the first platform admin

Use [Configure an OIDC provider](configure-oidc-provider.md) to set the issuer, audience, optional JWKS override, CLI client, callback URL, device-code support, scopes, and optional CA bundle.

Then use [Bootstrap the first platform admin](bootstrap-platform.md). The bootstrap call consumes the one-shot operator-supplied token and creates the platform org and first platform admin.

Verify the admin identity:

```bash
flowplane auth login --device-code \
  --issuer https://issuer.example \
  --client-id flowplane-cli

flowplane config set-context platform \
  --server https://cp.example

flowplane config use-context platform
flowplane auth whoami
```

The platform org is governance-only. It cannot host tenant teams, dataplanes, or gateway resources.

## 3. Create a tenant org and team

Follow [Create a tenant org and a team](create-tenant-org-and-team.md). The important boundary is:

- platform admin can create tenant orgs and seed the first owner;
- tenant teams own dataplanes and gateway resources;
- platform-admin status is not a tenant bypass.

Save an operator context for the tenant team:

```bash
flowplane config set-context edgeco-payments \
  --server https://cp.example \
  --org edgeco \
  --team payments

flowplane config use-context edgeco-payments
flowplane team list --org edgeco
```

## 4. Register and connect a dataplane over mTLS

Use [Register a dataplane and connect its agent over mTLS](register-dataplane-mtls.md). That page remains the canonical mTLS dataplane runbook.

The expected direction is outbound from the dataplane to the control plane:

- Envoy opens ADS/SDS streams to the control-plane xDS listener.
- `fp-agent` reports dataplane diagnostics to the control plane.
- The control plane does not call Envoy admin as a product path.
- Envoy admin remains local to the dataplane and is a manual diagnostic surface.

Verify the dataplane:

```bash
curl -fsS http://127.0.0.1:19902/healthz

flowplane dataplane get edge-gateway-1 --team payments
flowplane stats overview --team payments
flowplane ops xds status --team payments
flowplane ops xds nacks --team payments
```

`dataplane get` should show an advancing heartbeat once `fp-agent` reports. `stats overview` should count the dataplane as live. `ops xds status` / `ops xds nacks` should show whether Envoy accepted or rejected the pushed xDS resources.

## 5. Verify observability

Confirm metrics are scrapeable from the control plane:

```bash
curl -fsS https://cp.example/metrics | grep -E 'fp_api_requests_total|fp_xds_ads_streams_opened_total|fp_db_pool_'
```

Use [Observability Alert Pack](../reference/observability-alerts.md) as the baseline metric inventory, alert set, and dashboard panel list. Keep alert labels low-cardinality; do not add team, org, dataplane, route, budget, tool, or agent labels unless you have accepted the cardinality cost.

## 6. Decide what the platform owns

At the end of this evaluation, the platform team owns:

- control-plane deployment, database, TLS, OIDC, bootstrap, backup/restore, and observability;
- tenant org creation and first-owner handoff;
- dataplane registration policy and certificate issuance policy;
- production xDS and diagnostics network paths.

API teams should receive a control-plane URL, org/team names, granted permissions, and a CLI login path. The API-team self-service flow is covered separately by the API-team onboarding guide.

## References

- [Production Readiness](production-readiness.md)
- [Configure an OIDC provider](configure-oidc-provider.md)
- [Bootstrap the first platform admin](bootstrap-platform.md)
- [Create a tenant org and a team](create-tenant-org-and-team.md)
- [Register a dataplane and connect its agent over mTLS](register-dataplane-mtls.md)
- [Configuration reference](../reference/configuration.md)
- [CLI reference](../reference/cli.md)
- [REST API reference](../reference/rest-api.md)
- [Observability Alert Pack](../reference/observability-alerts.md)
