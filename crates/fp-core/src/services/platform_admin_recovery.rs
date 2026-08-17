//! Offline platform-admin recovery policy and redacted plan contracts.

use fp_domain::{DomainError, DomainResult, EntityStatus, OrgId, OrgRole, RequestId, UserId};
use fp_storage::repos::audit::{ActorType, AuditEntry, Outcome, Surface};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use uuid::Uuid;

const PLAN_VERSION: &str = "v1";
const PLAN_DOMAIN: &[u8] = b"flowplane:platform-admin-recovery-plan:v1\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PlanDigest(String);

impl PlanDigest {
    pub fn parse(raw: &str) -> DomainResult<Self> {
        let Some(hex) = raw.strip_prefix("sha256:") else {
            return Err(DomainError::validation(
                "expected plan digest must use the sha256:<64 lowercase hex> format",
            ));
        };
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(DomainError::validation(
                "expected plan digest must use the sha256:<64 lowercase hex> format",
            ));
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TenantTransfer {
    pub org_id: OrgId,
    pub org_name: String,
    pub membership_id: Uuid,
    pub role: OrgRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecoveryPlan {
    pub version: &'static str,
    pub digest: PlanDigest,
    pub platform_org_id: OrgId,
    pub source_user_id: UserId,
    pub source_user_status: EntityStatus,
    pub replacement_user_id: UserId,
    pub replacement_user_status: EntityStatus,
    pub platform_membership_id: Uuid,
    pub platform_role: OrgRole,
    pub tenant_transfers: Vec<TenantTransfer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecoveryApplyResult {
    pub request_id: RequestId,
    pub applied_plan: PlanDigest,
    pub platform_org_id: OrgId,
    pub source_user_id: UserId,
    pub replacement_user_id: UserId,
    pub transferred_memberships: usize,
    pub tenant_org_ids: Vec<OrgId>,
    pub rollback_available: bool,
}

#[derive(Serialize)]
struct CanonicalPlan<'a> {
    platform_membership_id: Uuid,
    platform_org_id: OrgId,
    platform_role: OrgRole,
    replacement_user_id: UserId,
    replacement_user_status: EntityStatus,
    source_user_id: UserId,
    source_user_status: EntityStatus,
    tenant_transfers: &'a [CanonicalTenantTransfer<'a>],
}

#[derive(Serialize)]
struct CanonicalTenantTransfer<'a> {
    membership_id: Uuid,
    org_id: OrgId,
    org_name: &'a str,
    role: OrgRole,
}

pub async fn plan(
    pool: &PgPool,
    replacement_subject: &str,
    transfer_owned_orgs: &[String],
) -> DomainResult<RecoveryPlan> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| DomainError::internal(format!("recovery plan: begin: {e}")))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(|e| DomainError::internal(format!("recovery plan: isolation: {e}")))?;
    if !fp_storage::repos::bootstrap::try_lock_platform_identity_in_tx(&mut tx).await? {
        return Err(DomainError::conflict(
            "another platform identity operation is in progress; retry recovery planning",
        ));
    }

    let result = build_plan_in_tx(&mut tx, replacement_subject, transfer_owned_orgs, false).await;
    tx.rollback()
        .await
        .map_err(|e| DomainError::internal(format!("recovery plan: rollback: {e}")))?;
    result
}

