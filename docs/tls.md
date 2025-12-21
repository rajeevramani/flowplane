# TLS Configuration

Flowplane supports TLS in multiple areas:

1. **Admin API** - HTTPS for the REST API and Web UI
2. **xDS Server** - TLS/mTLS between control plane and Envoy proxies
3. **Vault PKI** - Certificate generation for proxy authentication
4. **Listener TLS** - TLS termination on Envoy listeners

For delivering TLS certificates and secrets to Envoy via SDS, see [Secrets and SDS](secrets-sds.md).

## Admin API TLS

Enable HTTPS for the REST API (`/api/v1/...`) and Web UI.

### Environment Variables

| Variable | Description |
|----------|-------------|
| `FLOWPLANE_API_TLS_ENABLED` | Set to `true`, `1`, `yes`, or `on` to enable HTTPS |
| `FLOWPLANE_API_TLS_CERT_PATH` | Path to PEM-encoded server certificate |
| `FLOWPLANE_API_TLS_KEY_PATH` | Path to PEM-encoded private key |
| `FLOWPLANE_API_TLS_CHAIN_PATH` | Optional: PEM bundle of intermediate certificates |

### Example

```bash
export FLOWPLANE_API_TLS_ENABLED=true
export FLOWPLANE_API_TLS_CERT_PATH=/etc/flowplane/certs/api-cert.pem
export FLOWPLANE_API_TLS_KEY_PATH=/etc/flowplane/certs/api-key.pem
```

When TLS is enabled, Flowplane validates certificate files at startup. Invalid or mismatched certificates abort launch with a descriptive error.

## xDS Server TLS

Secure the gRPC connection between Flowplane and Envoy proxies.

### Server TLS (One-Way)

Encrypt xDS traffic without client authentication:

| Variable | Description |
|----------|-------------|
| `FLOWPLANE_XDS_TLS_CERT_PATH` | Path to server certificate |
| `FLOWPLANE_XDS_TLS_KEY_PATH` | Path to server private key |

### Mutual TLS (mTLS)

Authenticate Envoy proxies using client certificates:

| Variable | Description |
|----------|-------------|
| `FLOWPLANE_XDS_TLS_CERT_PATH` | Path to server certificate |
| `FLOWPLANE_XDS_TLS_KEY_PATH` | Path to server private key |
| `FLOWPLANE_XDS_TLS_CLIENT_CA_PATH` | CA certificate for verifying client certificates |
| `FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT` | Require client certificates (default: `true` when TLS enabled) |

### Example

```bash
# xDS server TLS
export FLOWPLANE_XDS_TLS_CERT_PATH=/etc/flowplane/certs/xds-server.pem
export FLOWPLANE_XDS_TLS_KEY_PATH=/etc/flowplane/certs/xds-server.key

# Enable mTLS
export FLOWPLANE_XDS_TLS_CLIENT_CA_PATH=/etc/flowplane/certs/xds-ca.pem
```

When mTLS is enabled:
- Team identity is extracted from the client certificate's SPIFFE URI
- Node metadata team is logged but not used for authorization
- Connections without valid certificates are rejected

## Vault PKI Integration

Flowplane integrates with HashiCorp Vault's PKI secrets engine to generate mTLS certificates for Envoy proxies.

### Configuration

| Variable | Description |
|----------|-------------|
| `FLOWPLANE_VAULT_PKI_MOUNT_PATH` | PKI engine mount path (enables mTLS if set) |
| `FLOWPLANE_VAULT_PKI_ROLE` | PKI role name (default: `envoy-proxy`) |
| `FLOWPLANE_SPIFFE_TRUST_DOMAIN` | SPIFFE trust domain (default: `flowplane.local`) |

### SPIFFE URI Format

Certificates include a SPIFFE URI in the Subject Alternative Name:

```
spiffe://{trust_domain}/team/{team}/proxy/{proxy_id}
```

Example: `spiffe://flowplane.local/team/payments/proxy/envoy-1`

### Generating Proxy Certificates

