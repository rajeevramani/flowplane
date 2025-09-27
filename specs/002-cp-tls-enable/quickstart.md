# Quickstart: TLS Bring-Your-Own-Cert MVP

## Overview
This guide walks through enabling TLS termination on Flowplane admin APIs using your own certificates.

## Prerequisites
- Flowplane control plane running
- Valid TLS certificate and private key files in PEM format
- Certificate files readable by Flowplane process

## Step 1: Prepare Certificate Files

### 1.1 Verify Certificate Files
```bash
# Check certificate file format
openssl x509 -in /etc/flowplane/tls/cert.pem -text -noout

# Check private key file format
openssl rsa -in /etc/flowplane/tls/key.pem -check -noout

# Verify certificate and key match
openssl x509 -noout -modulus -in /etc/flowplane/tls/cert.pem | openssl md5
openssl rsa -noout -modulus -in /etc/flowplane/tls/key.pem | openssl md5
# The MD5 hashes should match
```

### 1.2 Set File Permissions
```bash
# Secure file permissions
chmod 644 /etc/flowplane/tls/cert.pem
chmod 600 /etc/flowplane/tls/key.pem
chmod 644 /etc/flowplane/tls/chain.pem  # if using certificate chain

# Verify Flowplane can read the files
sudo -u flowplane cat /etc/flowplane/tls/cert.pem > /dev/null
sudo -u flowplane cat /etc/flowplane/tls/key.pem > /dev/null
```

## Step 2: Configure TLS Environment Variables

### 2.1 Basic TLS Configuration
```bash
# Enable TLS termination
export FLOWPLANE_TLS_ENABLED=true

# Specify certificate file paths
export FLOWPLANE_TLS_CERT_PATH=/etc/flowplane/tls/cert.pem
export FLOWPLANE_TLS_KEY_PATH=/etc/flowplane/tls/key.pem

# Optional: certificate chain for intermediate CAs
export FLOWPLANE_TLS_CHAIN_PATH=/etc/flowplane/tls/chain.pem

# Optional: customize bind address and port
export FLOWPLANE_TLS_BIND_ADDRESS=0.0.0.0
export FLOWPLANE_TLS_PORT=8443
```

### 2.2 Start Flowplane with TLS
```bash
# Start with TLS configuration
FLOWPLANE_TLS_ENABLED=true \
FLOWPLANE_TLS_CERT_PATH=/etc/flowplane/tls/cert.pem \
FLOWPLANE_TLS_KEY_PATH=/etc/flowplane/tls/key.pem \
cargo run --bin flowplane

# Expected output:
# [INFO] TLS configuration loaded successfully
# [INFO] Certificate: CN=api.example.com, expires 2025-12-31T23:59:59Z
# [INFO] Server listening on https://0.0.0.0:8443
```

## Step 3: Verify TLS Operation

### 3.1 Test HTTPS Health Check
```bash
# Test health endpoint over HTTPS
curl -k https://localhost:8443/health

# Expected response:
# {
#   "status": "healthy",
#   "timestamp": "2025-09-27T10:00:00Z",
#   "tls_enabled": true,
#   "server_version": "0.1.0"
# }
```

### 3.2 Test Authenticated API Access
```bash
# Use existing personal access token over HTTPS
export FLOWPLANE_TOKEN="fp_your_token_here"

curl -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  https://localhost:8443/api/v1/clusters

# Authentication should work identically to HTTP mode
```

### 3.3 Verify Certificate Details
```bash
# Check certificate served by server
openssl s_client -connect localhost:8443 -servername api.example.com < /dev/null

# Look for certificate chain and verify:
# - Certificate matches your file
# - No certificate errors
# - TLS handshake successful
```

## Step 4: Production Deployment Examples

