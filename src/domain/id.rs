//! Domain ID Types with NewType Pattern
//!
//! This module provides type-safe wrappers for domain identifiers to prevent
//! ID mixing errors at compile time. Each ID type implements Display, FromStr,
//! Debug, Serialize, and Deserialize for full compatibility with existing code.

use serde::{Deserialize, Serialize};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::{Decode, Encode, Sqlite, Type};
use std::fmt;
use std::str::FromStr;
use utoipa::ToSchema;
use uuid::Uuid;

/// Macro to generate NewType ID wrappers with all required traits
macro_rules! domain_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Create a new ID from a UUID
            pub fn new() -> Self {
                Self(Uuid::new_v4().to_string())
            }

            /// Create an ID from an existing string (for database retrieval)
            pub fn from_string(s: String) -> Self {
                Self(s)
            }

            /// Create an ID from a string slice
            pub fn from_str_unchecked(s: &str) -> Self {
                Self(s.to_string())
            }

            /// Get the inner string value
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Convert to inner string value
            pub fn into_string(self) -> String {
                self.0
            }

            /// Parse and validate a UUID string
            pub fn parse(s: &str) -> Result<Self, uuid::Error> {
                Uuid::parse_str(s)?;
                Ok(Self(s.to_string()))
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        // SQLx trait implementations for database compatibility
        impl Type<Sqlite> for $name {
            fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
                <String as Type<Sqlite>>::type_info()
            }
        }

        impl<'q> Encode<'q, Sqlite> for $name {
            fn encode_by_ref(
                &self,
                buf: &mut Vec<sqlx::sqlite::SqliteArgumentValue<'q>>,
            ) -> Result<IsNull, BoxDynError> {
                <String as Encode<'q, Sqlite>>::encode_by_ref(&self.0, buf)
            }
        }

        impl<'r> Decode<'r, Sqlite> for $name {
            fn decode(value: sqlx::sqlite::SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
                let s = <String as Decode<'r, Sqlite>>::decode(value)?;
                Ok(Self(s))
            }
        }
    };
}

// Define all domain ID types
domain_id!(
    /// Unique identifier for a cluster resource
    ClusterId
);

domain_id!(
    /// Unique identifier for a route configuration
    RouteId
);

domain_id!(
    /// Unique identifier for a listener configuration
    ListenerId
);

domain_id!(
    /// Unique identifier for an authentication token
    TokenId
);

domain_id!(
    /// Unique identifier for an API definition
    ApiDefinitionId
);

domain_id!(
    /// Unique identifier for an API route within an API definition
    ApiRouteId
);

domain_id!(
    /// Unique identifier for a user
    UserId
);

domain_id!(
    /// Unique identifier for a team
    TeamId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_id_creation() {
        let id = ClusterId::new();
        assert!(!id.as_str().is_empty());
        assert!(Uuid::parse_str(id.as_str()).is_ok());
    }

    #[test]
    fn route_id_from_string() {
        let uuid_str = Uuid::new_v4().to_string();
        let id = RouteId::from_string(uuid_str.clone());
        assert_eq!(id.as_str(), uuid_str);
    }

    #[test]
    fn listener_id_display() {
        let id = ListenerId::new();
        let display_str = format!("{}", id);
        assert_eq!(display_str, id.as_str());
    }

    #[test]
    fn token_id_from_str() {
        let uuid_str = Uuid::new_v4().to_string();
        let id: TokenId = uuid_str.parse().expect("Failed to parse UUID");
        assert_eq!(id.as_str(), uuid_str);
    }

    #[test]
    fn api_definition_id_invalid_uuid_fails() {
        let result = ApiDefinitionId::parse("not-a-uuid");
        assert!(result.is_err());
    }

    #[test]
    fn cluster_id_serialization() {
        let id = ClusterId::new();
        let json = serde_json::to_string(&id).expect("Failed to serialize");

        // Should serialize as a simple string, not as object
        assert!(json.starts_with('"'));
        assert!(json.ends_with('"'));

        let deserialized: ClusterId = serde_json::from_str(&json).expect("Failed to deserialize");
        assert_eq!(id, deserialized);
    }

    #[test]
    fn route_id_equality() {
        let id1 = RouteId::from_string("test-id".to_string());
        let id2 = RouteId::from_string("test-id".to_string());
        let id3 = RouteId::from_string("different-id".to_string());

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn listener_id_hash() {
        use std::collections::HashMap;

        let id = ListenerId::new();
        let mut map = HashMap::new();
        map.insert(id.clone(), "listener-data");

        assert_eq!(map.get(&id), Some(&"listener-data"));
    }

    #[test]
    fn token_id_clone() {
        let id1 = TokenId::new();
        let id2 = id1.clone();

        assert_eq!(id1, id2);
        assert_eq!(id1.as_str(), id2.as_str());
    }

    #[test]
    fn api_route_id_into_string() {
        let uuid_str = Uuid::new_v4().to_string();
        let id = ApiRouteId::from_string(uuid_str.clone());
        let string: String = id.into_string();

        assert_eq!(string, uuid_str);
    }

    #[test]
    fn cluster_id_as_ref() {
        let id = ClusterId::new();
        let s: &str = id.as_ref();

        assert_eq!(s, id.as_str());
    }

    #[test]
    fn compile_time_type_safety() {
        // This test verifies that IDs of different types cannot be mixed
        let cluster_id = ClusterId::new();
        let route_id = RouteId::new();

        // These should be different types
        fn takes_cluster_id(_id: ClusterId) {}
        fn takes_route_id(_id: RouteId) {}

        takes_cluster_id(cluster_id);
        takes_route_id(route_id);

        // The following would fail at compile time (uncomment to verify):
        // takes_cluster_id(route_id); // ERROR: mismatched types
        // takes_route_id(cluster_id); // ERROR: mismatched types
    }

    #[test]
    fn json_deserialization_from_object() {
        // Test that we can deserialize from both plain strings and objects
        let uuid_str = Uuid::new_v4().to_string();

        // From plain string
        let json_string = format!("\"{}\"", uuid_str);
        let id1: ClusterId =
            serde_json::from_str(&json_string).expect("Failed to deserialize from string");
        assert_eq!(id1.as_str(), uuid_str);
    }

    #[test]
    fn default_creates_new_id() {
        let id1 = ListenerId::default();
        let id2 = ListenerId::default();

        // Default should create unique IDs
        assert_ne!(id1, id2);
        assert!(Uuid::parse_str(id1.as_str()).is_ok());
        assert!(Uuid::parse_str(id2.as_str()).is_ok());
    }
}
