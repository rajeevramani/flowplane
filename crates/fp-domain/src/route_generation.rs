//! Persisted S9 route generation dry-run/apply plans.

use crate::gateway::cluster::ClusterSpec;
use crate::gateway::listener::ListenerSpec;
use crate::gateway::route_config::RouteConfigSpec;
use crate::id::{ApiDefinitionId, RouteGenerationPlanId, SpecVersionId, TeamId};
use crate::{DomainError, DomainResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteGenerationPlan {
    pub id: RouteGenerationPlanId,
    pub team_id: TeamId,
    pub spec_version_id: SpecVersionId,
    pub status: RouteGenerationPlanStatus,
    pub plan: RouteGenerationPlanSpec,
    pub applied_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteGenerationPlanStatus {
    DryRun,
    Applied,
}

impl RouteGenerationPlanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DryRun => "dry_run",
            Self::Applied => "applied",
        }
    }

    pub fn parse(raw: &str) -> DomainResult<Self> {
        match raw {
            "dry_run" => Ok(Self::DryRun),
            "applied" => Ok(Self::Applied),
            other => Err(DomainError::internal(format!(
                "unknown route generation plan status \"{other}\" in database"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteGenerationPlanSpec {
    pub api_definition_id: ApiDefinitionId,
    pub api_name: String,
    pub cluster_name: String,
    pub route_config_name: String,
    pub listener_name: String,
    pub listener_port: u16,
    pub cluster_spec: ClusterSpec,
    pub route_config_spec: RouteConfigSpec,
    pub listener_spec: ListenerSpec,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}