### 4.1 Docker Compose Configuration
```yaml
# docker-compose.yml
version: '3.8'
services:
  flowplane:
    image: flowplane:latest
    ports:
      - "8443:8443"
    environment:
      - FLOWPLANE_TLS_ENABLED=true
      - FLOWPLANE_TLS_CERT_PATH=/etc/tls/cert.pem
      - FLOWPLANE_TLS_KEY_PATH=/etc/tls/key.pem
      - FLOWPLANE_TLS_CHAIN_PATH=/etc/tls/chain.pem
      - FLOWPLANE_TLS_PORT=8443
    volumes:
      - ./tls:/etc/tls:ro
      - ./data:/data
    restart: unless-stopped
```

### 4.2 Systemd Service Configuration
```ini
# /etc/systemd/system/flowplane.service
[Unit]
Description=Flowplane Control Plane
After=network.target

[Service]
Type=exec
User=flowplane
Group=flowplane
WorkingDirectory=/opt/flowplane
ExecStart=/opt/flowplane/bin/flowplane
Environment=FLOWPLANE_TLS_ENABLED=true
Environment=FLOWPLANE_TLS_CERT_PATH=/etc/flowplane/tls/cert.pem
Environment=FLOWPLANE_TLS_KEY_PATH=/etc/flowplane/tls/key.pem
Environment=FLOWPLANE_TLS_CHAIN_PATH=/etc/flowplane/tls/chain.pem
Environment=FLOWPLANE_TLS_PORT=8443
Environment=FLOWPLANE_DATABASE_URL=sqlite:///var/lib/flowplane/flowplane.db
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

### 4.3 Kubernetes Deployment
```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: flowplane
spec:
  replicas: 1
  selector:
    matchLabels:
      app: flowplane
  template:
    metadata:
      labels:
        app: flowplane
    spec:
      containers:
      - name: flowplane
        image: flowplane:latest
        ports:
        - containerPort: 8443
        env:
        - name: FLOWPLANE_TLS_ENABLED
          value: "true"
        - name: FLOWPLANE_TLS_CERT_PATH
          value: "/etc/tls/cert.pem"
        - name: FLOWPLANE_TLS_KEY_PATH
          value: "/etc/tls/key.pem"
        - name: FLOWPLANE_TLS_CHAIN_PATH
          value: "/etc/tls/chain.pem"
        volumeMounts:
        - name: tls-certs
          mountPath: /etc/tls
          readOnly: true
      volumes:
      - name: tls-certs
        secret:
          secretName: flowplane-tls
---
apiVersion: v1
kind: Secret
metadata:
  name: flowplane-tls
type: Opaque
data:
  cert.pem: LS0tLS1CRUdJTi... # base64 encoded certificate
  key.pem: LS0tLS1CRUdJTi...  # base64 encoded private key
  chain.pem: LS0tLS1CRUdJTi... # base64 encoded chain
```

## Step 5: Certificate Source Examples

### 5.1 Let's Encrypt (Certbot)
```bash
# Install certbot
sudo apt-get install certbot

# Obtain certificate (standalone mode)
sudo certbot certonly --standalone \
  --email admin@example.com \
  --agree-tos \
  --no-eff-email \
  -d api.example.com

# Copy certificates to Flowplane directory
sudo cp /etc/letsencrypt/live/api.example.com/fullchain.pem /etc/flowplane/tls/cert.pem
sudo cp /etc/letsencrypt/live/api.example.com/privkey.pem /etc/flowplane/tls/key.pem
sudo cp /etc/letsencrypt/live/api.example.com/chain.pem /etc/flowplane/tls/chain.pem

# Set up automatic renewal
echo "0 12 * * * /usr/bin/certbot renew --quiet && systemctl restart flowplane" | sudo crontab -
```

### 5.2 Self-Signed Certificate (Development)
```bash
# Generate self-signed certificate for development
openssl req -x509 -newkey rsa:4096 -keyout key.pem -out cert.pem -days 365 -nodes \
  -subj "/CN=localhost/O=Development/C=US"

