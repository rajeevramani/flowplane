# TLS Test Fixture Regeneration

Flowplane no longer checks TLS private keys into the repository. Tests generate ephemeral key material on the fly, so you only need to create fixtures locally when running manual experiments or developing against Envoy.

## Prerequisites
- OpenSSL 1.1.1 or newer available on your `$PATH`
- Working directory: repository root

## Regenerate the Certificate & Key

```bash
openssl req \
  -config tests/fixtures/dev_cert.cnf \
  -newkey rsa:2048 \
  -nodes \
  -keyout tests/fixtures/valid_key.pem \
  -x509 \
  -days 90 \
  -out tests/fixtures/valid_cert.pem
```

The config file at `tests/fixtures/dev_cert.cnf` pins the subject and SANs so localhost/127.0.0.1 both work. Adjust it if tests need additional hostnames.

## Post-Regeneration Checklist
- Keep the generated `valid_cert.pem` and `valid_key.pem` files out of version control. `.gitignore` already blocks them, but double-check `git status` before committing.
- Verify any manual workflows that depend on the files (for example local Envoy runs) before deleting the temporary fixtures.
- If you change the private key format (e.g., to PKCS#1), update `load_certificate_bundle` test coverage accordingly.
