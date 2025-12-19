//! Proxy certificate API endpoints for mTLS certificate generation.
//!
//! This module provides endpoints for generating and managing proxy certificates
//! used for mTLS authentication between Envoy proxies and the control plane.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::{authorization::require_resource_access, models::AuthContext},
    domain::ProxyCertificateId,
    errors::Error,
    secrets::{PkiConfig, VaultSecretsClient},
    storage::repositories::{
        CreateProxyCertificateRequest, ProxyCertificateData, ProxyCertificateRepository,
        SqlxProxyCertificateRepository, SqlxTeamRepository, TeamRepository,
    },
};

// ============================================================================
// Request/Response Types
// ============================================================================

/// Request to generate a new proxy certificate.
#[derive(Debug, Clone, Deserialize, Serialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GenerateCertificateRequest {
    /// Unique identifier for the proxy instance (e.g., hostname, pod name).
    #[validate(length(min = 3, max = 64, message = "proxy_id must be 3-64 characters"))]
    #[validate(regex(
        path = *PROXY_ID_REGEX,
        message = "proxy_id must contain only alphanumeric characters, hyphens, and underscores"
    ))]
    pub proxy_id: String,
}

static PROXY_ID_REGEX: once_cell::sync::Lazy<regex::Regex> =
    once_cell::sync::Lazy::new(|| regex::Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9_-]*$").unwrap());

/// Response after successfully generating a certificate.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GenerateCertificateResponse {
    /// Certificate record ID
    pub id: String,

    /// Proxy instance identifier
    pub proxy_id: String,

    /// SPIFFE URI embedded in the certificate
    pub spiffe_uri: String,

    /// PEM-encoded X.509 certificate
    pub certificate: String,

    /// PEM-encoded private key (only returned once at generation time)
    pub private_key: String,

    /// PEM-encoded CA certificate chain
    pub ca_chain: String,

    /// Certificate expiration timestamp (ISO 8601)
    pub expires_at: String,
}

/// Query parameters for listing certificates.
#[derive(Debug, Clone, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ListCertificatesQuery {
    /// Maximum number of certificates to return
    #[serde(default = "default_limit")]
    pub limit: i64,

    /// Offset for pagination
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

/// Response for listing certificates.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListCertificatesResponse {
    /// List of certificates (without private keys)
    pub certificates: Vec<CertificateMetadata>,

    /// Total number of certificates for this team
    pub total: i64,

    /// Pagination limit used
    pub limit: i64,

    /// Pagination offset used
    pub offset: i64,
}

/// Certificate metadata (without private key).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CertificateMetadata {
    pub id: String,
    pub proxy_id: String,
    pub spiffe_uri: String,
    pub serial_number: String,
    pub issued_at: String,
    pub expires_at: String,
    pub is_valid: bool,
    pub is_expired: bool,
    pub is_revoked: bool,
    pub revoked_at: Option<String>,
    pub revoked_reason: Option<String>,
}

impl From<ProxyCertificateData> for CertificateMetadata {
    fn from(data: ProxyCertificateData) -> Self {
        // Call methods before consuming data fields
        let is_valid = data.is_valid();
        let is_expired = data.is_expired();
        let is_revoked = data.is_revoked();

        Self {
            id: data.id.to_string(),
            proxy_id: data.proxy_id,
            spiffe_uri: data.spiffe_uri,
            serial_number: data.serial_number,
            issued_at: data.issued_at.to_rfc3339(),
            expires_at: data.expires_at.to_rfc3339(),
            is_valid,
            is_expired,
            is_revoked,
            revoked_at: data.revoked_at.map(|dt| dt.to_rfc3339()),
            revoked_reason: data.revoked_reason,
        }
    }
}

/// Request to revoke a certificate.
#[derive(Debug, Clone, Deserialize, Serialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RevokeCertificateRequest {
    /// Reason for revocation
    #[validate(length(min = 1, max = 500))]
    pub reason: String,
}

// ============================================================================
// Handlers
// ============================================================================