pub async fn apply(
    pool: &PgPool,
    replacement_subject: &str,
    transfer_owned_orgs: &[String],
    expected_plan: &PlanDigest,
) -> DomainResult<RecoveryApplyResult> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| DomainError::internal(format!("recovery apply: begin: {e}")))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *tx)
        .await
        .map_err(|e| DomainError::internal(format!("recovery apply: isolation: {e}")))?;
    if !fp_storage::repos::bootstrap::try_lock_platform_identity_in_tx(&mut tx).await? {
        return Err(DomainError::conflict(
            "another platform identity operation is in progress; retry recovery apply",
        ));
    }

    let plan = build_plan_in_tx(&mut tx, replacement_subject, transfer_owned_orgs, true).await?;
    if plan.digest != *expected_plan {
        return Err(DomainError::conflict(
            "the recovery plan is stale; run plan again and review the new digest",
        ));
    }

    fp_storage::repos::identity::transfer_recovery_membership_in_tx(
        &mut tx,
        plan.platform_membership_id,
        plan.platform_org_id,
        plan.source_user_id,
        plan.replacement_user_id,
    )
    .await?;
    for tenant in &plan.tenant_transfers {
        fp_storage::repos::identity::transfer_recovery_membership_in_tx(
            &mut tx,
            tenant.membership_id,
            tenant.org_id,
            plan.source_user_id,
            plan.replacement_user_id,
        )
        .await?;
    }

    let request_id = RequestId::generate();
    let rollback_available = plan.source_user_status == EntityStatus::Active;
    let tenant_org_ids = plan
        .tenant_transfers
        .iter()
        .map(|tenant| tenant.org_id)
        .collect::<Vec<_>>();
    let transferred_memberships = 1 + tenant_org_ids.len();
    fp_storage::repos::audit::record_in_tx(
        &mut tx,
        &AuditEntry {
            request_id: Some(request_id),
            actor_type: ActorType::System,
            actor_id: None,
            actor_label: "offline-platform-admin-recovery".to_string(),
            surface: Surface::Cli,
            action: "platform_admin.recover".to_string(),
            resource: format!("organizations/{}/platform-admin", plan.platform_org_id),
            org_id: Some(plan.platform_org_id),
            team_id: None,
            outcome: Outcome::Success,
            detail: serde_json::json!({
                "source_user_id": plan.source_user_id,
                "replacement_user_id": plan.replacement_user_id,
                "platform_org_id": plan.platform_org_id,
                "tenant_org_ids": tenant_org_ids.clone(),
                "membership_count": transferred_memberships,
                "request_id": request_id,
                "expected_plan": expected_plan.as_str(),
                "applied_plan": plan.digest.as_str(),
                "rollback_available": rollback_available,
            }),
        },
    )
    .await?;
    tx.commit()
        .await
        .map_err(|e| DomainError::internal(format!("recovery apply: commit: {e}")))?;

    Ok(RecoveryApplyResult {
        request_id,
        applied_plan: plan.digest,
        platform_org_id: plan.platform_org_id,
        source_user_id: plan.source_user_id,
        replacement_user_id: plan.replacement_user_id,
        transferred_memberships,
        tenant_org_ids,
        rollback_available,
    })
}

async fn build_plan_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    replacement_subject: &str,
    transfer_owned_orgs: &[String],
    for_update: bool,
) -> DomainResult<RecoveryPlan> {
    let platform_org_id = if for_update {
        fp_storage::repos::identity::recovery_platform_org_id_for_update(tx).await?
    } else {
        fp_storage::repos::identity::recovery_platform_org_id(tx).await?
    }
    .ok_or_else(|| DomainError::conflict("the database is not initialized"))?;
    let replacement = if for_update {
        fp_storage::repos::identity::recovery_user_by_subject_for_update(tx, replacement_subject)
            .await?
    } else {
        fp_storage::repos::identity::recovery_user_by_subject(tx, replacement_subject).await?
    }
    .ok_or_else(|| DomainError::conflict("the replacement identity is not provisioned"))?;
    if replacement.status != EntityStatus::Active {
        return Err(DomainError::conflict(
            "the replacement identity is not active",
        ));
    }

    let owners = if for_update {
        fp_storage::repos::identity::recovery_owner_memberships_for_update(tx, platform_org_id)
            .await?
    } else {
        fp_storage::repos::identity::recovery_owner_memberships(tx, platform_org_id).await?
    };
    if owners.len() != 1 {
        return Err(DomainError::conflict(format!(
            "platform ownership is not recoverable: expected exactly one current owner, found {}",
            owners.len()
        )));
    }
    let source = &owners[0];
    let replacement_platform_membership = if for_update {
        fp_storage::repos::identity::recovery_membership_for_update(
            tx,
            platform_org_id,
            replacement.id,
        )
        .await?
    } else {
        fp_storage::repos::identity::recovery_membership(tx, platform_org_id, replacement.id)
            .await?
    };
    if replacement_platform_membership.is_some() {
        return Err(DomainError::conflict(
            "the replacement identity already has a platform membership",
        ));
    }

    let mut seen_orgs = BTreeSet::new();
    let mut tenant_transfers = Vec::with_capacity(transfer_owned_orgs.len());
    for org_ref in transfer_owned_orgs {
        let org = if for_update {
            fp_storage::repos::identity::recovery_org_by_ref_for_update(tx, org_ref).await?
        } else {
            fp_storage::repos::identity::recovery_org_by_ref(tx, org_ref).await?
        }
        .ok_or_else(|| DomainError::conflict("a requested tenant organization was not found"))?;
        if org.id == platform_org_id {
            return Err(DomainError::conflict(
                "the platform organization cannot be selected as a tenant transfer",
            ));
        }
        if !seen_orgs.insert(org.id) {
            return Err(DomainError::conflict(
                "a tenant organization was requested more than once",
            ));
        }
        let source_membership = if for_update {
            fp_storage::repos::identity::recovery_membership_for_update(tx, org.id, source.user_id)
                .await?
        } else {
            fp_storage::repos::identity::recovery_membership(tx, org.id, source.user_id).await?
        }
        .ok_or_else(|| {
            DomainError::conflict(
                "the source platform owner is not a member of a requested tenant organization",
            )
        })?;
        if source_membership.role != OrgRole::Owner {
            return Err(DomainError::conflict(
                "the source platform owner does not own a requested tenant organization",
            ));
        }
        let source_has_grants = if for_update {
            fp_storage::repos::identity::recovery_user_grants_exist_for_update(
                tx,
                org.id,
                source.user_id,
            )
            .await?
        } else {
            fp_storage::repos::identity::recovery_user_grants_exist(tx, org.id, source.user_id)
                .await?
        };
        if source_has_grants {
            return Err(DomainError::conflict(
                "a requested tenant organization has source user grants; recovery never transfers or deletes grants",
            ));
        }
        let replacement_tenant_membership = if for_update {
            fp_storage::repos::identity::recovery_membership_for_update(tx, org.id, replacement.id)
                .await?
        } else {
            fp_storage::repos::identity::recovery_membership(tx, org.id, replacement.id).await?
        };
        if replacement_tenant_membership.is_some() {
            return Err(DomainError::conflict(
                "the replacement identity already has a requested tenant membership",
            ));
        }
        tenant_transfers.push(TenantTransfer {
            org_id: org.id,
            org_name: org.name,
            membership_id: source_membership.id,
            role: source_membership.role,
        });
    }

    RecoveryPlan::new(
        platform_org_id,
        source.user_id,
        source.user_status,
        replacement.id,
        replacement.status,
        source.id,
        source.role,
        tenant_transfers,
    )
}

