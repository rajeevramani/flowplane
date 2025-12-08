#!/bin/bash
# Setup Vault PKI for mTLS development
#
# Prerequisites:
#   - Vault running: docker-compose -f docker-compose-mtls-dev.yml up -d
#   - vault CLI installed (brew install vault)
#
# Usage:
#   ./scripts/setup-vault-pki.sh

set -e

export VAULT_ADDR=${VAULT_ADDR:-http://localhost:8200}
export VAULT_TOKEN=${VAULT_TOKEN:-flowplane-dev-token}

echo "Configuring Vault PKI at $VAULT_ADDR..."

# Check Vault is accessible
if ! vault status > /dev/null 2>&1; then
    echo "Error: Cannot connect to Vault at $VAULT_ADDR"
    echo "Start Vault with: docker-compose -f docker-compose-mtls-dev.yml up -d"
    exit 1
fi

# Enable PKI secrets engine (ignore error if already enabled)
echo "Enabling PKI secrets engine..."
vault secrets enable -path=pki_int_proxies pki 2>/dev/null || echo "  (already enabled)"

# Generate self-signed root CA for development
echo "Generating root CA..."
vault write pki_int_proxies/root/generate/internal \
    common_name="Flowplane Dev CA" \
    ttl="87600h" > /dev/null

# Configure CA and CRL URLs
echo "Configuring CA URLs..."
vault write pki_int_proxies/config/urls \
    issuing_certificates="$VAULT_ADDR/v1/pki_int_proxies/ca" \
    crl_distribution_points="$VAULT_ADDR/v1/pki_int_proxies/crl" > /dev/null

# Create the envoy-proxy role
echo "Creating envoy-proxy role..."
vault write pki_int_proxies/roles/envoy-proxy \
    allowed_uri_sans="spiffe://*" \
    allow_any_name=true \
    max_ttl="720h" \
    require_cn=false > /dev/null

echo ""
echo "Vault PKI setup complete!"
echo ""
echo "Environment variables for control plane:"
echo "  export VAULT_ADDR=$VAULT_ADDR"
echo "  export VAULT_TOKEN=$VAULT_TOKEN"
echo "  export FLOWPLANE_VAULT_PKI_MOUNT_PATH=pki_int_proxies"
echo "  export FLOWPLANE_SPIFFE_TRUST_DOMAIN=flowplane.local"
echo ""
echo "Test certificate generation:"
echo "  vault write pki_int_proxies/issue/envoy-proxy \\"
echo "    uri_sans=\"spiffe://flowplane.local/team/test/proxy/test-proxy\""
