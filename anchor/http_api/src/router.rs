//! The routes for the HTTP API

use std::sync::Arc;

use api_types::{GenericResponse, VersionData, ComitteeData};
use axum::{extract::State, routing::get, Json, Router};
use ssv_types::CommitteeId;
use parking_lot::RwLock;
use version::version_with_platform;
use crate::Shared;

/// Creates all the routes for HTTP API
pub fn new(shared_state: Arc<RwLock<Shared>>) -> Router {
    // Default route
    Router::new()
        .route("/", get(root))
        .route("/anchor/version", get(get_version))       
        .route("/anchor/committees", get(get_committees))
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

async fn get_committees(
    State(shared_state): State<Arc<RwLock<Shared>>>,
) -> Json<GenericResponse<Vec<ComitteeData>>> {
    if let Some(database_state) = &shared_state.read().database_state {
        let state = database_state.borrow();
        let committee_ids = state
            .clusters()
            .values()
            .map(|cluster| cluster.committee_id())
            .collect::<Vec<CommitteeId>>();
        
            let committee_data = committee_ids
            .iter()
            .filter_map(|committee_id| {
                state.get_committee_info_by_committee_id(committee_id).map(|info| {
                    ComitteeData {
                        comittee_id: format!("{:?}", committee_id),
                        committee_members: info.committee_members.iter().map(|operator_id| operator_id.0).collect(),
                        validator_indices: info.validator_indices.iter().map(|validator_index| validator_index.0).collect(),
                    }
                })
            })
            .collect::<Vec<ComitteeData>>();
        Json(GenericResponse::from(committee_data))
    } else {
        Json(GenericResponse::from(Vec::new()))
    }
}