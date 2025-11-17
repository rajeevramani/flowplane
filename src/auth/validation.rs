//! Validation helpers and request DTOs for personal access token endpoints.

use chrono::{DateTime, Utc};
use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError, ValidationErrors};

lazy_static! {
    static ref NAME_REGEX: Regex = Regex::new(r"^[a-zA-Z0-9_-]{3,64}$")
        .expect("NAME_REGEX should be a valid regex pattern");
    // Scope patterns:
    // - admin:all (global admin)
    // - {resource}:{action} (e.g., routes:read, api-definitions:write)
    // - team:{team}:{resource}:{action} (e.g., team:platform:routes:read, team:team-test-1:clusters:read)
    // Team names can contain lowercase letters, digits, and hyphens
    static ref SCOPE_REGEX: Regex = Regex::new(r"^(team:[a-z0-9-]+:[a-z-]+:[a-z]+|[a-z-]+:[a-z]+)$")
        .expect("SCOPE_REGEX should be a valid regex pattern");
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct CreateTokenRequest {
    #[validate(custom(function = "validate_token_name"))]
    pub name: String,
    pub description: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    #[validate(length(min = 1), custom(function = "validate_scopes_list"))]
    pub scopes: Vec<String>,
    pub created_by: Option<String>,
    #[serde(default)]
    pub user_id: Option<crate::domain::UserId>,
    #[serde(default)]
    pub user_email: Option<String>,
}

impl CreateTokenRequest {
    /// Create a test request without user context (for CLI and tests)
    pub fn without_user(
        name: String,
        description: Option<String>,
        expires_at: Option<DateTime<Utc>>,
        scopes: Vec<String>,
        created_by: Option<String>,
    ) -> Self {
        Self {
            name,
            description,
            expires_at,
            scopes,
            created_by,
            user_id: None,
            user_email: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTokenRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub expires_at: Option<Option<DateTime<Utc>>>,
    pub scopes: Option<Vec<String>>,
}

impl Validate for UpdateTokenRequest {
    fn validate(&self) -> Result<(), ValidationErrors> {
        if let Some(name) = &self.name {
            validate_token_name(name).map_err(|err| {
                let mut errors = ValidationErrors::new();
                errors.add("name", err);
                errors
            })?;
        }

        if let Some(scopes) = &self.scopes {
            validate_scopes_list(scopes).map_err(|err| {
                let mut errors = ValidationErrors::new();
                errors.add("scopes", err);
                errors
            })?;
        }

        if let Some(status) = &self.status {
            if !matches!(status.as_str(), "active" | "revoked" | "expired") {
                let mut errors = ValidationErrors::new();
                errors.add("status", ValidationError::new("invalid_status"));
                return Err(errors);
            }
        }

        Ok(())
    }
}

pub fn validate_token_name(name: &str) -> Result<(), ValidationError> {
    if NAME_REGEX.is_match(name) {
        Ok(())
    } else {
        Err(ValidationError::new("invalid_token_name"))
    }
}

pub fn validate_scope(scope: &str) -> Result<(), ValidationError> {
    if SCOPE_REGEX.is_match(scope) {
        Ok(())
    } else {
        Err(ValidationError::new("invalid_scope"))
    }
}

fn validate_scopes_list(scopes: &Vec<String>) -> Result<(), ValidationError> {
    for scope in scopes {
        validate_scope(scope)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation_allows_valid_patterns() {
        assert!(validate_token_name("admin-token").is_ok());
        assert!(validate_token_name("A1_foo").is_ok());
        assert!(validate_token_name("no").is_err());
    }

    #[test]
    fn scope_validation() {
        // Resource-level scopes
        assert!(validate_scope("clusters:read").is_ok());
        assert!(validate_scope("routes:write").is_ok());
        assert!(validate_scope("api-definitions:read").is_ok());
        assert!(validate_scope("api-definitions:write").is_ok());

        // Admin scope
        assert!(validate_scope("admin:all").is_ok());

        // Team-scoped scopes
        assert!(validate_scope("team:platform:routes:read").is_ok());
        assert!(validate_scope("team:eng-team:api-definitions:write").is_ok());

        // Team names with digits (bug fix)
        assert!(validate_scope("team:team-test-1:clusters:read").is_ok());
        assert!(validate_scope("team:payments-2024:routes:write").is_ok());
        assert!(validate_scope("team:team123:api-definitions:read").is_ok());

        // Invalid patterns
        assert!(validate_scope("bad_scope").is_err()); // No colon
        assert!(validate_scope("routes:read:extra").is_err()); // Too many parts for resource-level
        assert!(validate_scope("team:only-two").is_err()); // Team scope needs 4 parts
        assert!(validate_scope("UPPERCASE:READ").is_err()); // Must be lowercase
    }

    #[test]
    fn update_validation_checks_optional_fields() {
        let mut request = UpdateTokenRequest {
            name: Some("new-name".into()),
            description: None,
            status: Some("revoked".into()),
            expires_at: None,
            scopes: Some(vec!["clusters:read".into()]),
        };
        assert!(request.validate().is_ok());

        request.name = Some("!bad".into());
        assert!(request.validate().is_err());

        request.name = Some("good".into());
        request.scopes = Some(vec!["invalid".into()]);
        assert!(request.validate().is_err());

        request.scopes = Some(vec!["clusters:read".into()]);
        request.status = Some("unknown".into());
        assert!(request.validate().is_err());
    }
}
