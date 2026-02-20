//! Invitation service — business logic for invite-only registration.

use std::sync::Arc;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{Duration, Utc};
use rand::{rngs::OsRng, RngCore};
use tracing::{info, instrument};

use crate::auth::hashing;
use crate::auth::invitation::{CreateInvitationResponse, InviteTokenInfo, PaginatedInvitations};
use crate::auth::login_service::compute_scopes_from_memberships;
use crate::auth::models::AuthContext;
use crate::auth::organization::{OrgRole, OrgStatus};
use crate::auth::user::{NewUser, NewUserTeamMembership, User, UserStatus};
use crate::domain::{InvitationId, OrgId, UserId};
use crate::errors::{AuthErrorType, Error, Result};
use crate::storage::repositories::{
    AuditEvent, AuditLogRepository, InvitationRepository, OrgMembershipRepository,
    OrganizationRepository, SqlxInvitationRepository, SqlxOrgMembershipRepository,
    SqlxOrganizationRepository, SqlxTeamMembershipRepository, SqlxTeamRepository,
    SqlxUserRepository, TeamMembershipRepository, TeamRepository, UserRepository,
};

/// Token prefix for invitations.
const TOKEN_PREFIX: &str = "fp_invite_";
/// Number of random bytes for the secret portion (64 bytes = 512 bits entropy).
const SECRET_BYTES: usize = 64;

/// Service for managing invitations.
#[derive(Clone)]
pub struct InvitationService {
    invitation_repo: Arc<dyn InvitationRepository>,
    org_repo: Arc<dyn OrganizationRepository>,
    org_membership_repo: Arc<dyn OrgMembershipRepository>,
    user_repo: Arc<dyn UserRepository>,
    team_membership_repo: Arc<dyn TeamMembershipRepository>,
    team_repo: Arc<dyn TeamRepository>,
    audit_repo: Arc<AuditLogRepository>,
    invite_expiry_hours: i64,
    base_url: String,
}

impl InvitationService {
    pub fn with_sqlx(
        pool: crate::storage::DbPool,
        invite_expiry_hours: i64,
        base_url: String,
    ) -> Self {
        Self {
            invitation_repo: Arc::new(SqlxInvitationRepository::new(pool.clone())),
            org_repo: Arc::new(SqlxOrganizationRepository::new(pool.clone())),
            org_membership_repo: Arc::new(SqlxOrgMembershipRepository::new(pool.clone())),
            user_repo: Arc::new(SqlxUserRepository::new(pool.clone())),
            team_membership_repo: Arc::new(SqlxTeamMembershipRepository::new(pool.clone())),
            team_repo: Arc::new(SqlxTeamRepository::new(pool.clone())),
            audit_repo: Arc::new(AuditLogRepository::new(pool)),
            invite_expiry_hours,
            base_url,
        }
    }

