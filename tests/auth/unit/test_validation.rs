use chrono::Utc;
use flowplane::auth::validation::{CreateTokenRequest, UpdateTokenRequest};
use validator::Validate;

#[test]
fn create_request_validates_required_fields() {
    let mut request = CreateTokenRequest {
        name: "valid-name".into(),
        description: None,
        expires_at: None,
        scopes: vec!["clusters:read".into()],
        created_by: None,
    };
    assert!(request.validate().is_ok());

    request.name = "!bad".into();
    assert!(request.validate().is_err());
}

#[test]
fn update_request_allows_optional_fields() {
    let mut request = UpdateTokenRequest {
        name: Some("new-name".into()),
        description: Some("desc".into()),
        status: Some("revoked".into()),
        expires_at: Some(Some(Utc::now())),
        scopes: Some(vec!["clusters:write".into()]),
    };
    assert!(request.validate().is_ok());

    request.scopes = Some(vec!["bad_scope".into()]);
    assert!(request.validate().is_err());
}
