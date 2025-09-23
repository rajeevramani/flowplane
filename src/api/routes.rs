use std::sync::Arc;

use axum::{routing::get, Router};

use crate::xds::XdsState;

use super::{
    docs,
    handlers::{
        create_cluster_handler, delete_cluster_handler, get_cluster_handler, list_clusters_handler,
        update_cluster_handler,
    },
};

#[derive(Clone)]
pub struct ApiState {
    pub xds_state: Arc<XdsState>,
}

pub fn build_router(state: Arc<XdsState>) -> Router {
    let api_state = ApiState {
        xds_state: state.clone(),
    };

    let api = Router::new()
        .route(
            "/api/v1/clusters",
            get(list_clusters_handler).post(create_cluster_handler),
        )
        .route(
            "/api/v1/clusters/{name}",
            get(get_cluster_handler)
                .put(update_cluster_handler)
                .delete(delete_cluster_handler),
        )
        .with_state(api_state);

    api.merge(docs::docs_router())
}
