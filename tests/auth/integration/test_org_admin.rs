#![cfg(feature = "postgres_tests")]

//! Integration tests for organization admin authorization.
//!
//! Tests the authorization functions related to org scopes: parsing, checking
//! admin/member access, scope validation, and privilege escalation prevention.

use flowplane::auth::authorization::{
    extract_org_scopes, has_admin_bypass, has_org_admin, has_org_membership, parse_org_from_scope,
    require_org_admin,
};
use flowplane::auth::models::{AuthContext, AuthError};
use flowplane::domain::TokenId;

// ---------------------------------------------------------------------------
// Scope parsing tests
// ---------------------------------------------------------------------------

#[test]
fn parse_org_scope_admin() {
    let result = parse_org_from_scope("org:acme:admin");
    assert_eq!(result, Some(("acme".to_string(), "admin".to_string())));
}

#[test]
fn parse_org_scope_member() {
    let result = parse_org_from_scope("org:acme:member");
    assert_eq!(result, Some(("acme".to_string(), "member".to_string())));
}

#[test]
fn parse_org_scope_invalid_role_rejected() {
    // Only "admin" and "member" are valid org roles in scopes
    assert_eq!(parse_org_from_scope("org:acme:viewer"), None);
    assert_eq!(parse_org_from_scope("org:acme:owner"), None);
    assert_eq!(parse_org_from_scope("org:acme:*"), None);
}

#[test]
fn parse_org_scope_wrong_prefix_rejected() {
    assert_eq!(parse_org_from_scope("team:acme:admin"), None);
    assert_eq!(parse_org_from_scope("routes:read"), None);
}

#[test]
fn parse_org_scope_too_few_parts_rejected() {
    assert_eq!(parse_org_from_scope("org:acme"), None);
    assert_eq!(parse_org_from_scope("org"), None);
}

#[test]
fn parse_org_scope_too_many_parts_rejected() {
    assert_eq!(parse_org_from_scope("org:acme:admin:extra"), None);
}

// ---------------------------------------------------------------------------
// has_org_admin tests
// ---------------------------------------------------------------------------

#[test]
fn has_org_admin_returns_true_for_org_admin() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("test-token"),
        "test".into(),
        vec!["org:acme:admin".into()],
    );
    assert!(has_org_admin(&ctx, "acme"));
}

#[test]
fn has_org_admin_returns_false_for_org_member() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("test-token"),
        "test".into(),
        vec!["org:acme:member".into()],
    );
    assert!(!has_org_admin(&ctx, "acme"));
}

#[test]
fn has_org_admin_returns_false_for_different_org() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("test-token"),
        "test".into(),
        vec!["org:acme:admin".into()],
    );
    assert!(!has_org_admin(&ctx, "globex"));
}

#[test]
fn has_org_admin_platform_admin_bypass() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("admin-token"),
        "admin".into(),
        vec!["admin:all".into()],
    );
    assert!(has_org_admin(&ctx, "acme"));
    assert!(has_org_admin(&ctx, "any-org"));
}

// ---------------------------------------------------------------------------
// require_org_admin tests
// ---------------------------------------------------------------------------

#[test]
fn require_org_admin_ok_for_admin() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("test-token"),
        "test".into(),
        vec!["org:acme:admin".into()],
    );
    assert!(require_org_admin(&ctx, "acme").is_ok());
}

#[test]
fn require_org_admin_forbidden_for_member() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("test-token"),
        "test".into(),
        vec!["org:acme:member".into()],
    );
    let err = require_org_admin(&ctx, "acme").unwrap_err();
    assert!(matches!(err, AuthError::Forbidden));
}

#[test]
fn require_org_admin_forbidden_for_no_org_scopes() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("test-token"),
        "test".into(),
        vec!["routes:read".into()],
    );
    let err = require_org_admin(&ctx, "acme").unwrap_err();
    assert!(matches!(err, AuthError::Forbidden));
}

