//! Secret Management MCP Tools
//!
//! Control Plane tools for managing secrets via the MCP protocol.
//! Secrets are used for TLS certificates, API keys, OAuth tokens, and other
//! sensitive configuration consumed by Envoy via SDS (Secret Discovery Service).

use crate::api::handlers::secrets::types::SecretResponse;
use crate::domain::{OrgId, SecretType};
use crate::mcp::error::McpError;
use crate::mcp::protocol::{Tool, ToolCallResult};
use crate::mcp::response_builders::{build_create_response, build_delete_response};
use crate::storage::repositories::secret::CreateSecretRequest as RepoCreateSecretRequest;
use crate::storage::SecretData;
use crate::xds::XdsState;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::instrument;

/// Tool definition for listing secrets
pub fn cp_list_secrets_tool() -> Tool {
    Tool::new(
        "cp_list_secrets",
        r#"List secrets managed by the control plane. Returns metadata only — secret values are never exposed.

PURPOSE: View all secrets for the team, optionally filtered by type.

SECRET TYPES:
- generic_secret: OAuth2 tokens, API keys, HMAC secrets
- tls_certificate: TLS certificate + private key pairs
- certificate_validation_context: CA certificates for peer verification
- session_ticket_keys: TLS session resumption keys

FILTERING:
- secretType: Filter by secret type (e.g., "generic_secret", "tls_certificate")
- limit: Maximum number of results (1-1000, default: 50)
- offset: Pagination offset (default: 0)

RETURNS: Array of secret metadata objects (id, name, secretType, description, version, source, team, timestamps). Secret values are NEVER included.

RELATED TOOLS: cp_get_secret (details), cp_create_secret (new secret), cp_delete_secret (remove)"#,
        {
            let mut props = super::pagination_schema("secrets");
            props["secretType"] = json!({
                "type": "string",
                "description": "Filter by secret type",
                "enum": ["generic_secret", "tls_certificate", "certificate_validation_context", "session_ticket_keys"]
            });
            json!({
                "type": "object",
                "properties": props
            })
        },
    )
}

/// Tool definition for getting a specific secret
pub fn cp_get_secret_tool() -> Tool {
    Tool::new(
        "cp_get_secret",
        r#"Get metadata for a specific secret by ID. Secret values are never exposed.

PURPOSE: Retrieve complete secret metadata including version, type, source, and timestamps.

RETURNS:
- id: Secret UUID
- name: Secret name (unique within team)
- secretType: Type of secret (generic_secret, tls_certificate, etc.)
- description: Optional description
- version: Version number (incremented on rotation/update)
- source: How the secret was created (native_api, reference, etc.)
- team: Owning team
- createdAt, updatedAt: Lifecycle timestamps
- expiresAt: Optional expiration timestamp
- backend: Backend type for reference secrets (vault, aws_secrets_manager, etc.)
- reference: Backend-specific reference (Vault path, AWS ARN)

WHEN TO USE:
- Check secret version before rotation
- Verify secret expiration
- Inspect secret metadata before attaching to a filter

Authorization: Requires secrets:read scope."#,
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Secret UUID"
                }
            },
            "required": ["id"]
        }),
    )
}

/// Tool definition for creating a secret
pub fn cp_create_secret_tool() -> Tool {
    Tool::new(
        "cp_create_secret",
        r#"Create a new secret in the control plane. The secret value is encrypted at rest.

PURPOSE: Store sensitive configuration (API keys, TLS certs, OAuth tokens) for use by Envoy filters via SDS.

REQUIRED PARAMETERS:
- name: Unique name within the team (1-255 chars)
- secretType: One of: generic_secret, tls_certificate, certificate_validation_context, session_ticket_keys
- configuration: JSON object with type-specific fields (see below)

OPTIONAL PARAMETERS:
- description: Human-readable description
- expiresAt: ISO 8601 expiration timestamp

CONFIGURATION FORMAT BY TYPE:
- generic_secret: { "type": "generic_secret", "secret": "<base64-encoded-value>" }
- tls_certificate: { "type": "tls_certificate", "certificate_chain": "<PEM>", "private_key": "<PEM>" }
- certificate_validation_context: { "type": "certificate_validation_context", "trusted_ca": "<PEM>" }
- session_ticket_keys: { "type": "session_ticket_keys", "keys": [{ "name": "key1", "key": "<base64-80-bytes>" }] }

AFTER CREATION:
- Secret is encrypted and stored in the database
- Use cp_get_secret to verify creation
- Attach to filters (jwt_auth, ext_authz, etc.) by referencing the secret name

Authorization: Requires secrets:create scope."#,
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Secret name (unique within team, 1-255 chars)"
                },
                "secretType": {
                    "type": "string",
                    "description": "Type of secret",
                    "enum": ["generic_secret", "tls_certificate", "certificate_validation_context", "session_ticket_keys"]
                },
                "configuration": {
                    "type": "object",
                    "description": "Secret configuration (type-specific, see tool description)"
                },
                "description": {
                    "type": "string",
                    "description": "Optional description of the secret"
                },
                "expiresAt": {
                    "type": "string",
                    "description": "Optional expiration time in ISO 8601 format"
                }
            },
            "required": ["name", "secretType", "configuration"]
        }),
    )
}

