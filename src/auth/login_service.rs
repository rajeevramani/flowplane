//! Login service for email/password authentication.

use std::sync::Arc;

use tracing::{info, instrument, warn};

use crate::auth::organization::{OrgRole, OrganizationMembership};
use crate::auth::{hashing, user::UserStatus, LoginRequest, User, UserTeamMembership};
use crate::errors::{AuthErrorType, Error, Result};
use crate::observability::metrics;
use crate::storage::repositories::{
    AuditEvent, AuditLogRepository, OrgMembershipRepository, SqlxOrgMembershipRepository,
    SqlxTeamMembershipRepository, SqlxUserRepository, TeamMembershipRepository, UserRepository,
};

/// Service for handling email/password authentication.
#[derive(Clone)]
pub struct LoginService {
    user_repository: Arc<dyn UserRepository>,
    membership_repository: Arc<dyn TeamMembershipRepository>,
    org_membership_repository: Arc<dyn OrgMembershipRepository>,
    audit_repository: Arc<AuditLogRepository>,
}

impl LoginService {
    pub fn new(
        user_repository: Arc<dyn UserRepository>,
        membership_repository: Arc<dyn TeamMembershipRepository>,
        org_membership_repository: Arc<dyn OrgMembershipRepository>,
        audit_repository: Arc<AuditLogRepository>,
    ) -> Self {
        Self { user_repository, membership_repository, org_membership_repository, audit_repository }
    }

    pub fn with_sqlx(pool: crate::storage::DbPool) -> Self {
        let audit_repository = Arc::new(AuditLogRepository::new(pool.clone()));
        Self::new(
            Arc::new(SqlxUserRepository::new(pool.clone())),
            Arc::new(SqlxTeamMembershipRepository::new(pool.clone())),
            Arc::new(SqlxOrgMembershipRepository::new(pool.clone())),
            audit_repository,
        )
    }

