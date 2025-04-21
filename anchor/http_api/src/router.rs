//! The routes for the HTTP API

use std::sync::Arc;

use api_types::{GenericResponse, ValidatorData, VersionData};
use axum::{extract::State, routing::get, Json, Router};
use parking_lot::RwLock;
use version::version_with_platform;

use crate::Shared;
/// Creates all the routes for HTTP API
pub fn new(shared_state: Arc<RwLock<Shared>>) -> Router {
    // Default route
    Router::new()
        .route("/", get(root))
        .route("/anchor/version", get(get_version))
        .route("/anchor/validators", get(get_validators))
        .with_state(shared_state)
}

// Temporary return value.
async fn root() -> &'static str {
    "Anchor client"
}

async fn get_version() -> Json<GenericResponse<VersionData>> {
    Json(GenericResponse::from(VersionData {
        version: version_with_platform(),
    }))
}

async fn get_validators(
    State(shared_state): State<Arc<RwLock<Shared>>>,
) -> Json<GenericResponse<Vec<ValidatorData>>> {
    if let Some(database_state) = &shared_state.read().database_state {
        let validators = database_state
            .borrow()
            .metadata()
            .values()
            .map(|v| ValidatorData {
                public_key: v.public_key.to_string(),
                cluster_id: format!("{:?}", v.cluster_id),
                index: v.index.map(|i| i.0),
                graffiti: hex::encode(v.graffiti.0),
            })
            .collect::<Vec<_>>();

        Json(GenericResponse::from(validators))
    } else {
        Json(GenericResponse::from(Vec::new()))
    }
}
