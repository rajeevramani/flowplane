use flowplane::auth::validation::{validate_scope, validate_token_name};
use proptest::prelude::*;

proptest! {
    #[test]
    fn valid_token_names(name in "[A-Za-z0-9_\\-]{3,64}") {
        prop_assert!(validate_token_name(&name).is_ok());
    }

    #[test]
    fn invalid_token_names(name in "[A-Za-z0-9_\\- ]{0,2}" ) {
        // Too short or containing space
        prop_assume!(name.len() < 3 || name.contains(' '));
        prop_assert!(validate_token_name(&name).is_err());
    }

    #[test]
    fn valid_scopes(scope in "[a-z]{1,16}:[a-z]{1,16}") {
        prop_assert!(validate_scope(&scope).is_ok());
    }

    #[test]
    fn invalid_scopes(scope in "[A-Za-z0-9_:]{0,5}") {
        prop_assume!(!scope.contains(':') || scope.chars().any(|c| c.is_uppercase()));
        prop_assert!(validate_scope(&scope).is_err());
    }
}