    /// Authenticate user with email and password, returning user and computed scopes.
    ///
    /// # Arguments
    ///
    /// * `request` - Login request with email and password
    /// * `client_ip` - Optional client IP address for audit logging
    /// * `user_agent` - Optional user agent string for audit logging
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - User not found
    /// - Password is incorrect
    /// - User account is not active
    /// - User account is suspended
    #[instrument(skip(self, request, client_ip, user_agent), fields(email = %request.email))]
    pub async fn login(
        &self,
        request: &LoginRequest,
        client_ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(User, Vec<String>)> {
        // Normalize email
        let email = User::normalize_email(&request.email);

        // Fetch user with password hash
        let (user, password_hash) =
            match self.user_repository.get_user_with_password(&email).await? {
                Some(result) => result,
                None => {
                    warn!(email = %email, "login attempt for non-existent user");
                    metrics::record_authentication("invalid_credentials").await;
                    return Err(Error::auth(
                        "Invalid email or password",
                        AuthErrorType::InvalidCredentials,
                    ));
                }
            };

        // Verify password
        let password_matches = hashing::verify_password(&request.password, &password_hash)?;
        if !password_matches {
            warn!(user_id = %user.id, email = %email, "login attempt with incorrect password");
            metrics::record_authentication("invalid_credentials").await;

            // Audit failed login
            self.audit_repository
                .record_auth_event(
                    AuditEvent::token(
                        "auth.login.failed",
                        Some(user.id.as_str()),
                        Some(&user.email),
                        serde_json::json!({ "reason": "invalid_password" }),
                    )
                    .with_user_context(
                        Some(user.id.to_string()),
                        client_ip.clone(),
                        user_agent.clone(),
                    ),
                )
                .await?;

            return Err(Error::auth(
                "Invalid email or password",
                AuthErrorType::InvalidCredentials,
            ));
        }

        // Check user status
        if user.status != UserStatus::Active {
            warn!(user_id = %user.id, status = ?user.status, "login attempt for non-active user");
            metrics::record_authentication("account_not_active").await;

            let status_str = match user.status {
                UserStatus::Inactive => "inactive",
                UserStatus::Suspended => "suspended",
                UserStatus::Active => unreachable!(),
            };

            // Audit failed login
            self.audit_repository
                .record_auth_event(
                    AuditEvent::token(
                        "auth.login.failed",
                        Some(user.id.as_str()),
                        Some(&user.email),
                        serde_json::json!({ "reason": "account_not_active", "status": status_str }),
                    )
                    .with_user_context(
                        Some(user.id.to_string()),
                        client_ip.clone(),
                        user_agent.clone(),
                    ),
                )
                .await?;

            return Err(Error::auth(
                format!("Account is {}", status_str),
                AuthErrorType::InvalidCredentials,
            ));
        }

        // Fetch team memberships
        let memberships = self.membership_repository.list_user_memberships(&user.id).await?;

        // Fetch org memberships
        let org_memberships = self
            .org_membership_repository
            .list_user_memberships(&user.id)
            .await
            .unwrap_or_default();

        // Compute scopes from team + org memberships
        let scopes = compute_scopes_from_memberships(&user, &memberships, &org_memberships);

        // Audit successful login
        self.audit_repository
            .record_auth_event(
                AuditEvent::token(
                    "auth.login.success",
                    Some(user.id.as_str()),
                    Some(&user.email),
                    serde_json::json!({
                        "teams": memberships.iter().map(|m| &m.team).collect::<Vec<_>>(),
                        "scope_count": scopes.len(),
                    }),
                )
                .with_user_context(
                    Some(user.id.to_string()),
                    client_ip,
                    user_agent,
                ),
            )
            .await?;

        metrics::record_authentication("success").await;
        info!(user_id = %user.id, email = %user.email, "user logged in successfully");

        Ok((user, scopes))
    }
}

/// Compute scopes from user's team and organization memberships.
///
/// If user is an admin, grants `admin:all` scope.
/// Otherwise, returns all scopes from team memberships + org scopes from org memberships.
///
/// Org scope mapping:
/// - `OrgRole::Owner` or `OrgRole::Admin` -> `org:{name}:admin`
/// - `OrgRole::Member` or `OrgRole::Viewer` -> `org:{name}:member`
pub fn compute_scopes_from_memberships(
    user: &User,
    memberships: &[UserTeamMembership],
    org_memberships: &[OrganizationMembership],
) -> Vec<String> {
    let mut scopes = Vec::new();

    if user.is_admin {
        // Admin users get admin:all scope plus team-scoped permissions
        // This ensures extract_teams_from_scopes() can extract team names
        // for dashboard and UI components that need team context
        scopes.push("admin:all".to_string());
    }

    // Include team-scoped permissions from memberships
    let team_scopes: Vec<String> =
        memberships.iter().flat_map(|m| m.scopes.iter()).map(|s| s.to_string()).collect();
    scopes.extend(team_scopes);

    // Include org scopes from org memberships
    for org_mem in org_memberships {
        let org_scope = match org_mem.role {
            OrgRole::Owner | OrgRole::Admin => format!("org:{}:admin", org_mem.org_name),
            OrgRole::Member | OrgRole::Viewer => format!("org:{}:member", org_mem.org_name),
        };
        scopes.push(org_scope);
    }

    // Remove duplicates
    scopes.sort();
    scopes.dedup();

    scopes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::UserId;
    use chrono::Utc;

    #[test]
    fn compute_scopes_admin_user() {
        let user = User {
            id: UserId::new(),
            email: "admin@example.com".to_string(),
            name: "Admin User".to_string(),
            status: UserStatus::Active,
            is_admin: true,
            org_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let memberships = vec![UserTeamMembership {
            id: "m1".to_string(),
            user_id: user.id.clone(),
            team: "platform-admin".to_string(),
            scopes: vec!["team:platform-admin:*:*".to_string()],
            created_at: Utc::now(),
        }];

        let scopes = compute_scopes_from_memberships(&user, &memberships, &[]);

        // Admin users should get both admin:all and team-scoped permissions
        assert!(scopes.contains(&"admin:all".to_string()));
        assert!(scopes.contains(&"team:platform-admin:*:*".to_string()));
        assert_eq!(scopes.len(), 2);
    }

    #[test]
    fn compute_scopes_regular_user_single_team() {
        let user = User {
            id: UserId::new(),
            email: "user@example.com".to_string(),
            name: "Regular User".to_string(),
            status: UserStatus::Active,
            is_admin: false,
            org_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let memberships = vec![UserTeamMembership {
            id: "m1".to_string(),
            user_id: user.id.clone(),
            team: "team-a".to_string(),
            scopes: vec!["clusters:read".to_string(), "clusters:write".to_string()],
            created_at: Utc::now(),
        }];

        let scopes = compute_scopes_from_memberships(&user, &memberships, &[]);

        assert_eq!(scopes, vec!["clusters:read", "clusters:write"]);
    }

    #[test]
    fn compute_scopes_regular_user_multiple_teams() {
        let user = User {
            id: UserId::new(),
            email: "user@example.com".to_string(),
            name: "Regular User".to_string(),
            status: UserStatus::Active,
            is_admin: false,
            org_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let memberships = vec![
            UserTeamMembership {
                id: "m1".to_string(),
                user_id: user.id.clone(),
                team: "team-a".to_string(),
                scopes: vec!["clusters:read".to_string(), "routes:write".to_string()],
                created_at: Utc::now(),
            },
            UserTeamMembership {
                id: "m2".to_string(),
                user_id: user.id.clone(),
                team: "team-b".to_string(),
                scopes: vec!["listeners:read".to_string(), "clusters:read".to_string()],
                created_at: Utc::now(),
            },
        ];

        let scopes = compute_scopes_from_memberships(&user, &memberships, &[]);

        // Should deduplicate clusters:read
        assert_eq!(scopes, vec!["clusters:read", "listeners:read", "routes:write"]);
    }

    #[test]
    fn compute_scopes_user_no_teams() {
        let user = User {
            id: UserId::new(),
            email: "user@example.com".to_string(),
            name: "Regular User".to_string(),
            status: UserStatus::Active,
            is_admin: false,
            org_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let memberships = vec![];

        let scopes = compute_scopes_from_memberships(&user, &memberships, &[]);

        assert_eq!(scopes, Vec::<String>::new());
    }

    #[test]
    fn compute_scopes_includes_org_scopes() {
        use crate::auth::organization::OrganizationMembership;
        use crate::domain::OrgId;

        let user = User {
            id: UserId::new(),
            email: "user@example.com".to_string(),
            name: "Org User".to_string(),
            status: UserStatus::Active,
            is_admin: false,
            org_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let memberships = vec![UserTeamMembership {
            id: "m1".to_string(),
            user_id: user.id.clone(),
            team: "team-a".to_string(),
            scopes: vec!["team:team-a:routes:read".to_string()],
            created_at: Utc::now(),
        }];

        let org_memberships = vec![
            OrganizationMembership {
                id: "om1".to_string(),
                user_id: user.id.clone(),
                org_id: OrgId::new(),
                org_name: "acme".to_string(),
                role: crate::auth::organization::OrgRole::Admin,
                created_at: Utc::now(),
            },
            OrganizationMembership {
                id: "om2".to_string(),
                user_id: user.id.clone(),
                org_id: OrgId::new(),
                org_name: "globex".to_string(),
                role: crate::auth::organization::OrgRole::Member,
                created_at: Utc::now(),
            },
        ];

        let scopes = compute_scopes_from_memberships(&user, &memberships, &org_memberships);

        assert!(scopes.contains(&"team:team-a:routes:read".to_string()));
        assert!(scopes.contains(&"org:acme:admin".to_string()));
        assert!(scopes.contains(&"org:globex:member".to_string()));
        assert_eq!(scopes.len(), 3);
    }

    #[test]
    fn compute_scopes_org_owner_gets_admin_scope() {
        use crate::auth::organization::OrganizationMembership;
        use crate::domain::OrgId;

        let user = User {
            id: UserId::new(),
            email: "owner@example.com".to_string(),
            name: "Org Owner".to_string(),
            status: UserStatus::Active,
            is_admin: false,
            org_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let org_memberships = vec![OrganizationMembership {
            id: "om1".to_string(),
            user_id: user.id.clone(),
            org_id: OrgId::new(),
            org_name: "acme".to_string(),
            role: crate::auth::organization::OrgRole::Owner,
            created_at: Utc::now(),
        }];

        let scopes = compute_scopes_from_memberships(&user, &[], &org_memberships);

        // Owner role maps to org:admin scope
        assert!(scopes.contains(&"org:acme:admin".to_string()));
        assert_eq!(scopes.len(), 1);
    }

    #[test]
    fn admin_scopes_work_with_extract_teams() {
        use crate::auth::session::extract_teams_from_scopes;

        let user = User {
            id: UserId::new(),
            email: "admin@example.com".to_string(),
            name: "Admin User".to_string(),
            status: UserStatus::Active,
            is_admin: true,
            org_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let memberships = vec![UserTeamMembership {
            id: "m1".to_string(),
            user_id: user.id.clone(),
            team: "platform-admin".to_string(),
            scopes: vec!["team:platform-admin:*:*".to_string()],
            created_at: Utc::now(),
        }];

        let scopes = compute_scopes_from_memberships(&user, &memberships, &[]);

        // Verify scopes include both admin:all and team scopes
        assert!(scopes.contains(&"admin:all".to_string()));
        assert!(scopes.contains(&"team:platform-admin:*:*".to_string()));

        // Verify extract_teams_from_scopes can extract team names
        let teams = extract_teams_from_scopes(&scopes);
        assert_eq!(teams, vec!["platform-admin"]);
    }
}
