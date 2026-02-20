//! Validation helpers for user-related requests.

use lazy_static::lazy_static;
use regex::Regex;
use validator::{Validate, ValidationError, ValidationErrors};

use super::user::{
    ChangePasswordRequest, CreateTeamMembershipRequest, CreateUserRequest, LoginRequest,
    UpdateTeamMembershipRequest, UpdateUserRequest,
};

lazy_static! {
    // Email validation: basic RFC 5322 compliant pattern
    static ref EMAIL_REGEX: Regex = Regex::new(
        r"^[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$"
    )
    .expect("EMAIL_REGEX should be a valid regex pattern");

    // Team name validation: lowercase letters, numbers, and hyphens
    static ref TEAM_NAME_REGEX: Regex = Regex::new(r"^[a-z0-9-]{3,64}$")
        .expect("TEAM_NAME_REGEX should be a valid regex pattern");

    // Scope validation (same as token scopes)
    static ref SCOPE_REGEX: Regex = Regex::new(r"^(team:[a-z0-9-]+:[a-z-]+:[a-z]+|[a-z-]+:[a-z]+|admin:all)$")
        .expect("SCOPE_REGEX should be a valid regex pattern");
}

/// Minimum password length requirement
const MIN_PASSWORD_LENGTH: usize = 8;

/// Maximum password length to prevent DoS
const MAX_PASSWORD_LENGTH: usize = 128;

/// Validate email format
pub fn validate_email(email: &str) -> Result<(), ValidationError> {
    if EMAIL_REGEX.is_match(email) {
        Ok(())
    } else {
        Err(ValidationError::new("invalid_email"))
    }
}

/// Validate password strength
/// Requirements:
/// - At least 8 characters
/// - At most 128 characters (to prevent DoS)
/// - Contains at least one uppercase letter
/// - Contains at least one lowercase letter
/// - Contains at least one digit
/// - Contains at least one special character
pub fn validate_password(password: &str) -> Result<(), ValidationError> {
    if password.len() < MIN_PASSWORD_LENGTH {
        return Err(ValidationError::new("password_too_short"));
    }

    if password.len() > MAX_PASSWORD_LENGTH {
        return Err(ValidationError::new("password_too_long"));
    }

    let has_uppercase = password.chars().any(|c| c.is_uppercase());
    let has_lowercase = password.chars().any(|c| c.is_lowercase());
    let has_digit = password.chars().any(|c| c.is_numeric());
    let has_special = password.chars().any(|c| !c.is_alphanumeric());

    if !has_uppercase {
        return Err(ValidationError::new("password_missing_uppercase"));
    }

    if !has_lowercase {
        return Err(ValidationError::new("password_missing_lowercase"));
    }

    if !has_digit {
        return Err(ValidationError::new("password_missing_digit"));
    }

    if !has_special {
        return Err(ValidationError::new("password_missing_special"));
    }

    Ok(())
}

/// Validate team name format
pub fn validate_team_name(team: &str) -> Result<(), ValidationError> {
    if TEAM_NAME_REGEX.is_match(team) {
        Ok(())
    } else {
        Err(ValidationError::new("invalid_team_name"))
    }
}

/// Validate scope format
pub fn validate_scope(scope: &str) -> Result<(), ValidationError> {
    if SCOPE_REGEX.is_match(scope) {
        Ok(())
    } else {
        Err(ValidationError::new("invalid_scope"))
    }
}

/// Validate a list of scopes
fn validate_scopes_list(scopes: &Vec<String>) -> Result<(), ValidationError> {
    if scopes.is_empty() {
        return Err(ValidationError::new("scopes_empty"));
    }

    for scope in scopes {
        validate_scope(scope)?;
    }
    Ok(())
}

/// Validate user name (non-empty, reasonable length)
pub fn validate_user_name(name: &str) -> Result<(), ValidationError> {
    let trimmed = name.trim();

    if trimmed.is_empty() {
        return Err(ValidationError::new("name_empty"));
    }

    if trimmed.len() > 255 {
        return Err(ValidationError::new("name_too_long"));
    }

    Ok(())
}