// ---------------------------------------------------------------------------
// has_org_membership tests
// ---------------------------------------------------------------------------

#[test]
fn has_org_membership_true_for_admin() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("test-token"),
        "test".into(),
        vec!["org:acme:admin".into()],
    );
    assert!(has_org_membership(&ctx, "acme"));
}

#[test]
fn has_org_membership_true_for_member() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("test-token"),
        "test".into(),
        vec!["org:acme:member".into()],
    );
    assert!(has_org_membership(&ctx, "acme"));
}

#[test]
fn has_org_membership_false_for_wrong_org() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("test-token"),
        "test".into(),
        vec!["org:acme:admin".into()],
    );
    assert!(!has_org_membership(&ctx, "globex"));
}

// ---------------------------------------------------------------------------
// extract_org_scopes tests
// ---------------------------------------------------------------------------

#[test]
fn extract_org_scopes_returns_all_org_pairs() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("multi-org"),
        "multi".into(),
        vec![
            "org:acme:admin".into(),
            "org:globex:member".into(),
            "team:platform:routes:read".into(), // Not an org scope
            "routes:read".into(),               // Not an org scope
        ],
    );

    let orgs = extract_org_scopes(&ctx);
    assert_eq!(orgs.len(), 2);
    assert!(orgs.contains(&("acme".into(), "admin".into())));
    assert!(orgs.contains(&("globex".into(), "member".into())));
}

#[test]
fn extract_org_scopes_empty_for_no_org_scopes() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("no-org"),
        "no-org".into(),
        vec!["routes:read".into(), "clusters:write".into()],
    );

    let orgs = extract_org_scopes(&ctx);
    assert!(orgs.is_empty());
}

// ---------------------------------------------------------------------------
// strip_org_scopes tests
// ---------------------------------------------------------------------------

#[test]
fn strip_org_scopes_removes_only_org_scopes() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("strip-test"),
        "strip".into(),
        vec![
            "org:acme:admin".into(),
            "org:globex:member".into(),
            "routes:read".into(),
            "team:platform:routes:write".into(),
            "admin:all".into(),
        ],
    );

    let stripped = ctx.strip_org_scopes();

    // Org scopes removed
    assert!(!stripped.has_scope("org:acme:admin"));
    assert!(!stripped.has_scope("org:globex:member"));

    // Non-org scopes preserved
    assert!(stripped.has_scope("routes:read"));
    assert!(stripped.has_scope("team:platform:routes:write"));
    assert!(stripped.has_scope("admin:all"));

    assert_eq!(stripped.scopes().count(), 3);
}

#[test]
fn strip_org_scopes_noop_when_no_org_scopes() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("no-org-strip"),
        "no-org".into(),
        vec!["routes:read".into(), "admin:all".into()],
    );

    let stripped = ctx.strip_org_scopes();
    assert_eq!(stripped.scopes().count(), 2);
    assert!(stripped.has_scope("routes:read"));
    assert!(stripped.has_scope("admin:all"));
}

// ---------------------------------------------------------------------------
// Cross-org scope detection
// ---------------------------------------------------------------------------

#[test]
fn context_with_multiple_org_scopes_detected() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("cross-org"),
        "cross".into(),
        vec!["org:acme:admin".into(), "org:globex:admin".into()],
    );

    let orgs = extract_org_scopes(&ctx);
    assert_eq!(orgs.len(), 2, "Should detect scopes from two different orgs");

    // Verify each org is detected
    let org_names: Vec<&str> = orgs.iter().map(|(name, _)| name.as_str()).collect();
    assert!(org_names.contains(&"acme"));
    assert!(org_names.contains(&"globex"));
}

// ---------------------------------------------------------------------------
// Platform admin has org admin for any org
// ---------------------------------------------------------------------------

