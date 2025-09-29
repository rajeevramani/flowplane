use serde_json::json;

use crate::errors::Result;
use crate::storage::{AuditEvent, AuditLogRepository};

pub async fn record_create_event(
    audit_repo: &AuditLogRepository,
    api_id: &str,
    team: &str,
    domain: &str,
) -> Result<()> {
    audit_repo
        .record_platform_event(AuditEvent {
            resource_id: Some(api_id.to_string()),
            resource_name: Some(format!("{team}:{domain}")),
            action: "platform.api.created".to_string(),
            metadata: json!({
                "team": team,
                "domain": domain,
            }),
        })
        .await
}

pub async fn record_route_appended_event(
    audit_repo: &AuditLogRepository,
    api_id: &str,
    route_id: &str,
    match_type: &str,
    match_value: &str,
) -> Result<()> {
    audit_repo
        .record_platform_event(AuditEvent {
            resource_id: Some(api_id.to_string()),
            resource_name: Some(route_id.to_string()),
            action: "platform.api.route_appended".to_string(),
            metadata: json!({
                "match_type": match_type,
                "match_value": match_value,
            }),
        })
        .await
}
