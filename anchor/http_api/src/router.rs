//! The routes for the HTTP API

use axum::{routing::get, Router};

/// Creates all the routes for HTTP API
pub fn new() -> Router {
    // Default route
    Router::new().route("/", get(root))
}

// Temporary return value.
async fn root() -> &'static str {
    "Anchor client"
}