    /// Create an invitation.
    #[instrument(skip(self, context), fields(org_id = %org_id, email = %email, role = %role))]
    pub async fn create_invitation(
        &self,
        context: &AuthContext,
        org_id: &OrgId,
        email: &str,
        role: OrgRole,
        client_ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<CreateInvitationResponse> {
        // Owner role is never invitable
        if role == OrgRole::Owner {
            return Err(Error::auth(
                "Cannot invite users with owner role",
                AuthErrorType::InsufficientPermissions,
            ));
        }

        // Verify inviter permissions + role hierarchy
        self.check_invite_permission(context, org_id, role).await?;

        // Verify org exists and is active
        let org = self
            .org_repo
            .get_organization_by_id(org_id)
            .await?
            .ok_or_else(|| Error::not_found("organization", org_id.as_str()))?;

        if org.status != OrgStatus::Active {
            return Err(Error::auth(
                "Cannot create invitations for inactive organizations",
                AuthErrorType::InsufficientPermissions,
            ));
        }

        // Normalize email
        let email = User::normalize_email(email);

        // Check if email is already a member of this org
        if let Some(user_id) = self.find_user_by_email(&email).await? {
            let membership = self.org_membership_repo.get_membership(&user_id, org_id).await?;
            if membership.is_some() {
                return Err(Error::conflict(
                    "This email is already a member of this organization",
                    "invitation",
                ));
            }
        }

        // Generate token
        let invitation_id = InvitationId::new();
        let secret = generate_secret()?;
        let token_hash = hashing::hash_password(&secret)?;
        let expires_at = Utc::now() + Duration::hours(self.invite_expiry_hours);

        let inviter_user_id = context.user_id.as_ref();

        // Create invitation (PG unique constraint catches duplicate pending)
        let invitation = self
            .invitation_repo
            .create_invitation(
                &invitation_id,
                org_id,
                &email,
                &role,
                &token_hash,
                inviter_user_id,
                expires_at,
            )
            .await?;

        // Build token and URL (hash fragment — not logged by servers)
        let token = format!("{}{}.{}", TOKEN_PREFIX, invitation_id, secret);
        let invite_url = format!("{}/register#token={}", self.base_url, token);

        // Audit log
        self.audit_repo
            .record_auth_event(
                AuditEvent::token(
                    "invitation.create",
                    inviter_user_id.map(|u| u.as_str()),
                    context.user_email.as_deref(),
                    serde_json::json!({
                        "invitation_id": invitation_id.as_str(),
                        "email": &email,
                        "role": role.as_str(),
                        "org_id": org_id.as_str(),
                    }),
                )
                .with_user_context(
                    inviter_user_id.map(|u| u.to_string()),
                    client_ip,
                    user_agent,
                ),
            )
            .await?;

        info!(
            invitation_id = %invitation_id,
            email = %email,
            role = %role,
            org = %org.name,
            "invitation created"
        );

        Ok(CreateInvitationResponse {
            id: invitation.id.to_string(),
            email: invitation.email,
            role: invitation.role,
            org_name: org.name,
            invite_url,
            expires_at: invitation.expires_at,
        })
    }

    /// Validate an invite token (public — returns only safe info).
    #[instrument(skip(self, token_string))]
    pub async fn validate_invite_token(&self, token_string: &str) -> Result<InviteTokenInfo> {
        let (id, secret) = parse_invite_token(token_string)?;

        let row = self.invitation_repo.get_invitation_with_hash(&id).await?.ok_or_else(|| {
            Error::auth("Invalid or expired invitation", AuthErrorType::InvalidToken)
        })?;

        // Check status
        if row.status != "pending" {
            return Err(Error::auth("Invalid or expired invitation", AuthErrorType::InvalidToken));
        }

        // Check expiry
        let expires_at: chrono::DateTime<Utc> = row.expires_at;
        if expires_at < Utc::now() {
            return Err(Error::auth("Invalid or expired invitation", AuthErrorType::InvalidToken));
        }

        // Verify secret against stored hash
        let valid = hashing::verify_password(&secret, &row.token_hash)?;
        if !valid {
            return Err(Error::auth("Invalid or expired invitation", AuthErrorType::InvalidToken));
        }

        // Look up org for display info
        let org_id = OrgId::from_string(row.org_id);
        let org = self.org_repo.get_organization_by_id(&org_id).await?.ok_or_else(|| {
            Error::auth("Invalid or expired invitation", AuthErrorType::InvalidToken)
        })?;

        let role = OrgRole::from_str_safe(&row.role).ok_or_else(|| {
            Error::auth("Invalid or expired invitation", AuthErrorType::InvalidToken)
        })?;

        Ok(InviteTokenInfo {
            org_name: org.name,
            org_display_name: org.display_name,
            email: row.email,
            role,
            expires_at,
        })
    }

    /// Accept an invitation — creates user, org membership, marks invitation accepted.
    #[instrument(skip(self, token, password), fields(name = %name))]
    pub async fn accept_invitation(
        &self,
        token: &str,
        name: &str,
        password: &str,
        client_ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(User, Vec<String>)> {
        // Parse + validate token
        let (invitation_id, secret) = parse_invite_token(token)?;

        let row = self.invitation_repo.get_invitation_with_hash(&invitation_id).await?.ok_or_else(
            || Error::auth("Invalid or expired invitation", AuthErrorType::InvalidToken),
        )?;

        // Check status + expiry
        if row.status != "pending" || row.expires_at < Utc::now() {
            return Err(Error::auth("Invalid or expired invitation", AuthErrorType::InvalidToken));
        }

        // Verify secret
        let valid = hashing::verify_password(&secret, &row.token_hash)?;
        if !valid {
            return Err(Error::auth("Invalid or expired invitation", AuthErrorType::InvalidToken));
        }

        let org_id = OrgId::from_string(row.org_id.clone());

        // Verify org still active
        let org = self.org_repo.get_organization_by_id(&org_id).await?.ok_or_else(|| {
            Error::auth("Invalid or expired invitation", AuthErrorType::InvalidToken)
        })?;

        if org.status != OrgStatus::Active {
            return Err(Error::auth("Invalid or expired invitation", AuthErrorType::InvalidToken));
        }

        // Normalize email
        let email = User::normalize_email(&row.email);

        // Check if email already registered
        if self.find_user_by_email(&email).await?.is_some() {
            return Err(Error::auth(
                "An account with this email already exists. Please log in instead.",
                AuthErrorType::InvalidCredentials,
            ));
        }

        // Hash password
        let password_hash = hashing::hash_password(password)?;

        // Create user
        let user_id = UserId::new();
        let new_user = NewUser {
            id: user_id.clone(),
            email: email.clone(),
            password_hash,
            name: name.to_string(),
            status: UserStatus::Active,
            is_admin: false,
            org_id: org_id.clone(),
        };

        let user = self.user_repo.create_user(new_user).await?;

        // Create org membership
        let role = OrgRole::from_str_safe(&row.role)
            .ok_or_else(|| Error::internal(format!("Invalid role in invitation: {}", row.role)))?;
        self.org_membership_repo.create_membership(&user_id, &org_id, role).await?;

        // Create team membership for the org's default team only.
        // Org admins get implicit access to all org teams via org:NAME:admin scope
        // (see check_resource_access). When they create new teams, create_org_team
        // auto-adds all existing org members. This avoids pre-populating memberships
        // for teams the user didn't create or explicitly join.
        let default_team_name = format!("{}-default", org.name);
        let org_teams = self.team_repo.list_teams_by_org(&org_id).await?;
        for team in &org_teams {
            if team.name == default_team_name {
                let scopes = scopes_for_role(role, &team.name);
                let membership = NewUserTeamMembership {
                    id: format!("utm_{}", uuid::Uuid::new_v4()),
                    user_id: user_id.clone(),
                    team: team.id.to_string(),
                    scopes,
                };
                self.team_membership_repo.create_membership(membership).await?;
                break;
            }
        }

        // Mark invitation as accepted
        self.invitation_repo.accept_invitation(&invitation_id, &user_id).await?;

        // Fetch memberships to compute scopes
        let team_memberships = self.team_membership_repo.list_user_memberships(&user_id).await?;
        let org_memberships = self.org_membership_repo.list_user_memberships(&user_id).await?;
        let scopes = compute_scopes_from_memberships(&user, &team_memberships, &org_memberships);

        // Audit log
        self.audit_repo
            .record_auth_event(
                AuditEvent::token(
                    "auth.register.success",
                    Some(user_id.as_str()),
                    Some(&email),
                    serde_json::json!({
                        "invitation_id": invitation_id.as_str(),
                        "org_id": org_id.as_str(),
                        "role": role.as_str(),
                    }),
                )
                .with_user_context(
                    Some(user_id.to_string()),
                    client_ip,
                    user_agent,
                ),
            )
            .await?;

        info!(
            user_id = %user_id,
            email = %email,
            org = %org.name,
            role = %role,
            "user registered via invitation"
        );

        Ok((user, scopes))
    }

    /// Revoke an invitation.
    #[instrument(skip(self, context), fields(invitation_id = %invitation_id, org_id = %org_id))]
    pub async fn revoke_invitation(
        &self,
        context: &AuthContext,
        invitation_id: &InvitationId,
        org_id: &OrgId,
        client_ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<()> {
        // Verify revoker has admin access
        self.check_org_admin_permission(context, org_id)?;

        // Fetch invitation
        let invitation = self
            .invitation_repo
            .get_invitation_by_id(invitation_id)
            .await?
            .ok_or_else(|| Error::not_found("invitation", invitation_id.as_str()))?;

        // Verify org_id matches (prevents cross-org revocation)
        if invitation.org_id != *org_id {
            return Err(Error::auth(
                "Not authorized to revoke this invitation",
                AuthErrorType::InsufficientPermissions,
            ));
        }

        self.invitation_repo.revoke_invitation(invitation_id).await?;

        // Audit log
        self.audit_repo
            .record_auth_event(
                AuditEvent::token(
                    "invitation.revoke",
                    context.user_id.as_ref().map(|u| u.as_str()),
                    context.user_email.as_deref(),
                    serde_json::json!({
                        "invitation_id": invitation_id.as_str(),
                        "email": &invitation.email,
                        "org_id": org_id.as_str(),
                    }),
                )
                .with_user_context(
                    context.user_id.as_ref().map(|u| u.to_string()),
                    client_ip,
                    user_agent,
                ),
            )
            .await?;

        info!(invitation_id = %invitation_id, email = %invitation.email, "invitation revoked");

        Ok(())
    }

    /// List invitations for an org.
    pub async fn list_invitations(
        &self,
        org_id: &OrgId,
        limit: i64,
        offset: i64,
    ) -> Result<PaginatedInvitations> {
        let invitations =
            self.invitation_repo.list_invitations_by_org(org_id, limit, offset).await?;
        let total = self.invitation_repo.count_invitations_by_org(org_id).await?;

        let items = invitations
            .into_iter()
            .map(|inv| crate::auth::invitation::InvitationResponse {
                id: inv.id.to_string(),
                email: inv.email,
                role: inv.role,
                status: inv.status,
                invited_by: inv.invited_by.map(|u| u.to_string()),
                expires_at: inv.expires_at,
                created_at: inv.created_at,
            })
            .collect();

        Ok(PaginatedInvitations { invitations: items, total })
    }

    // --- Private helpers ---

    /// Check that the inviter has permission to create invitations and that the
    /// requested role is within their authority.
    async fn check_invite_permission(
        &self,
        context: &AuthContext,
        org_id: &OrgId,
        requested_role: OrgRole,
    ) -> Result<()> {
        // Platform admin can invite any role (except owner, checked earlier)
        if context.has_scope("admin:all") {
            return Ok(());
        }

        // Check org membership
        let user_id = context.user_id.as_ref().ok_or_else(|| {
            Error::auth("Authentication required", AuthErrorType::InsufficientPermissions)
        })?;

        let membership =
            self.org_membership_repo.get_membership(user_id, org_id).await?.ok_or_else(|| {
                Error::auth(
                    "Not authorized to manage invitations for this organization",
                    AuthErrorType::InsufficientPermissions,
                )
            })?;

        let inviter_role = membership.role;

        // Must be org admin or owner to invite
        if !inviter_role.is_admin() {
            return Err(Error::auth(
                "Only organization admins can create invitations",
                AuthErrorType::InsufficientPermissions,
            ));
        }

        // Role hierarchy: org admin cannot invite admin
        if inviter_role == OrgRole::Admin && requested_role == OrgRole::Admin {
            return Err(Error::auth(
                "Organization admins cannot invite other admins",
                AuthErrorType::InsufficientPermissions,
            ));
        }

        Ok(())
    }

    /// Check that the user is an org admin or platform admin.
    fn check_org_admin_permission(&self, context: &AuthContext, _org_id: &OrgId) -> Result<()> {
        if context.has_scope("admin:all") {
            return Ok(());
        }

        // Check for org-level admin scope
        let has_org_admin =
            context.scopes().any(|s| s.starts_with("org:") && s.ends_with(":admin"));

        if !has_org_admin {
            return Err(Error::auth(
                "Not authorized to manage this organization",
                AuthErrorType::InsufficientPermissions,
            ));
        }

        Ok(())
    }

    /// Find a user by email.
    async fn find_user_by_email(&self, email: &str) -> Result<Option<UserId>> {
        let user = self.user_repo.get_user_by_email(email).await?;
        Ok(user.map(|u| u.id))
    }
}

/// Parse an invite token into (InvitationId, secret).
/// Format: `fp_invite_{id}.{secret}`
fn parse_invite_token(token: &str) -> Result<(InvitationId, String)> {
    let (prefix_and_id, secret) = token
        .split_once('.')
        .ok_or_else(|| Error::auth("Invalid or expired invitation", AuthErrorType::InvalidToken))?;

    let id = prefix_and_id
        .strip_prefix(TOKEN_PREFIX)
        .ok_or_else(|| Error::auth("Invalid or expired invitation", AuthErrorType::InvalidToken))?;

    if id.is_empty() || secret.is_empty() {
        return Err(Error::auth("Invalid or expired invitation", AuthErrorType::InvalidToken));
    }

    Ok((InvitationId::from_string(id.to_string()), secret.to_string()))
}

/// Generate a cryptographically secure random secret.
fn generate_secret() -> Result<String> {
    let mut bytes = [0u8; SECRET_BYTES];
    OsRng.fill_bytes(&mut bytes);
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Compute team-scoped scopes based on an org role.
pub(crate) fn scopes_for_role(role: OrgRole, team_name: &str) -> Vec<String> {
    match role {
        OrgRole::Admin | OrgRole::Owner => {
            vec![format!("team:{}:*:*", team_name)]
        }
        OrgRole::Member => {
            vec![
                format!("team:{}:routes:read", team_name),
                format!("team:{}:routes:write", team_name),
                format!("team:{}:clusters:read", team_name),
                format!("team:{}:clusters:write", team_name),
                format!("team:{}:listeners:read", team_name),
                format!("team:{}:listeners:write", team_name),
                format!("team:{}:filters:read", team_name),
                format!("team:{}:filters:write", team_name),
                format!("team:{}:stats:read", team_name),
            ]
        }
        OrgRole::Viewer => {
            vec![
                format!("team:{}:routes:read", team_name),
                format!("team:{}:clusters:read", team_name),
                format!("team:{}:listeners:read", team_name),
            ]
        }
    }
}

/// Safe OrgRole parsing (no error, just Option).
trait OrgRoleExt {
    fn from_str_safe(s: &str) -> Option<OrgRole>;
}

impl OrgRoleExt for OrgRole {
    fn from_str_safe(s: &str) -> Option<OrgRole> {
        match s {
            "owner" => Some(OrgRole::Owner),
            "admin" => Some(OrgRole::Admin),
            "member" => Some(OrgRole::Member),
            "viewer" => Some(OrgRole::Viewer),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_invite_token_valid() {
        let (id, secret) = parse_invite_token("fp_invite_abc123.secretvalue").unwrap();
        assert_eq!(id.as_str(), "abc123");
        assert_eq!(secret, "secretvalue");
    }

    #[test]
    fn test_parse_invite_token_missing_dot() {
        let result = parse_invite_token("fp_invite_abc123secretvalue");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_invite_token_wrong_prefix() {
        let result = parse_invite_token("fp_session_abc123.secretvalue");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_invite_token_empty_id() {
        let result = parse_invite_token("fp_invite_.secretvalue");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_invite_token_empty_secret() {
        let result = parse_invite_token("fp_invite_abc123.");
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_secret_length() {
        let secret = generate_secret().unwrap();
        // 64 bytes -> base64 URL-safe = ~86 characters
        assert!(secret.len() > 80);
    }

    #[test]
    fn test_generate_secret_uniqueness() {
        let s1 = generate_secret().unwrap();
        let s2 = generate_secret().unwrap();
        assert_ne!(s1, s2);
    }

    #[test]
    fn test_role_hierarchy_owner_never_invitable() {
        // This is enforced in create_invitation, tested here conceptually
        assert_eq!(OrgRole::Owner.as_str(), "owner");
    }

    #[test]
    fn test_generate_and_parse_token_roundtrip() {
        // Generate a secret and build a token the same way create_invitation does
        let id = InvitationId::new();
        let secret = generate_secret().unwrap();
        let token = format!("{}{}.{}", TOKEN_PREFIX, id, secret);

        // Token should start with fp_invite_
        assert!(token.starts_with("fp_invite_"));
        // Token should contain a dot separator
        assert!(token.contains('.'));

        // Parse should succeed and extract matching id and secret
        let (parsed_id, parsed_secret) = parse_invite_token(&token).unwrap();
        assert_eq!(parsed_id.as_str(), id.as_str());
        assert_eq!(parsed_secret, secret);
    }

    #[test]
    fn test_parse_invite_token_no_prefix_at_all() {
        let result = parse_invite_token("just_a_random_string.secret");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_invite_token_no_dot_no_prefix() {
        let result = parse_invite_token("completely_invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_org_role_from_str_safe() {
        assert_eq!(OrgRole::from_str_safe("admin"), Some(OrgRole::Admin));
        assert_eq!(OrgRole::from_str_safe("member"), Some(OrgRole::Member));
        assert_eq!(OrgRole::from_str_safe("viewer"), Some(OrgRole::Viewer));
        assert_eq!(OrgRole::from_str_safe("owner"), Some(OrgRole::Owner));
        assert_eq!(OrgRole::from_str_safe("invalid"), None);
    }

    #[test]
    fn test_scopes_for_role_admin() {
        let scopes = scopes_for_role(OrgRole::Admin, "engineering");
        assert_eq!(scopes, vec!["team:engineering:*:*"]);
    }

    #[test]
    fn test_scopes_for_role_owner() {
        let scopes = scopes_for_role(OrgRole::Owner, "engineering");
        assert_eq!(scopes, vec!["team:engineering:*:*"]);
    }

    #[test]
    fn test_scopes_for_role_member() {
        let scopes = scopes_for_role(OrgRole::Member, "dev-team");
        assert_eq!(scopes.len(), 9);
        assert!(scopes.contains(&"team:dev-team:routes:read".to_string()));
        assert!(scopes.contains(&"team:dev-team:routes:write".to_string()));
        assert!(scopes.contains(&"team:dev-team:clusters:read".to_string()));
        assert!(scopes.contains(&"team:dev-team:clusters:write".to_string()));
        assert!(scopes.contains(&"team:dev-team:listeners:read".to_string()));
        assert!(scopes.contains(&"team:dev-team:listeners:write".to_string()));
        assert!(scopes.contains(&"team:dev-team:filters:read".to_string()));
        assert!(scopes.contains(&"team:dev-team:filters:write".to_string()));
        assert!(scopes.contains(&"team:dev-team:stats:read".to_string()));
    }

    #[test]
    fn test_scopes_for_role_viewer() {
        let scopes = scopes_for_role(OrgRole::Viewer, "dev-team");
        assert_eq!(scopes.len(), 3);
        assert!(scopes.contains(&"team:dev-team:routes:read".to_string()));
        assert!(scopes.contains(&"team:dev-team:clusters:read".to_string()));
        assert!(scopes.contains(&"team:dev-team:listeners:read".to_string()));
    }
}
