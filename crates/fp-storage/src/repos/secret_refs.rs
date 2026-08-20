//! Normalized listener/cluster secret-reference maintenance.
//!
//! The JSON spec remains authoritative configuration. These rows provide race-safe, same-team
//! dependency enforcement for secret lifecycle operations and are replaced in the same owner
//! transaction as the spec.

use fp_domain::secret::{SecretReference, SecretType};
use fp_domain::{DomainError, DomainResult, TeamId};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
pub struct ResolvedSecretReference {
    pub secret_id: Uuid,
    pub usage: &'static str,
}

pub async fn resolve(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    references: &[SecretReference<'_>],
) -> DomainResult<Vec<ResolvedSecretReference>> {
    let mut resolved = Vec::with_capacity(references.len());
    for reference in references {
        let row = sqlx::query(
            "SELECT id, secret_type FROM secrets \
             WHERE team_id = $1 AND name = $2 FOR KEY SHARE",
        )
        .bind(team_id.as_uuid())
        .bind(reference.name)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("resolve secret reference: {e}")))?
        .ok_or_else(|| DomainError::not_found("secret", reference.name))?;
        let actual = row.get::<String, _>("secret_type").parse::<SecretType>()?;
        if actual != reference.required_type {
            return Err(DomainError::validation(format!(
                "secret \"{}\" has type {}, but {} requires {}",
                reference.name,
                actual.as_str(),
                reference.usage,
                reference.required_type.as_str()
            ))
            .with_hint("create a correctly typed secret and update the resource reference"));
        }
        resolved.push(ResolvedSecretReference {
            secret_id: row.get("id"),
            usage: reference.usage,
        });
    }
    Ok(resolved)
}

pub async fn replace_listener(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    listener_id: Uuid,
    references: &[ResolvedSecretReference],
) -> DomainResult<()> {
    sqlx::query("DELETE FROM listener_secret_refs WHERE listener_id = $1")
        .bind(listener_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("clear listener secret refs: {e}")))?;
    for reference in references {
        sqlx::query(
            "INSERT INTO listener_secret_refs (listener_id, team_id, secret_id, usage) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(listener_id)
        .bind(team_id.as_uuid())
        .bind(reference.secret_id)
        .bind(reference.usage)
        .execute(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("insert listener secret ref: {e}")))?;
    }
    Ok(())
}

pub async fn replace_cluster(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    cluster_id: Uuid,
    references: &[ResolvedSecretReference],
) -> DomainResult<()> {
    sqlx::query("DELETE FROM cluster_secret_refs WHERE cluster_id = $1")
        .bind(cluster_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("clear cluster secret refs: {e}")))?;
    for reference in references {
        sqlx::query(
            "INSERT INTO cluster_secret_refs (cluster_id, team_id, secret_id, usage) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(cluster_id)
        .bind(team_id.as_uuid())
        .bind(reference.secret_id)
        .bind(reference.usage)
        .execute(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("insert cluster secret ref: {e}")))?;
    }
    Ok(())
}
