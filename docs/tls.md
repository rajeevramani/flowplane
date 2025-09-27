# Bring Your Own Certificate for the Admin API

Flowplane’s control plane can expose the admin API over HTTPS by loading certificates that you provision. This guide explains how to turn TLS on, choose a certificate source, and keep certificates current in production and development.

## Enabling TLS

Set the following environment variables (or their CLI equivalents) when starting the control plane:

- `FLOWPLANE_API_TLS_ENABLED` – set to `true`, `1`, `yes`, or `on` to enable HTTPS. Defaults to HTTP when unset.
- `FLOWPLANE_API_TLS_CERT_PATH` – absolute or relative path to a PEM-encoded leaf certificate.
- `FLOWPLANE_API_TLS_KEY_PATH` – path to the matching unencrypted PEM private key.
- `FLOWPLANE_API_TLS_CHAIN_PATH` *(optional)* – PEM bundle of intermediate certificates if your issuer is not directly trusted by clients.

When TLS is enabled, Flowplane validates the files at startup (readability, PEM structure, expiry, and key/certificate match). Any failure aborts launch with a descriptive configuration error so you can correct it before exposing an insecure endpoint.

## Choosing a Certificate Source

### ACME Providers (e.g., Let’s Encrypt)
- **What you get**: Publicly trusted certificates automated via ACME challenges.
- **How to use**: Issue certificates with tools such as Certbot or Lego, then mount the resulting `fullchain.pem` and `privkey.pem` into the control plane container or host. Point `FLOWPLANE_API_TLS_CERT_PATH` to the leaf or chain file and `FLOWPLANE_API_TLS_KEY_PATH` to the key.
- **Rotation cadence**: ACME certificates typically expire every 90 days. Automate renewal with a cron job or systemd timer, copy the fresh files into place, and restart Flowplane to load them.

### Corporate PKI
- **What you get**: Certificates signed by your internal CA with custom policies (longer validity, restricted hostnames, smart card backing, etc.).
- **How to use**: Request a server certificate for the control plane hostname through your corporate CA workflow. Export the certificate, private key, and any intermediate chain in PEM format and reference them with the environment variables above.
- **Rotation cadence**: Follows your organization’s security policy (often 1–12 months). Schedule renewals ahead of expiry and restart Flowplane after updating the files. Coordinate with endpoint monitoring so alerts fire before the deadline.

### Self-Signed for Local Development
- **What you get**: Quick certificates for testing without external dependencies.
- **How to use**: Generate a certificate using the OpenSSL config at `tests/fixtures/dev_cert.cnf` (see `docs/dev/tls-fixtures.md`). The files land under `tests/fixtures/` but stay untracked so you can regenerate freely.
- **Client trust**: Development clients may need to trust the generated certificate manually (curl `--cacert`, browser trust dialog, etc.).

## Operational Checklist

- Store certificate and key files with permissions limited to the Flowplane process owner.
- Monitor expiry dates (`journalctl` logs include the subject and `not_after` timestamp at startup) and integrate them into your observability stack.
- Restart the control plane after replacing certificates; hot reload is not part of the MVP.
- If you need mutual TLS between users and the admin API, track it as a follow-up feature—this release only handles server-side TLS.

## Troubleshooting

- **Startup fails with “certificate and private key do not match”**: Ensure the key corresponds to the issued certificate; regenerate or re-export as needed.
- **Clients see certificate trust errors**: Confirm you supplied the intermediate chain via `FLOWPLANE_API_TLS_CHAIN_PATH` or that the client trusts your corporate root CA.
- **Connectivity works over HTTP but not HTTPS**: Double-check that TLS is actually enabled (check startup logs) and that load balancers or proxies forward traffic to the TLS port.

For additional context on upcoming enhancements (automatic rotation, richer observability), see the open items in `specs/002-cp-tls-enable/spec.md`.