/// Generate a new proxy certificate for mTLS authentication.
///
/// This endpoint generates a certificate via Vault PKI with a SPIFFE identity
/// embedded in the Subject Alternative Name (SAN). The certificate can be used
/// by an Envoy proxy to authenticate with the control plane.
///
/// **Important:** The private key is only returned once at generation time.
/// It is not stored and cannot be retrieved later.
#[utoipa::path(
    post,
    path = "/api/v1/teams/{team}/proxy-certificates",
    params(
        ("team" = String, Path, description = "Team name")
    ),
    request_body = GenerateCertificateRequest,
    responses(
        (status = 201, description = "Certificate generated successfully", body = GenerateCertificateResponse),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Forbidden - user does not have access to the team"),
        (status = 503, description = "mTLS not configured - FLOWPLANE_VAULT_PKI_MOUNT_PATH not set")
    ),
    tag = "Secrets"
)]
#[instrument(skip(state, payload), fields(team = %team, proxy_id = %payload.proxy_id))]
pub async fn generate_certificate_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(team): Path<String>,
    Json(payload): Json<GenerateCertificateRequest>,
) -> Result<(StatusCode, Json<GenerateCertificateResponse>), ApiError> {
    // Validate request
    payload.validate().map_err(|e| ApiError::BadRequest(e.to_string()))?;

    // Authorization: user must have access to the team
    require_resource_access(&context, "proxy-certificates", "create", Some(&team))?;

    // Check if mTLS is configured
    let pki_config = PkiConfig::from_env().ok_or_else(|| {
        ApiError::service_unavailable(
            "mTLS is not configured. Set FLOWPLANE_VAULT_PKI_MOUNT_PATH to enable certificate generation."
        )
    })?;

    // Verify team exists
    let team_repo = get_team_repository(&state)?;
    let team_data = team_repo
        .get_team_by_name(&team)
        .await
        .map_err(convert_error)?
        .ok_or_else(|| ApiError::NotFound(format!("Team '{}' not found", team)))?;

    // Generate certificate via Vault
    let vault_client = VaultSecretsClient::from_env()
        .await
        .map_err(|e| ApiError::service_unavailable(format!("Vault unavailable: {}", e)))?;

    let generated = vault_client
        .generate_proxy_certificate(&pki_config, &team, &payload.proxy_id)
        .await
        .map_err(|e| {
            ApiError::service_unavailable(format!("Certificate generation failed: {}", e))
        })?;

    // Store certificate metadata in database
    let cert_repo = get_certificate_repository(&state)?;
    let record = cert_repo
        .create(CreateProxyCertificateRequest {
            team_id: team_data.id,
            proxy_id: payload.proxy_id.clone(),
            serial_number: generated.serial_number.clone(),
            spiffe_uri: generated.spiffe_uri.clone(),
            issued_at: chrono::Utc::now(),
            expires_at: generated.expires_at,
            issued_by_user_id: context.user_id,
        })
        .await
        .map_err(convert_error)?;

    Ok((
        StatusCode::CREATED,
        Json(GenerateCertificateResponse {
            id: record.id.to_string(),
            proxy_id: record.proxy_id,
            spiffe_uri: generated.spiffe_uri,
            certificate: generated.certificate,
            private_key: generated.private_key,
            ca_chain: generated.ca_chain,
            expires_at: generated.expires_at.to_rfc3339(),
        }),
    ))
}

/// List all proxy certificates for a team.
///
/// Returns certificate metadata without private keys.
#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/proxy-certificates",
    params(
        ("team" = String, Path, description = "Team name"),
        ListCertificatesQuery
    ),
    responses(
        (status = 200, description = "List of certificates", body = ListCertificatesResponse),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Team not found")
    ),
    tag = "Secrets"
)]
#[instrument(skip(state), fields(team = %team, limit = query.limit, offset = query.offset))]
pub async fn list_certificates_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(team): Path<String>,
    Query(query): Query<ListCertificatesQuery>,
) -> Result<Json<ListCertificatesResponse>, ApiError> {
    // Authorization
    require_resource_access(&context, "proxy-certificates", "read", Some(&team))?;

    // Get team
    let team_repo = get_team_repository(&state)?;
    let team_data = team_repo
        .get_team_by_name(&team)
        .await
        .map_err(convert_error)?
        .ok_or_else(|| ApiError::NotFound(format!("Team '{}' not found", team)))?;

    // List certificates
    let cert_repo = get_certificate_repository(&state)?;
    let certificates = cert_repo
        .list_by_team(&team_data.id, query.limit, query.offset)
        .await
        .map_err(convert_error)?;

    let total = cert_repo.count_by_team(&team_data.id).await.map_err(convert_error)?;

    Ok(Json(ListCertificatesResponse {
        certificates: certificates.into_iter().map(CertificateMetadata::from).collect(),
        total,
        limit: query.limit,
        offset: query.offset,
    }))
}

