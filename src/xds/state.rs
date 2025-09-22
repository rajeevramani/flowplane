use std::sync::Arc;

use crate::{
    config::SimpleXdsConfig,
    storage::{ClusterRepository, DbPool},
};

/// Shared xDS server state, providing configuration and optional database access
#[derive(Debug)]
pub struct XdsState {
    pub config: SimpleXdsConfig,
    pub version: Arc<std::sync::atomic::AtomicU64>,
    pub cluster_repository: Option<ClusterRepository>,
}

impl XdsState {
    pub fn new(config: SimpleXdsConfig) -> Self {
        Self {
            config,
            version: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            cluster_repository: None,
        }
    }

    pub fn with_database(config: SimpleXdsConfig, pool: DbPool) -> Self {
        Self {
            config,
            version: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            cluster_repository: Some(ClusterRepository::new(pool)),
        }
    }

    pub fn get_version(&self) -> String {
        self.version
            .load(std::sync::atomic::Ordering::Relaxed)
            .to_string()
    }

    pub fn increment_version(&self) {
        self.version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}
