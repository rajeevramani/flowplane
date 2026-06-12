//! Audit writer (spec/08a §6): every mutation, every denial, every auth failure.
//!
//! Mutations record audit inside their own transaction (the fp-core services own that from
//! S3). Denials and auth failures use [`record_best_effort`]: the request outcome must not
//! depend on the audit insert, but a failed insert is itself loudly logged and counted —
//! silent audit loss is how incidents become unexplainable.

use fp_domain::{AuditEntryId, OrgId, RequestId, TeamId, UserId};
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorType {
    User,
    Agent,
    Dataplane,
    System,
    Anonymous,
}

impl ActorType {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
            Self::Dataplane => "dataplane",
            Self::System => "system",
            Self::Anonymous => "anonymous",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Rest,
    Mcp,
    Cli,
    Xds,
    System,
}

impl Surface {
    fn as_str(self) -> &'static str {
        match self {
            Self::Rest => "rest",
            Self::Mcp => "mcp",
            Self::Cli => "cli",
            Self::Xds => "xds",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Denied,
    Failure,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Denied => "denied",
            Self::Failure => "failure",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub request_id: Option<RequestId>,
    pub actor_type: ActorType,
    pub actor_id: Option<uuid::Uuid>,
    pub actor_label: String,
    pub surface: Surface,
    /// Verb-style action, e.g. `team.create`, `authz.denied`, `authn.failed`.
    pub action: String,
    /// Resource reference, e.g. `clusters/payments-db`.
    pub resource: String,
    pub org_id: Option<OrgId>,
    pub team_id: Option<TeamId>,
    pub outcome: Outcome,
    /// Structured context. MUST NOT contain secrets, tokens, or captured payloads.
    pub detail: serde_json::Value,
}

impl AuditEntry {
    pub fn denial(
        request_id: RequestId,
        user_id: Option<UserId>,
        surface: Surface,
        resource: String,
        reason: &str,
    ) -> Self {
        Self {
            request_id: Some(request_id),
            actor_type: if user_id.is_some() {
                ActorType::User
            } else {
                ActorType::Anonymous
            },
            actor_id: user_id.map(|u| u.as_uuid()),
            actor_label: String::new(),
            surface,
            action: "authz.denied".into(),
            resource,
            org_id: None,
            team_id: None,
            outcome: Outcome::Denied,
            detail: serde_json::json!({ "reason": reason }),
        }
    }
}

const INSERT: &str = "INSERT INTO audit_log \
    (id, request_id, actor_type, actor_id, actor_label, surface, action, resource, org_id, team_id, outcome, detail) \
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)";

fn bind_entry<'q>(
    query: sqlx::query::Query<'q, Postgres, sqlx::postgres::PgArguments>,
    id: AuditEntryId,
    entry: &'q AuditEntry,
) -> sqlx::query::Query<'q, Postgres, sqlx::postgres::PgArguments> {
    query
        .bind(id.as_uuid())
        .bind(entry.request_id.map(|r| r.as_uuid()))
        .bind(entry.actor_type.as_str())
        .bind(entry.actor_id)
        .bind(&entry.actor_label)
        .bind(entry.surface.as_str())
        .bind(&entry.action)
        .bind(&entry.resource)
        .bind(entry.org_id.map(|o| o.as_uuid()))
        .bind(entry.team_id.map(|t| t.as_uuid()))
        .bind(entry.outcome.as_str())
        .bind(&entry.detail)
}

/// Record within a transaction — used by mutating services so the audit row commits or
/// rolls back WITH the mutation it describes.
pub async fn record_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    entry: &AuditEntry,
) -> fp_domain::DomainResult<()> {
    bind_entry(sqlx::query(INSERT), AuditEntryId::generate(), entry)
        .execute(&mut **tx)
        .await
        .map(|_| ())
        .map_err(|e| fp_domain::DomainError::internal(format!("audit insert: {e}")))
}

/// Best-effort record for denials/auth failures: never fails the caller, never silent.
pub async fn record_best_effort(pool: &PgPool, entry: &AuditEntry) {
    if let Err(e) = bind_entry(sqlx::query(INSERT), AuditEntryId::generate(), entry)
        .execute(pool)
        .await
    {
        metrics::counter!("fp_audit_write_failures_total").increment(1);
        tracing::error!(action = %entry.action, "audit write failed: {e}");
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn denial_entries_persist_with_reason() {
        let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
            eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
            return;
        };
        let pool = crate::connect(&url, 2).await.expect("connect");
        crate::migrate(&pool).await.expect("migrate");

        let rid = RequestId::generate();
        let entry = AuditEntry::denial(
            rid,
            None,
            Surface::Rest,
            "clusters/secret-cluster".into(),
            "cross_org",
        );
        record_best_effort(&pool, &entry).await;

        let (outcome, detail): (String, serde_json::Value) =
            sqlx::query_as("SELECT outcome, detail FROM audit_log WHERE request_id = $1")
                .bind(rid.as_uuid())
                .fetch_one(&pool)
                .await
                .expect("audit row exists");
        assert_eq!(outcome, "denied");
        assert_eq!(detail["reason"], "cross_org");
    }
}