/// Tool definition for deleting a secret
pub fn cp_delete_secret_tool() -> Tool {
    Tool::new(
        "cp_delete_secret",
        r#"Delete a secret from the control plane.

PURPOSE: Remove a secret that is no longer needed. The encrypted data is permanently deleted.

PREREQUISITES:
- Ensure the secret is not referenced by any active filter configuration
- Deleting a secret referenced by a filter will cause filter configuration errors

WHEN TO USE:
- Remove expired or rotated-out secrets
- Clean up test secrets
- Remove secrets before team decommissioning

Required Parameters:
- id: Secret UUID to delete

Authorization: Requires secrets:delete scope."#,
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Secret UUID to delete"
                }
            },
            "required": ["id"]
        }),
    )
}

// =============================================================================
// EXECUTE FUNCTIONS
// =============================================================================

/// Execute list secrets operation.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_list_secrets")]
pub async fn execute_list_secrets(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let secret_type_filter = args.get("secretType").and_then(|v| v.as_str());
    let limit = args.get("limit").and_then(|v| v.as_i64()).map(|v| v as i32);
    let offset = args.get("offset").and_then(|v| v.as_i64()).map(|v| v as i32);

    let repo = xds_state
        .secret_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Secret repository unavailable".to_string()))?;

    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = super::resolve_mcp_auth(team, org_id, team_repo).await?;

    let secrets = repo
        .list_by_teams(&auth.allowed_teams, limit, offset)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to list secrets: {}", e)))?;

    // Apply optional secret type filter client-side
    let secrets: Vec<&SecretData> = if let Some(type_str) = secret_type_filter {
        let parsed_type: SecretType = type_str
            .parse()
            .map_err(|e: String| McpError::InvalidParams(format!("Invalid secretType: {}", e)))?;
        secrets.iter().filter(|s| s.secret_type == parsed_type).collect()
    } else {
        secrets.iter().collect()
    };

    let result = json!({
        "secrets": secrets.iter().map(|s| secret_metadata_json(s)).collect::<Vec<_>>(),
        "count": secrets.len()
    });

    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult::text(result_text))
}

/// Execute get secret operation.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_get_secret")]
pub async fn execute_get_secret(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let id = args["id"]
        .as_str()
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: id".to_string()))?;

    let repo = xds_state
        .secret_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Secret repository unavailable".to_string()))?;

    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = super::resolve_mcp_auth(team, org_id, team_repo).await?;

    let secret_id = crate::domain::SecretId::from(id.to_string());
    let secret = repo
        .get_by_id(&secret_id)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to get secret: {}", e)))?;

    // Enforce team isolation
    if !auth.allowed_teams.contains(&secret.team) {
        return Err(McpError::Forbidden("Secret not found in your team".to_string()));
    }

    let result = secret_metadata_json(&secret);
    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult::text(result_text))
}

