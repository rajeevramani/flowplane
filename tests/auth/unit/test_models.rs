use chrono::Utc;
use flowplane::auth::models::{AuthContext, PersonalAccessToken, TokenScope, TokenStatus};

#[test]
fn token_status_parse_and_display() {
    assert_eq!("active".parse::<TokenStatus>().unwrap(), TokenStatus::Active);
    assert_eq!(TokenStatus::Revoked.to_string(), "revoked");
    assert!("invalid".parse::<TokenStatus>().is_err());
}

#[test]
fn auth_context_scope_lookup() {
    let ctx = AuthContext::new(
        "abc123".to_string(),
        "demo".to_string(),
        vec!["clusters:read".to_string(), "routes:write".to_string()],
    );

    assert!(ctx.has_scope("clusters:read"));
    assert!(!ctx.has_scope("listeners:read"));
    assert_eq!(ctx.scopes().count(), 2);
}

#[test]
fn token_has_scope_helper() {
    let token = PersonalAccessToken {
        id: "tok".into(),
        name: "demo".into(),
        description: Some("test".into()),
        status: TokenStatus::Active,
        expires_at: None,
        last_used_at: None,
        created_by: Some("user".into()),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        scopes: vec!["listeners:write".into(), "listeners:read".into()],
    };

    assert!(token.has_scope("listeners:read"));
    assert!(!token.has_scope("clusters:read"));
}

#[test]
fn token_scope_display_wrapper() {
    let scope = TokenScope("clusters:write".into());
    assert_eq!(scope.to_string(), "clusters:write");
}