#[test]
fn platform_admin_has_all_org_admin() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("platform-admin"),
        "admin".into(),
        vec!["admin:all".into()],
    );

    assert!(has_admin_bypass(&ctx));
    assert!(has_org_admin(&ctx, "any-org"));
    assert!(has_org_admin(&ctx, "another-org"));
    assert!(has_org_membership(&ctx, "any-org"));
    assert!(require_org_admin(&ctx, "any-org").is_ok());
}

// ---------------------------------------------------------------------------
// Org context on AuthContext
// ---------------------------------------------------------------------------

#[test]
fn auth_context_with_org() {
    let org_id = flowplane::domain::OrgId::new();
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("org-ctx"),
        "test".into(),
        vec!["org:acme:admin".into()],
    )
    .with_org(org_id.clone(), "acme".to_string());

    assert_eq!(ctx.org_id.as_ref().map(|id| id.as_str()), Some(org_id.as_str()));
    assert_eq!(ctx.org_name.as_deref(), Some("acme"));
}

// ---------------------------------------------------------------------------
// Org admin can manage teams in their org (scope check)
// ---------------------------------------------------------------------------

#[test]
fn org_admin_can_manage_own_org_teams() {
    // Org admin has org:acme:admin scope
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("org-admin"),
        "org-admin".into(),
        vec![
            "org:acme:admin".into(),
            "team:engineering:routes:read".into(),
            "team:engineering:clusters:write".into(),
        ],
    );

    // Has admin access to their own org
    assert!(has_org_admin(&ctx, "acme"));
    assert!(require_org_admin(&ctx, "acme").is_ok());

    // Has membership in their org
    assert!(has_org_membership(&ctx, "acme"));
}

// ---------------------------------------------------------------------------
// Org admin CANNOT access another org's teams
// ---------------------------------------------------------------------------

#[test]
fn org_admin_cannot_access_other_org_teams() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("org-admin"),
        "org-admin".into(),
        vec!["org:acme:admin".into()],
    );

    // NOT admin of other org
    assert!(!has_org_admin(&ctx, "globex"));
    assert!(!has_org_admin(&ctx, "other-org"));
    assert!(require_org_admin(&ctx, "globex").is_err());

    // NOT a member of other org
    assert!(!has_org_membership(&ctx, "globex"));
}

// ---------------------------------------------------------------------------
// Org member does NOT have org admin
// ---------------------------------------------------------------------------

#[test]
fn org_member_is_not_org_admin() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("member"),
        "member".into(),
        vec!["org:acme:member".into()],
    );

    // Member has membership but NOT admin
    assert!(has_org_membership(&ctx, "acme"));
    assert!(!has_org_admin(&ctx, "acme"));

    // require_org_admin fails for member
    let err = require_org_admin(&ctx, "acme").unwrap_err();
    assert!(matches!(err, AuthError::Forbidden));
}

// ---------------------------------------------------------------------------
// is_global_resource_scope correctly classifies scope types
// ---------------------------------------------------------------------------

#[test]
fn is_global_resource_scope_security_classification() {
    use flowplane::auth::authorization::is_global_resource_scope;

    // Dangerous global scopes that should be restricted to platform admins
    assert!(is_global_resource_scope("clusters:read"), "clusters:read is global");
    assert!(is_global_resource_scope("routes:write"), "routes:write is global");
    assert!(is_global_resource_scope("listeners:read"), "listeners:read is global");
    assert!(is_global_resource_scope("openapi-import:write"), "openapi-import:write is global");

    // Safe scopes (not global resource scopes)
    assert!(!is_global_resource_scope("admin:all"), "admin:all is NOT a global resource scope");
    assert!(
        !is_global_resource_scope("team:eng:routes:read"),
        "team-prefixed scopes are NOT global"
    );
    assert!(
        !is_global_resource_scope("org:acme:admin"),
        "org scopes are NOT global resource scopes"
    );
    assert!(
        !is_global_resource_scope("team:platform:*:*"),
        "wildcards are NOT global resource scopes"
    );
}

// ---------------------------------------------------------------------------
// verify_org_boundary with different org combinations
// ---------------------------------------------------------------------------

