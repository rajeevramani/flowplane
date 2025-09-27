# Data Model: TLS Bring-Your-Own-Cert MVP

## Core Entities

### TlsConfig
**Purpose**: Represents TLS configuration settings for the admin API server
**Lifecycle**: Loaded at startup → Validated → Applied to server

**Attributes**:
- `enabled: bool` - Whether TLS termination is enabled (default: false)
- `cert_path: Option<PathBuf>` - Path to PEM certificate file
- `key_path: Option<PathBuf>` - Path to PEM private key file
- `chain_path: Option<PathBuf>` - Optional path to certificate chain file
- `bind_address: String` - IP address to bind (default: "0.0.0.0")
- `port: u16` - Port to listen on when TLS enabled

**Validation Rules**:
- If enabled = true, cert_path and key_path must be Some
- All file paths must exist and be readable
- Certificate and key files must be valid PEM format
- Certificate and key must form a valid pair
- Port must be in valid range (1-65535)

**State Transitions**:
```
Uninitialized → Loaded (from env vars)
Loaded → Validated (file checks, PEM parsing)
Validated → Applied (server configuration)
```

### CertificateBundle
**Purpose**: Represents loaded and validated certificate materials
**Lifetime**: Application startup to shutdown

**Attributes**:
- `certificate: Vec<u8>` - Parsed certificate chain
- `private_key: Vec<u8>` - Parsed private key
- `cert_info: CertificateInfo` - Extracted certificate metadata
- `validation_time: SystemTime` - When validation occurred

**Methods**:
- `load_from_files(cert_path, key_path, chain_path) -> Result<Self, TlsError>`
- `validate_pair() -> Result<(), TlsError>` - Verify cert/key match
- `is_expired() -> bool` - Check certificate expiration

### CertificateInfo
**Purpose**: Metadata extracted from certificate for logging and validation
**Lifetime**: Same as CertificateBundle

**Attributes**:
- `subject: String` - Certificate subject (CN, O, etc.)
- `issuer: String` - Certificate issuer
- `not_before: SystemTime` - Certificate validity start
- `not_after: SystemTime` - Certificate validity end
- `serial_number: String` - Certificate serial number
- `fingerprint: String` - SHA-256 fingerprint

### TlsError
**Purpose**: Structured error types for TLS configuration and validation failures
**Usage**: Error handling throughout TLS setup process

**Variants**:
- `ConfigurationError(String)` - Invalid configuration (missing files, bad paths)
- `CertificateError(String)` - Certificate parsing or validation failure
- `KeyError(String)` - Private key parsing or validation failure
- `MismatchError` - Certificate and private key don't match
- `ExpirationError(SystemTime)` - Certificate has expired
- `PermissionError(String)` - File access permission denied
- `IoError(std::io::Error)` - File system I/O error

### ServerConfig
**Purpose**: Complete server configuration including TLS settings
**Lifecycle**: Built from TlsConfig and passed to Axum server

**Attributes**:
- `tls_enabled: bool` - Whether to use HTTPS
- `bind_address: SocketAddr` - Complete socket address
- `tls_acceptor: Option<TlsAcceptor>` - rustls acceptor if TLS enabled
- `certificate_info: Option<CertificateInfo>` - Certificate metadata for logging

**Methods**:
- `from_tls_config(tls_config: TlsConfig) -> Result<Self, TlsError>`
- `create_server() -> Result<Server, TlsError>` - Create Axum server instance

## Configuration Schema

### Environment Variables
```bash
# TLS enablement toggle
FLOWPLANE_TLS_ENABLED=true|false          # Default: false

# Certificate file paths (required if TLS enabled)
FLOWPLANE_TLS_CERT_PATH=/path/to/cert.pem
FLOWPLANE_TLS_KEY_PATH=/path/to/key.pem
FLOWPLANE_TLS_CHAIN_PATH=/path/to/chain.pem  # Optional

# Server binding (optional overrides)
FLOWPLANE_TLS_BIND_ADDRESS=0.0.0.0         # Default: 0.0.0.0
FLOWPLANE_TLS_PORT=8443                    # Default: 8443 when TLS enabled
```

### Configuration File Support (Optional)
```toml
[tls]
enabled = true
cert_path = "/etc/flowplane/tls/cert.pem"
key_path = "/etc/flowplane/tls/key.pem"
chain_path = "/etc/flowplane/tls/chain.pem"
bind_address = "0.0.0.0"
port = 8443
```

## Validation & Business Rules

### Certificate Validation Rules
1. Certificate files must be readable PEM format
2. Private key must be readable PEM format (RSA, ECDSA, or Ed25519)
3. Certificate and private key must cryptographically match
4. Certificate must not be expired at startup time
5. Certificate chain (if provided) must form valid chain to certificate

### Configuration Validation Rules
1. TLS cannot be enabled without both cert_path and key_path
2. File paths must be absolute paths to existing files
3. Certificate files must have appropriate read permissions
4. Port must not conflict with existing xDS listener port
5. Bind address must be valid IPv4 or IPv6 address

### Security Constraints
1. Private key contents never logged or exposed in error messages
2. Certificate validation failure must prevent server startup
3. TLS configuration parsing errors must be descriptive for operators
4. File permission errors must indicate specific permission requirements
5. Certificate expiration warnings should include renewal guidance

### Compatibility Requirements
1. When TLS disabled, server behavior identical to current HTTP mode
2. Personal access token authentication works identically over HTTPS
3. All existing API endpoints remain accessible via HTTPS
4. Audit logging includes TLS connection information
5. Existing xDS mTLS communication remains unchanged

## Usage Patterns

### Startup Validation Flow
```
1. Load TLS configuration from environment variables
2. If TLS enabled: validate all required paths are provided
3. Read and parse certificate files (fail fast on errors)
4. Validate certificate/key pair matching
5. Check certificate expiration (warn if expires within 30 days)
6. Create TLS acceptor with validated materials
7. Bind server to configured address/port
8. Log successful TLS initialization with certificate info
```

### Certificate Information Logging
```
1. Log certificate subject, issuer, and expiration on startup
2. Include certificate fingerprint for verification
3. Log TLS protocol versions and cipher suites in use
4. Audit log TLS connection attempts and successes
5. Never log private key information or TLS session keys
```

### Error Handling Patterns
```
1. Configuration errors: fail fast with specific remediation steps
2. File access errors: include file path and permission requirements
3. Certificate parsing errors: indicate PEM format requirements
4. Key/cert mismatch: provide fingerprint comparison for debugging
5. Expiration errors: include current time and certificate validity period
```

### Deployment Integration
```
1. Docker: mount certificate files as volumes, set env vars
2. Kubernetes: use secrets for certificates, configmap for config
3. Systemd: set env vars in service file, secure file permissions
4. Development: support self-signed certificates with clear warnings
5. Production: validate certificate chain to trusted CA
```