Use the API to generate certificates for Envoy proxies:

```bash
curl -X POST "http://localhost:8080/api/v1/teams/payments/proxy-certificates" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"proxyId": "envoy-1"}'
```

Response includes:
- `certificate` - PEM-encoded X.509 certificate
- `privateKey` - PEM-encoded private key (only returned at generation)
- `caChain` - PEM-encoded CA chain
- `spiffeUri` - SPIFFE identity embedded in certificate
- `expiresAt` - Certificate expiration timestamp

### mTLS Status

Check mTLS configuration status:

```bash
curl -H "Authorization: Bearer $TOKEN" \
  http://localhost:8080/api/v1/mtls/status
```

## Listener TLS Configuration

Configure TLS termination on Envoy listener filter chains:

```json
{
  "name": "https-listener",
  "address": "0.0.0.0",
  "port": 443,
  "team": "payments",
  "filterChains": [{
    "name": "default",
    "tlsContext": {
      "certChainFile": "/etc/envoy/certs/server-cert.pem",
      "privateKeyFile": "/etc/envoy/certs/server-key.pem",
      "caCertFile": "/etc/envoy/certs/client-ca.pem",
      "requireClientCertificate": true
    },
    "filters": [...]
  }]
}
```

| Field | Description |
|-------|-------------|
| `certChainFile` | Path to server certificate chain |
| `privateKeyFile` | Path to server private key |
| `caCertFile` | Path to CA for client certificate verification |
| `requireClientCertificate` | Enable mTLS (require client certs) |

For delivering TLS certificates dynamically via SDS, see [Secrets and SDS](secrets-sds.md).

## High Availability Considerations

Since Flowplane acts as the xDS/SDS server, it becomes a dependency for configuration and secret delivery.

### What happens when CP connection breaks

**Existing Envoy instances** continue working with cached configuration:
- Envoy caches all xDS resources (including secrets) locally
- Existing secrets remain usable until they expire or need rotation
- Traffic continues flowing

**What breaks:**
- New Envoy instances can't bootstrap (no configuration or secrets)
- Secret rotations don't propagate
- New secrets can't be delivered
- Configuration updates stop

### Mitigations

| Approach | Description |
|----------|-------------|
| **CP High Availability** | Run multiple CP instances behind load balancer |
| **Envoy xDS caching** | Envoy persists xDS config to survive restarts |
| **Vault Agent sidecar** | Alternative: Vault Agent writes to filesystem, Envoy watches files |
| **Longer TTLs** | Use longer-lived certificates/secrets to survive outages |

### Architecture tradeoffs

| Architecture | Pros | Cons |
|--------------|------|------|
| **CP intermediary** (current) | Unified management, audit, team isolation | CP is dependency |
| **Vault Agent + filesystem** | No CP dependency for secrets | No dynamic SDS, manual rotation |
| **Direct Vault SDS** | N/A - Envoy doesn't support this | - |

The CP intermediary pattern is standard across control planes (Istio, Gloo, etc.).

## Troubleshooting

### Admin API TLS

| Error | Solution |
|-------|----------|
| Certificate and private key do not match | Ensure key corresponds to certificate; regenerate if needed |
| Client trust errors | Supply intermediate chain via `FLOWPLANE_API_TLS_CHAIN_PATH` |
| HTTPS not working | Check startup logs to confirm TLS is enabled |

### xDS mTLS

| Error | Solution |
|-------|----------|
| mTLS disabled warning at startup | Set `FLOWPLANE_VAULT_PKI_MOUNT_PATH` to enable |
| Client certificate rejected | Verify certificate is signed by configured CA |
| SPIFFE URI not extracted | Check certificate contains correct SAN format |

### Listener TLS

| Error | Solution |
|-------|----------|
| Certificate file not found | Verify path is accessible to Envoy |
| TLS handshake failure | Check certificate chain is complete |
| Client cert rejected | Verify client cert is signed by configured CA |

For SDS-related troubleshooting, see [Secrets and SDS - Troubleshooting](secrets-sds.md#troubleshooting).