#[test]
fn verify_org_boundary_comprehensive() {
    use flowplane::api::error::ApiError;
    use flowplane::auth::authorization::verify_org_boundary;
    use flowplane::domain::OrgId;

    let org_a = OrgId::new();
    let org_b = OrgId::new();

    // Case 1: Same org -- allowed
    let ctx_a = AuthContext::new(
        TokenId::from_str_unchecked("user-a"),
        "user-a".into(),
        vec!["team:eng:routes:read".into()],
    )
    .with_org(org_a.clone(), "alpha".into());
    assert!(verify_org_boundary(&ctx_a, &Some(org_a.clone())).is_ok());

    // Case 2: Different org -- returns NotFound (not Forbidden, to prevent enumeration)
    let result = verify_org_boundary(&ctx_a, &Some(org_b.clone()));
    assert!(result.is_err());
    assert!(
        matches!(result, Err(ApiError::NotFound(_))),
        "Cross-org should return NotFound, not Forbidden"
    );

    // Case 3: User has no org, team has org -- denied
    let ctx_no_org = AuthContext::new(
        TokenId::from_str_unchecked("no-org"),
        "no-org".into(),
        vec!["routes:read".into()],
    );
    assert!(verify_org_boundary(&ctx_no_org, &Some(org_a.clone())).is_err());

    // Case 4: User has org, team has no org (global) -- allowed
    assert!(verify_org_boundary(&ctx_a, &None).is_ok());

    // Case 5: Both have no org -- allowed
    assert!(verify_org_boundary(&ctx_no_org, &None).is_ok());

    // Case 6: Admin bypasses all checks
    let admin_ctx = AuthContext::new(
        TokenId::from_str_unchecked("admin"),
        "admin".into(),
        vec!["admin:all".into()],
    );
    assert!(verify_org_boundary(&admin_ctx, &Some(org_a)).is_ok());
    assert!(verify_org_boundary(&admin_ctx, &Some(org_b)).is_ok());
    assert!(verify_org_boundary(&admin_ctx, &None).is_ok());
}

// ---------------------------------------------------------------------------
// check_resource_access comprehensive org+team scenarios
// ---------------------------------------------------------------------------

#[test]
fn check_resource_access_org_team_matrix() {
    use flowplane::auth::authorization::check_resource_access;

    // Scenario 1: User with team scope can access their team
    let team_user = AuthContext::new(
        TokenId::from_str_unchecked("team-user"),
        "team-user".into(),
        vec!["team:engineering:clusters:read".into(), "team:engineering:routes:write".into()],
    );
    assert!(check_resource_access(&team_user, "clusters", "read", Some("engineering")));
    assert!(check_resource_access(&team_user, "routes", "write", Some("engineering")));

    // Scenario 2: User CANNOT access other teams
    assert!(!check_resource_access(&team_user, "clusters", "read", Some("platform")));
    assert!(!check_resource_access(&team_user, "routes", "write", Some("platform")));

    // Scenario 3: User with global scope but NOT admin is denied
    let global_user = AuthContext::new(
        TokenId::from_str_unchecked("global-user"),
        "global-user".into(),
        vec!["clusters:read".into()],
    );
    assert!(!check_resource_access(&global_user, "clusters", "read", None));
    assert!(!check_resource_access(&global_user, "clusters", "read", Some("engineering")));

    // Scenario 4: Admin bypasses everything
    let admin = AuthContext::new(
        TokenId::from_str_unchecked("admin"),
        "admin".into(),
        vec!["admin:all".into()],
    );
    assert!(check_resource_access(&admin, "clusters", "read", None));
    assert!(check_resource_access(&admin, "routes", "delete", Some("any-team")));
}

// ---------------------------------------------------------------------------
// Cross-org token scope validation edge cases
// ---------------------------------------------------------------------------