/// Get a specific proxy certificate by ID.
///
/// Returns certificate metadata without the private key.
#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/proxy-certificates/{id}",
    params(
        ("team" = String, Path, description = "Team name"),
        ("id" = String, Path, description = "Certificate ID")
    ),
    responses(
        (status = 200, description = "Certificate details", body = CertificateMetadata),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Certificate not found")
    ),
    tag = "Secrets"
)]
#[instrument(skip(state), fields(team = %team, id = %id))]
pub async fn get_certificate_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((team, id)): Path<(String, String)>,
) -> Result<Json<CertificateMetadata>, ApiError> {
    // Authorization
    require_resource_access(&context, "proxy-certificates", "read", Some(&team))?;

    // Verify team exists
    let team_repo = get_team_repository(&state)?;
    let _team_data = team_repo
        .get_team_by_name(&team)
        .await
        .map_err(convert_error)?
        .ok_or_else(|| ApiError::NotFound(format!("Team '{}' not found", team)))?;

    // Get certificate
    let cert_repo = get_certificate_repository(&state)?;
    let cert_id = ProxyCertificateId::from_string(id);
    let certificate = cert_repo
        .get_by_id(&cert_id)
        .await
        .map_err(convert_error)?
        .ok_or_else(|| ApiError::NotFound("Certificate not found".to_string()))?;

    Ok(Json(CertificateMetadata::from(certificate)))
}

/// Revoke a proxy certificate.
///
/// Marks the certificate as revoked. Revoked certificates should not be trusted.
#[utoipa::path(
    post,
    path = "/api/v1/teams/{team}/proxy-certificates/{id}/revoke",
    params(
        ("team" = String, Path, description = "Team name"),
        ("id" = String, Path, description = "Certificate ID")
    ),
    request_body = RevokeCertificateRequest,
    responses(
        (status = 200, description = "Certificate revoked", body = CertificateMetadata),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Certificate not found")
    ),
    tag = "Secrets"
)]
#[instrument(skip(state, payload), fields(team = %team, id = %id, reason = %payload.reason))]
pub async fn revoke_certificate_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((team, id)): Path<(String, String)>,
    Json(payload): Json<RevokeCertificateRequest>,
) -> Result<Json<CertificateMetadata>, ApiError> {
    // Validate request
    payload.validate().map_err(|e| ApiError::BadRequest(e.to_string()))?;

    // Authorization
    require_resource_access(&context, "proxy-certificates", "delete", Some(&team))?;

    // Verify team exists
    let team_repo = get_team_repository(&state)?;
    let _team_data = team_repo
        .get_team_by_name(&team)
        .await
        .map_err(convert_error)?
        .ok_or_else(|| ApiError::NotFound(format!("Team '{}' not found", team)))?;

    // Revoke certificate
    let cert_repo = get_certificate_repository(&state)?;
    let cert_id = ProxyCertificateId::from_string(id);
    let revoked = cert_repo.revoke(&cert_id, &payload.reason).await.map_err(convert_error)?;

    Ok(Json(CertificateMetadata::from(revoked)))
}

// ============================================================================
// Helper Functions
// ============================================================================

fn get_team_repository(state: &ApiState) -> Result<SqlxTeamRepository, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Database unavailable"))?;
    let pool = cluster_repo.pool().clone();
    Ok(SqlxTeamRepository::new(pool))
}

fn get_certificate_repository(
    state: &ApiState,
) -> Result<SqlxProxyCertificateRepository, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Database unavailable"))?;
    let pool = cluster_repo.pool().clone();
    Ok(SqlxProxyCertificateRepository::new(pool))
}

fn convert_error(error: Error) -> ApiError {
    ApiError::from(error)
}
