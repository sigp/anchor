//! The routes for the HTTP API

use axum::{routing::get, Router, Json};
use version::{version_with_platform};
use api_types::{GenericResponse, VersionData};
/// Creates all the routes for HTTP API
pub fn new() -> Router {
    // Default route
    Router::new()
        .route("/", get(root))
        .route("/anchor/version", get(get_version))
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