# Copy to Flowplane directory
sudo mkdir -p /etc/flowplane/tls
sudo cp cert.pem /etc/flowplane/tls/
sudo cp key.pem /etc/flowplane/tls/
sudo chmod 644 /etc/flowplane/tls/cert.pem
sudo chmod 600 /etc/flowplane/tls/key.pem
```

### 5.3 Corporate PKI Certificate
```bash
# Request certificate from corporate CA (example process)
# 1. Generate certificate signing request (CSR)
openssl req -new -newkey rsa:4096 -nodes \
  -keyout flowplane.key \
  -out flowplane.csr \
  -subj "/CN=api.internal.company.com/O=Company Name/C=US"

# 2. Submit CSR to corporate CA and receive certificate files
# 3. Copy received files to Flowplane directory
sudo cp flowplane.crt /etc/flowplane/tls/cert.pem
sudo cp flowplane.key /etc/flowplane/tls/key.pem
sudo cp corporate-ca-chain.crt /etc/flowplane/tls/chain.pem
```

## Step 6: Testing and Validation

### 6.1 Startup Validation Test
```bash
# Test with invalid certificate path
FLOWPLANE_TLS_ENABLED=true \
FLOWPLANE_TLS_CERT_PATH=/nonexistent/cert.pem \
FLOWPLANE_TLS_KEY_PATH=/etc/flowplane/tls/key.pem \
cargo run --bin flowplane

# Expected: startup failure with clear error message
# [ERROR] TLS configuration error: Certificate file not found: /nonexistent/cert.pem
```

### 6.2 Certificate Validation Test
```bash
# Test with mismatched certificate and key
FLOWPLANE_TLS_ENABLED=true \
FLOWPLANE_TLS_CERT_PATH=/etc/flowplane/tls/cert.pem \
FLOWPLANE_TLS_KEY_PATH=/etc/flowplane/tls/wrong-key.pem \
cargo run --bin flowplane

# Expected: startup failure with mismatch error
# [ERROR] TLS configuration error: Certificate and private key do not match
```

### 6.3 End-to-End Authentication Test
```bash
# Create test token via HTTPS
curl -X POST https://localhost:8443/api/v1/tokens \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "HTTPS Test Token",
    "scopes": ["clusters:read"]
  }'

# Use new token for API access
curl -H "Authorization: Bearer $NEW_TOKEN" \
  https://localhost:8443/api/v1/clusters

# Verify authentication works identically to HTTP mode
```

## Step 7: Monitoring and Maintenance

### 7.1 Certificate Expiration Monitoring
```bash
# Check certificate expiration
openssl x509 -in /etc/flowplane/tls/cert.pem -checkend 2592000  # 30 days
echo $?  # 0 = valid, 1 = expires within 30 days

# Add to monitoring script
#!/bin/bash
CERT_FILE="/etc/flowplane/tls/cert.pem"
if ! openssl x509 -in "$CERT_FILE" -checkend 2592000 > /dev/null 2>&1; then
  echo "WARNING: Flowplane TLS certificate expires within 30 days"
  # Send alert to monitoring system
fi
```

### 7.2 TLS Health Monitoring
```bash
# Monitor TLS endpoint health
curl -f -k https://localhost:8443/health || echo "TLS endpoint failed"

# Check TLS handshake performance
time openssl s_client -connect localhost:8443 < /dev/null
# Should complete in <50ms for good performance
```

## Troubleshooting

### Common Issues

**Certificate file not found**
- Verify file paths are absolute and correct
- Check file permissions (cert: 644, key: 600)
- Ensure Flowplane process can read files

**Certificate/key mismatch**
- Verify files correspond to same certificate request
- Check file formats (both must be PEM)
- Use `openssl` commands to verify matching modulus

**Permission denied**
- Check file ownership and permissions
- Ensure Flowplane user can read certificate files
- Verify SELinux/AppArmor policies if applicable

**TLS handshake failures**
- Check certificate chain completeness
- Verify certificate not expired
- Test with `openssl s_client` for detailed errors

**Port conflicts**
- Ensure TLS port doesn't conflict with xDS port
- Check no other services using configured port
- Verify firewall allows connections to TLS port

### Getting Help
- Check server logs for detailed error messages
- Use `openssl` tools for certificate validation
- Test with curl `-k` flag to bypass certificate verification during troubleshooting
- Verify working HTTP mode before enabling TLS