// Implement Validate trait for CreateUserRequest

impl Validate for CreateUserRequest {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();

        // Validate email
        if let Err(err) = validate_email(&self.email) {
            errors.add("email", err);
        }

        // Validate password
        if let Err(err) = validate_password(&self.password) {
            errors.add("password", err);
        }

        // Validate name
        if let Err(err) = validate_user_name(&self.name) {
            errors.add("name", err);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

// Implement Validate trait for UpdateUserRequest

impl Validate for UpdateUserRequest {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();

        // Validate email if present
        if let Some(email) = &self.email {
            if let Err(err) = validate_email(email) {
                errors.add("email", err);
            }
        }

        // Validate name if present
        if let Some(name) = &self.name {
            if let Err(err) = validate_user_name(name) {
                errors.add("name", err);
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

// Implement Validate trait for ChangePasswordRequest

impl Validate for ChangePasswordRequest {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();

        // Validate current password (basic check - just not empty)
        if self.current_password.is_empty() {
            errors.add("current_password", ValidationError::new("password_empty"));
        }

        // Validate new password
        if let Err(err) = validate_password(&self.new_password) {
            errors.add("new_password", err);
        }

        // Ensure new password is different from current
        if self.current_password == self.new_password {
            errors.add("new_password", ValidationError::new("password_unchanged"));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

// Implement Validate trait for CreateTeamMembershipRequest

impl Validate for CreateTeamMembershipRequest {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();

        // Validate team name
        if let Err(err) = validate_team_name(&self.team) {
            errors.add("team", err);
        }

        // Validate scopes
        if let Err(err) = validate_scopes_list(&self.scopes) {
            errors.add("scopes", err);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

// Implement Validate trait for UpdateTeamMembershipRequest

impl Validate for UpdateTeamMembershipRequest {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();

        // Validate scopes
        if let Err(err) = validate_scopes_list(&self.scopes) {
            errors.add("scopes", err);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

// Implement Validate trait for LoginRequest

impl Validate for LoginRequest {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();

        // Validate email
        if let Err(err) = validate_email(&self.email) {
            errors.add("email", err);
        }

        // Password must not be empty (we don't validate strength for login)
        if self.password.is_empty() {
            errors.add("password", ValidationError::new("password_empty"));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::user::UserStatus;
    use crate::domain::UserId;

    #[test]
    fn email_validation_accepts_valid_emails() {
        assert!(validate_email("user@example.com").is_ok());
        assert!(validate_email("test.user+tag@example.co.uk").is_ok());
        assert!(validate_email("admin@subdomain.example.com").is_ok());
    }

    #[test]
    fn email_validation_rejects_invalid_emails() {
        assert!(validate_email("notanemail").is_err());
        assert!(validate_email("@example.com").is_err());
        assert!(validate_email("user@").is_err());
        assert!(validate_email("user name@example.com").is_err());
    }

    #[test]
    fn password_validation_accepts_strong_passwords() {
        assert!(validate_password("SecureP@ssw0rd").is_ok());
        assert!(validate_password("MyP@ssw0rd123").is_ok());
        assert!(validate_password("C0mpl3x!Pass").is_ok());
    }

    #[test]
    fn password_validation_rejects_weak_passwords() {
        assert!(validate_password("short").is_err()); // Too short
        assert!(validate_password("alllowercase1!").is_err()); // No uppercase
        assert!(validate_password("ALLUPPERCASE1!").is_err()); // No lowercase
        assert!(validate_password("NoDigits!Here").is_err()); // No digit
        assert!(validate_password("NoSpecial123").is_err()); // No special char
    }

    #[test]
    fn team_name_validation_accepts_valid_names() {
        assert!(validate_team_name("team-a").is_ok());
        assert!(validate_team_name("my-awesome-team").is_ok());
        assert!(validate_team_name("team123").is_ok());
    }

    #[test]
    fn team_name_validation_rejects_invalid_names() {
        assert!(validate_team_name("ab").is_err()); // Too short
        assert!(validate_team_name("Team-A").is_err()); // Uppercase not allowed
        assert!(validate_team_name("team_name").is_err()); // Underscore not allowed
        assert!(validate_team_name("team name").is_err()); // Space not allowed
    }

    #[test]
    fn scope_validation_accepts_valid_scopes() {
        assert!(validate_scope("admin:all").is_ok());
        assert!(validate_scope("clusters:read").is_ok());
        assert!(validate_scope("team:my-team:routes:write").is_ok());
    }

    #[test]
    fn scope_validation_rejects_invalid_scopes() {
        assert!(validate_scope("invalid").is_err());
        assert!(validate_scope("UPPERCASE:READ").is_err());
        assert!(validate_scope("team:only-two").is_err());
    }

    #[test]
    fn user_name_validation_accepts_valid_names() {
        assert!(validate_user_name("John Doe").is_ok());
        assert!(validate_user_name("Admin").is_ok());
        assert!(validate_user_name("Test User 123").is_ok());
    }

    #[test]
    fn user_name_validation_rejects_invalid_names() {
        assert!(validate_user_name("").is_err()); // Empty
        assert!(validate_user_name("   ").is_err()); // Only whitespace
        assert!(validate_user_name(&"a".repeat(256)).is_err()); // Too long
    }

    #[test]
    fn create_user_request_validation() {
        let mut request = CreateUserRequest {
            email: "test@example.com".to_string(),
            password: "SecureP@ssw0rd".to_string(),
            name: "Test User".to_string(),
            is_admin: false,
            org_id: None,
        };

        assert!(request.validate().is_ok());

        // Invalid email
        request.email = "invalid-email".to_string();
        assert!(request.validate().is_err());

        // Fix email, invalid password
        request.email = "test@example.com".to_string();
        request.password = "weak".to_string();
        assert!(request.validate().is_err());

        // Fix password, invalid name
        request.password = "SecureP@ssw0rd".to_string();
        request.name = "".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn update_user_request_validation() {
        let mut request = UpdateUserRequest {
            email: Some("test@example.com".to_string()),
            name: Some("Test User".to_string()),
            status: Some(UserStatus::Active),
            is_admin: Some(false),
        };

        assert!(request.validate().is_ok());

        // Invalid email
        request.email = Some("invalid-email".to_string());
        assert!(request.validate().is_err());

        // Fix email, invalid name
        request.email = Some("test@example.com".to_string());
        request.name = Some("".to_string());
        assert!(request.validate().is_err());
    }

    #[test]
    fn change_password_request_validation() {
        let mut request = ChangePasswordRequest {
            current_password: "OldP@ssw0rd".to_string(),
            new_password: "NewP@ssw0rd123".to_string(),
        };

        assert!(request.validate().is_ok());

        // Empty current password
        request.current_password = "".to_string();
        assert!(request.validate().is_err());

        // Weak new password
        request.current_password = "OldP@ssw0rd".to_string();
        request.new_password = "weak".to_string();
        assert!(request.validate().is_err());

        // Same password
        request.new_password = "OldP@ssw0rd".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn create_team_membership_request_validation() {
        let mut request = CreateTeamMembershipRequest {
            user_id: UserId::new(),
            team: "team-a".to_string(),
            scopes: vec!["clusters:read".to_string(), "routes:write".to_string()],
        };

        assert!(request.validate().is_ok());

        // Invalid team name
        request.team = "Team-A".to_string();
        assert!(request.validate().is_err());

        // Fix team, empty scopes
        request.team = "team-a".to_string();
        request.scopes = vec![];
        assert!(request.validate().is_err());

        // Invalid scope
        request.scopes = vec!["invalid-scope".to_string()];
        assert!(request.validate().is_err());
    }

    #[test]
    fn login_request_validation() {
        let mut request = LoginRequest {
            email: "test@example.com".to_string(),
            password: "password".to_string(),
        };

        assert!(request.validate().is_ok());

        // Invalid email
        request.email = "invalid-email".to_string();
        assert!(request.validate().is_err());

        // Fix email, empty password
        request.email = "test@example.com".to_string();
        request.password = "".to_string();
        assert!(request.validate().is_err());
    }
}
