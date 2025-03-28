//! The routes for the HTTP API

use api_types::{GenericResponse, VersionData};
use axum::{routing::get, Json, Router};
use version::version_with_platform;
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
