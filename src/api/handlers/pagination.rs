//! Shared pagination types for list endpoints.
//!
//! Provides `PaginationQuery` for standardized request parameters and
//! `PaginatedResponse<T>` for consistent list response format.

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// Default limit for paginated list queries.
pub fn default_limit() -> i64 {
    50
}

/// Shared pagination query parameters for list endpoints.
///
/// Use directly for handlers that only need limit/offset.
/// For handlers with additional filter fields, embed these fields
/// in a handler-specific struct.
#[derive(Debug, Clone, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct PaginationQuery {
    /// Maximum number of items to return (default: 50)
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// Number of items to skip (default: 0)
    #[serde(default)]
    pub offset: i64,
}

impl PaginationQuery {
    /// Clamp pagination parameters to safe bounds.
    ///
    /// Limits the `limit` to the range [1, max_limit] and ensures `offset` >= 0.
    pub fn clamp(&self, max_limit: i64) -> (i64, i64) {
        (self.limit.clamp(1, max_limit), self.offset.max(0))
    }
}

/// Standardized paginated response wrapper for list endpoints.
///
/// All list endpoints should return this format for client predictability.
/// The `items` field name is consistent across all resources.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PaginatedResponse<T> {
    /// The list of items for the current page
    pub items: Vec<T>,
    /// Total number of items matching the query (across all pages)
    pub total: i64,
    /// Applied limit
    pub limit: i64,
    /// Applied offset
    pub offset: i64,
}

impl<T> PaginatedResponse<T> {
    /// Create a new paginated response.
    pub fn new(items: Vec<T>, total: i64, limit: i64, offset: i64) -> Self {
        Self { items, total, limit, offset }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_limit() {
        assert_eq!(default_limit(), 50);
    }

    #[test]
    fn test_pagination_query_clamp() {
        let query = PaginationQuery { limit: 200, offset: -5 };
        let (limit, offset) = query.clamp(100);
        assert_eq!(limit, 100);
        assert_eq!(offset, 0);
    }

    #[test]
    fn test_pagination_query_clamp_minimum() {
        let query = PaginationQuery { limit: 0, offset: 10 };
        let (limit, offset) = query.clamp(100);
        assert_eq!(limit, 1);
        assert_eq!(offset, 10);
    }

    #[derive(Debug, Clone, Serialize, ToSchema)]
    struct TestItem {
        name: String,
    }

    #[test]
    fn test_paginated_response_new() {
        let items = vec![TestItem { name: "a".to_string() }, TestItem { name: "b".to_string() }];
        let resp = PaginatedResponse::new(items, 10, 50, 0);
        assert_eq!(resp.items.len(), 2);
        assert_eq!(resp.total, 10);
        assert_eq!(resp.limit, 50);
        assert_eq!(resp.offset, 0);
    }

    #[test]
    fn test_paginated_response_serialization() {
        let resp = PaginatedResponse::new(vec![TestItem { name: "test".to_string() }], 1, 50, 0);
        let json = serde_json::to_value(&resp).expect("serialize");
        assert_eq!(json["items"][0]["name"], "test");
        assert_eq!(json["total"], 1);
        assert_eq!(json["limit"], 50);
        assert_eq!(json["offset"], 0);
    }
}
