# Fly.io deployment packaging

This directory is optional provider packaging for Flowplane. Fly is not product identity, tenant identity, or dataplane identity. OIDC establishes user identity; Flowplane authorization establishes tenant access; xDS client certificates and the Flowplane certificate registry establish dataplane identity.

## TLS boundary

The public service is raw TCP passthrough on port 443. Fly Proxy does not terminate TLS because the service port has no `tls` or `http` handler. Flowplane terminates API TLS on port 8080 using `FLOWPLANE_API_TLS_CERT` and `FLOWPLANE_API_TLS_KEY`. `FLOWPLANE_API_INSECURE` remains false. The API certificate must chain to a client-trusted CA and contain the public API hostname in its SAN.

For this qualification, the frozen public API hostname is `cp.getflowplane.io`. Its existing stale AWS DNS record is replaced only after the Fly endpoint and matching certificate are ready. Before deployment, render `API_TLS_SERVER_NAME_REQUIRED` in a private copy of `fly.toml` to `cp.getflowplane.io`. Do not deploy the checked-in manifest unchanged. The HTTPS readiness check verifies the same hostname and must never use `tls_skip_verify=true`.

xDS port 18000 is not a Fly public service. Tailscale userspace networking exposes a tailnet-only raw TCP forwarder from the node's port 18000 to Flowplane's loopback port 18000. Tailscale does not terminate TLS: Flowplane terminates xDS mTLS and authorizes the dataplane client certificate.

`rls.fly.toml` packages `flowplane-rls` as a separate private app. Its CP-facing admin listener uses HTTPS plus a secret-backed bearer over Fly 6PN. Its Envoy-facing gRPC listener remains loopback and is exposed only through a tailnet raw TCP forwarder; `flowplane-rls` terminates mandatory mTLS. The RLS manifest declares no public Fly service.

## Runtime inputs

Set these as ordinary Fly secrets; do not add their values to `fly.toml`:

- `FLOWPLANE_DATABASE_URL`
- `FLOWPLANE_OIDC_ISSUER`
- `FLOWPLANE_OIDC_AUDIENCE`
- `FLOWPLANE_SECRET_ENCRYPTION_KEY`
- `FLOWPLANE_RLS_ADMIN_URL`
- `FLOWPLANE_RLS_GRPC_URL`
- `FLOWPLANE_DATAPLANE_TLS_CERT`, `_KEY`, and `_CLIENT_CA` as paths that exist on each
  dataplane VM; these values are emitted into Envoy configuration and are not CP-local files

Private files use Fly `[[files]]` bindings. Fly expects the associated secret values to be base64-encoded. The manifest declares file bindings for API TLS, xDS mTLS, certificate issuance, bootstrap, RLS administration, and the Tailscale auth key. Never print or commit those values.

The entrypoint applies database migrations before serving. Fly release-command arguments bypass
machine startup and replace the entrypoint process, so a release migration exits instead of
starting a second control plane. The entrypoint does not print the environment or secret-bearing
variables. The bootstrap token is one-shot and should be removed after bootstrap succeeds.

## Qualification limits

The Gate 2 topology is production-shaped, not production-grade. A single Machine and single-node PostgreSQL do not prove HA, SLOs, capacity, provider support, or production operational ownership. xDS, PostgreSQL, RLS, Envoy admin, and agent health ports must remain absent from all public Fly service declarations.