#[test]
fn token_with_cross_org_scopes_are_flagged() {
    // A token that somehow has admin scope in two different orgs
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("cross-org-attempt"),
        "cross-org".into(),
        vec!["org:acme:admin".into(), "org:globex:admin".into()],
    );

    let org_scopes = extract_org_scopes(&ctx);
    let unique_org_names: std::collections::BTreeSet<&str> =
        org_scopes.iter().map(|(name, _)| name.as_str()).collect();

    assert_eq!(unique_org_names.len(), 2, "Should detect 2 different org scopes");
    assert!(unique_org_names.contains("acme"));
    assert!(unique_org_names.contains("globex"));

    // Such tokens should be detectable so the system can reject them
    let is_cross_org = unique_org_names.len() > 1;
    assert!(is_cross_org, "Cross-org tokens must be flagged");
}

// ---------------------------------------------------------------------------
// Org admin with additional team scopes
// ---------------------------------------------------------------------------

#[test]
fn org_admin_with_team_scopes_retains_both() {
    use flowplane::auth::authorization::check_resource_access;

    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("org-admin-with-teams"),
        "org-team-user".into(),
        vec![
            "org:acme:admin".into(),
            "team:engineering:clusters:read".into(),
            "team:engineering:routes:write".into(),
        ],
    );

    // Has org admin for acme
    assert!(has_org_admin(&ctx, "acme"));
    assert!(has_org_membership(&ctx, "acme"));

    // Has team-scoped access to engineering
    assert!(check_resource_access(&ctx, "clusters", "read", Some("engineering")));
    assert!(check_resource_access(&ctx, "routes", "write", Some("engineering")));

    // Does NOT have access to other teams
    assert!(!check_resource_access(&ctx, "clusters", "read", Some("platform")));
    assert!(!check_resource_access(&ctx, "routes", "write", Some("platform")));

    // Does NOT have org admin for other orgs
    assert!(!has_org_admin(&ctx, "globex"));
    assert!(!has_org_membership(&ctx, "globex"));
}

// ---------------------------------------------------------------------------
// Org boundary check: no org on user + no org on team = allowed
// ---------------------------------------------------------------------------

#[test]
fn no_org_user_accessing_no_org_team_allowed() {
    use flowplane::auth::authorization::verify_org_boundary;

    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("legacy-user"),
        "legacy".into(),
        vec!["team:eng:routes:read".into()],
    );

    // Both user and team have no org -- allowed (backward compat)
    assert!(verify_org_boundary(&ctx, &None).is_ok());
}

// ---------------------------------------------------------------------------
// Org scope does NOT grant team-level resource access
// ---------------------------------------------------------------------------

#[test]
fn org_scope_does_not_grant_resource_access() {
    use flowplane::auth::authorization::check_resource_access;

    // User has only org:acme:admin, no team scopes
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("org-only-user"),
        "org-only".into(),
        vec!["org:acme:admin".into()],
    );

    // Org admin scope does NOT grant resource access (that requires team scopes)
    assert!(!check_resource_access(&ctx, "clusters", "read", Some("engineering")));
    assert!(!check_resource_access(&ctx, "routes", "write", Some("platform")));
    assert!(!check_resource_access(&ctx, "clusters", "read", None));
}

// ---------------------------------------------------------------------------
// Empty scopes means no access
// ---------------------------------------------------------------------------

#[test]
fn empty_scopes_no_access() {
    use flowplane::auth::authorization::check_resource_access;

    let ctx = AuthContext::new(TokenId::from_str_unchecked("empty-scopes"), "empty".into(), vec![]);

    assert!(!has_admin_bypass(&ctx));
    assert!(!has_org_admin(&ctx, "acme"));
    assert!(!has_org_membership(&ctx, "acme"));
    assert!(!check_resource_access(&ctx, "clusters", "read", None));
    assert!(!check_resource_access(&ctx, "routes", "write", Some("engineering")));

    let org_scopes = extract_org_scopes(&ctx);
    assert!(org_scopes.is_empty());
}