impl RecoveryPlan {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        platform_org_id: OrgId,
        source_user_id: UserId,
        source_user_status: EntityStatus,
        replacement_user_id: UserId,
        replacement_user_status: EntityStatus,
        platform_membership_id: Uuid,
        platform_role: OrgRole,
        mut tenant_transfers: Vec<TenantTransfer>,
    ) -> DomainResult<Self> {
        if platform_role != OrgRole::Owner {
            return Err(DomainError::conflict(
                "the current platform membership is not an owner membership",
            ));
        }
        if replacement_user_status != EntityStatus::Active {
            return Err(DomainError::conflict(
                "the replacement identity is not active",
            ));
        }
        if tenant_transfers
            .iter()
            .any(|item| item.role != OrgRole::Owner)
        {
            return Err(DomainError::conflict(
                "only tenant-owner memberships can be transferred",
            ));
        }
        tenant_transfers.sort_by_key(|item| (item.org_id, item.membership_id));

        let mut plan = Self {
            version: PLAN_VERSION,
            digest: PlanDigest(String::new()),
            platform_org_id,
            source_user_id,
            source_user_status,
            replacement_user_id,
            replacement_user_status,
            platform_membership_id,
            platform_role,
            tenant_transfers,
        };
        let canonical_json = plan.canonical_json_bytes()?;
        let mut hasher = Sha256::new();
        hasher.update(PLAN_DOMAIN);
        hasher.update(canonical_json);
        let mut digest = String::with_capacity(71);
        digest.push_str("sha256:");
        for byte in hasher.finalize() {
            write!(&mut digest, "{byte:02x}")
                .map_err(|_| DomainError::internal("cannot format recovery-plan digest"))?;
        }
        plan.digest = PlanDigest(digest);
        Ok(plan)
    }

    pub fn canonical_json(&self) -> DomainResult<String> {
        String::from_utf8(self.canonical_json_bytes()?).map_err(|error| {
            DomainError::internal(format!("cannot encode recovery plan as UTF-8: {error}"))
        })
    }

    fn canonical_json_bytes(&self) -> DomainResult<Vec<u8>> {
        let canonical_tenants = self
            .tenant_transfers
            .iter()
            .map(|item| CanonicalTenantTransfer {
                membership_id: item.membership_id,
                org_id: item.org_id,
                org_name: &item.org_name,
                role: item.role,
            })
            .collect::<Vec<_>>();
        let canonical = CanonicalPlan {
            platform_membership_id: self.platform_membership_id,
            platform_org_id: self.platform_org_id,
            platform_role: self.platform_role,
            replacement_user_id: self.replacement_user_id,
            replacement_user_status: self.replacement_user_status,
            source_user_id: self.source_user_id,
            source_user_status: self.source_user_status,
            tenant_transfers: &canonical_tenants,
        };
        serde_json::to_vec(&canonical)
            .map_err(|error| DomainError::internal(format!("cannot encode recovery plan: {error}")))
    }
}
