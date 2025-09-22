use std::sync::Arc;

use axum::{routing::post, Router};

use crate::xds::XdsState;

use super::handlers::create_cluster_handler;

#[derive(Clone)]
pub struct ApiState {
    pub xds_state: Arc<XdsState>,
}

pub fn build_router(state: Arc<XdsState>) -> Router {
    Router::new()
        .route("/api/v1/clusters", post(create_cluster_handler))
        .with_state(ApiState { xds_state: state })
}
