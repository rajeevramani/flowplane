TLS Test Fixtures
=================

This folder contains deterministic, self‑signed certificates used in E2E TLS/mTLS tests.

Content
- `ca.pem` / `ca.key`: Root CA
- `server.pem` / `server.key`: Server certificate (CN=localhost, SAN=DNS:localhost,IP:127.0.0.1)
- `client.pem` / `client.key`: Client certificate signed by the same CA
- `xds-server.pem` / `xds-server.key`: xDS gRPC server certificate
- `xds-client.pem` / `xds-client.key`: xDS client (Envoy) certificate

Reproducible generation (OpenSSL)

Note: The files checked in are already generated. To regenerate locally:

1) Root CA

    openssl req -x509 -newkey rsa:2048 -days 3650 -nodes \
      -subj "/CN=flowplane-e2e-ca" \
      -keyout ca.key -out ca.pem

2) Create `server.cnf` with SANs

    cat > server.cnf <<EOF
    [ req ]
    distinguished_name = req_distinguished_name
    x509_extensions = v3_req
    prompt = no
    [ req_distinguished_name ]
    CN = localhost
    [ v3_req ]
    subjectAltName = @alt_names
    [ alt_names ]
    DNS.1 = localhost
    IP.1 = 127.0.0.1
    EOF

3) Server cert

    openssl req -new -newkey rsa:2048 -nodes -keyout server.key \
      -subj "/CN=localhost" -out server.csr
    openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial \
      -days 3650 -extfile server.cnf -extensions v3_req -out server.pem

4) Client cert

    openssl req -new -newkey rsa:2048 -nodes -keyout client.key \
      -subj "/CN=e2e-client" -out client.csr
    openssl x509 -req -in client.csr -CA ca.pem -CAkey ca.key -CAcreateserial \
      -days 3650 -out client.pem

5) XDS server/client can reuse the same CA; generate separate keypairs as needed using steps similar to 3/4 with distinct CNs.

