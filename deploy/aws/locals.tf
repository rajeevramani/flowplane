locals {
  name = var.name
  azs  = var.availability_zones

  nat_gateway_azs = var.enable_nat_gateway ? (var.single_nat_gateway ? [local.azs[0]] : local.azs) : []

  tags = merge(
    {
      Project     = "flowplane"
      Environment = var.environment
      ManagedBy   = "opentofu"
    },
    var.tags,
  )

  container_tls_dir = "/tmp/flowplane/tls"

  container_command = <<-EOT
    set -eu
    mkdir -p ${local.container_tls_dir}
    umask 077
    printf '%s' "$FLOWPLANE_API_TLS_CERT_PEM" > ${local.container_tls_dir}/api.crt
    printf '%s' "$FLOWPLANE_API_TLS_KEY_PEM" > ${local.container_tls_dir}/api.key
    printf '%s' "$FLOWPLANE_XDS_TLS_CERT_PEM" > ${local.container_tls_dir}/xds.crt
    printf '%s' "$FLOWPLANE_XDS_TLS_KEY_PEM" > ${local.container_tls_dir}/xds.key
    printf '%s' "$FLOWPLANE_XDS_TLS_CLIENT_CA_PEM" > ${local.container_tls_dir}/dp-ca.crt
    printf '%s' "$FLOWPLANE_CERT_ISSUER_CA_CERT_PEM" > ${local.container_tls_dir}/issuer-ca.crt
    printf '%s' "$FLOWPLANE_CERT_ISSUER_CA_KEY_PEM" > ${local.container_tls_dir}/issuer-ca.key
    export FLOWPLANE_API_TLS_CERT=${local.container_tls_dir}/api.crt
    export FLOWPLANE_API_TLS_KEY=${local.container_tls_dir}/api.key
    export FLOWPLANE_XDS_TLS_CERT=${local.container_tls_dir}/xds.crt
    export FLOWPLANE_XDS_TLS_KEY=${local.container_tls_dir}/xds.key
    export FLOWPLANE_XDS_TLS_CLIENT_CA=${local.container_tls_dir}/dp-ca.crt
    export FLOWPLANE_CERT_ISSUER_CA_CERT_PATH=${local.container_tls_dir}/issuer-ca.crt
    export FLOWPLANE_CERT_ISSUER_CA_KEY_PATH=${local.container_tls_dir}/issuer-ca.key
    export FLOWPLANE_DATABASE_URL="postgres://${var.db_username}:$FLOWPLANE_DB_PASSWORD@$FLOWPLANE_DB_HOST:5432/${var.db_name}?sslmode=${var.database_sslmode}"
    exec /usr/local/bin/flowplane serve
  EOT
}
