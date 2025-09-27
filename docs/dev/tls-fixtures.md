# TLS Test Fixture Regeneration

Flowplane ships a self-signed certificate and key under `tests/fixtures/` so integration tests can bring up an HTTPS listener without hitting the network. Regenerate them whenever the subject or SANs need to change, or if the fixtures are close to expiring.

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
- Verify `cargo test --tests` still succeeds. The HTTPS integration suite trusts the regenerated certificate directly, so any SAN mismatch will surface here.
- Commit the updated `valid_cert.pem` and `valid_key.pem` files together. They must stay in sync.
- If the private key format changes (e.g., to PKCS#1), update `load_certificate_bundle` test coverage accordingly.
```