/// Execute create secret operation.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_create_secret")]
pub async fn execute_create_secret(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    let secret_type_str = args.get("secretType").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: secretType".to_string())
    })?;

    let configuration = args.get("configuration").ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: configuration".to_string())
    })?;

    let description = args.get("description").and_then(|v| v.as_str()).map(String::from);
    let expires_at_str = args.get("expiresAt").and_then(|v| v.as_str());

    let secret_type: SecretType = secret_type_str
        .parse()
        .map_err(|e: String| McpError::InvalidParams(format!("Invalid secretType: {}", e)))?;

    // Parse the configuration as a SecretSpec
    let secret_spec: crate::domain::secret::SecretSpec =
        serde_json::from_value(configuration.clone())
            .map_err(|e| McpError::InvalidParams(format!("Invalid configuration: {}", e)))?;

    // Parse optional expires_at
    let expires_at = if let Some(ts) = expires_at_str {
        Some(
            chrono::DateTime::parse_from_rfc3339(ts)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .map_err(|e| {
                    McpError::InvalidParams(format!(
                        "Invalid expiresAt format (use ISO 8601): {}",
                        e
                    ))
                })?,
        )
    } else {
        None
    };

    let repo = xds_state
        .secret_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Secret repository unavailable".to_string()))?;

    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = super::resolve_mcp_auth(team, org_id, team_repo).await?;

    tracing::debug!(
        team = %team,
        secret_name = %name,
        secret_type = %secret_type,
        "Creating secret via MCP"
    );

    let request = RepoCreateSecretRequest {
        name: name.to_string(),
        secret_type,
        description,
        configuration: secret_spec,
        team: auth.team.clone().unwrap_or_default(),
        expires_at,
    };

    let secret = repo
        .create(request)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to create secret: {}", e)))?;

    let mut output = build_create_response("secret", &secret.name, secret.id.as_ref());
    output["secretType"] = json!(secret.secret_type.as_str());
    output["version"] = json!(secret.version);

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        secret_id = %secret.id,
        secret_name = %secret.name,
        "Successfully created secret via MCP"
    );

    Ok(ToolCallResult::text(text))
}

/// Execute delete secret operation.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_delete_secret")]
pub async fn execute_delete_secret(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let id = args["id"]
        .as_str()
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: id".to_string()))?;

    let repo = xds_state
        .secret_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Secret repository unavailable".to_string()))?;

    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = super::resolve_mcp_auth(team, org_id, team_repo).await?;

    let secret_id = crate::domain::SecretId::from(id.to_string());

    // Verify the secret exists and belongs to this team
    let secret = repo
        .get_by_id(&secret_id)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to get secret: {}", e)))?;

    if !auth.allowed_teams.contains(&secret.team) {
        return Err(McpError::Forbidden("Secret not found in your team".to_string()));
    }

    tracing::debug!(team = %team, secret_id = %id, "Deleting secret via MCP");

    repo.delete(&secret_id)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to delete secret: {}", e)))?;

    let output = build_delete_response();
    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(team = %team, secret_id = %id, "Successfully deleted secret via MCP");

    Ok(ToolCallResult::text(text))
}

// =============================================================================
// HELPERS
// =============================================================================

/// Convert SecretData to JSON metadata (never includes decrypted values).
fn secret_metadata_json(s: &SecretData) -> Value {
    let resp = SecretResponse::from_data(s);
    json!({
        "id": resp.id,
        "name": resp.name,
        "secretType": resp.secret_type.as_str(),
        "description": resp.description,
        "version": resp.version,
        "source": resp.source,
        "team": resp.team,
        "createdAt": resp.created_at.to_rfc3339(),
        "updatedAt": resp.updated_at.to_rfc3339(),
        "expiresAt": resp.expires_at.map(|dt| dt.to_rfc3339()),
        "backend": resp.backend,
        "reference": resp.reference,
        "referenceVersion": resp.reference_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cp_list_secrets_tool_definition() {
        let tool = cp_list_secrets_tool();
        assert_eq!(tool.name, "cp_list_secrets");
        assert!(tool.description.as_ref().is_some_and(|d| d.contains("secret")));
    }

    #[test]
    fn test_cp_get_secret_tool_definition() {
        let tool = cp_get_secret_tool();
        assert_eq!(tool.name, "cp_get_secret");

        let required = tool.input_schema["required"].as_array();
        assert!(required.is_some_and(|r| r.contains(&json!("id"))));
    }

    #[test]
    fn test_cp_create_secret_tool_definition() {
        let tool = cp_create_secret_tool();
        assert_eq!(tool.name, "cp_create_secret");

        let required = tool.input_schema["required"].as_array();
        assert!(required.is_some_and(|r| r.contains(&json!("name"))));
        assert!(required.is_some_and(|r| r.contains(&json!("secretType"))));
        assert!(required.is_some_and(|r| r.contains(&json!("configuration"))));
    }

    #[test]
    fn test_cp_delete_secret_tool_definition() {
        let tool = cp_delete_secret_tool();
        assert_eq!(tool.name, "cp_delete_secret");

        let required = tool.input_schema["required"].as_array();
        assert!(required.is_some_and(|r| r.contains(&json!("id"))));
    }

    #[test]
    fn test_tool_names_are_unique() {
        let tools = [
            cp_list_secrets_tool(),
            cp_get_secret_tool(),
            cp_create_secret_tool(),
            cp_delete_secret_tool(),
        ];

        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        let mut unique_names = names.clone();
        unique_names.sort();
        unique_names.dedup();

        assert_eq!(names.len(), unique_names.len(), "Tool names must be unique");
    }
}
