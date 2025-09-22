use std::sync::Arc;

use crate::{
    config::SimpleXdsConfig,
    storage::{ClusterRepository, DbPool},
};
use tokio::sync::broadcast;

/// Shared xDS server state, providing configuration and optional database access
#[derive(Debug)]
pub struct XdsState {
    pub config: SimpleXdsConfig,
    pub version: Arc<std::sync::atomic::AtomicU64>,
    pub cluster_repository: Option<ClusterRepository>,
    update_tx: broadcast::Sender<u64>,
}

impl XdsState {
    pub fn new(config: SimpleXdsConfig) -> Self {
        let (update_tx, _) = broadcast::channel(128);
        Self {
            config,
            version: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            cluster_repository: None,
            update_tx,
        }
    }

    pub fn with_database(config: SimpleXdsConfig, pool: DbPool) -> Self {
        let (update_tx, _) = broadcast::channel(128);
        Self {
            config,
            version: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            cluster_repository: Some(ClusterRepository::new(pool)),
            update_tx,
        }
    }

    pub fn get_version(&self) -> String {
        self.version
            .load(std::sync::atomic::Ordering::Relaxed)
            .to_string()
    }

    pub fn get_version_number(&self) -> u64 {
        self.version
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn increment_version(&self) {
        let new_version = self
            .version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        let _ = self.update_tx.send(new_version);
    }

    pub fn subscribe_updates(&self) -> broadcast::Receiver<u64> {
        self.update_tx.subscribe()
    }
}
