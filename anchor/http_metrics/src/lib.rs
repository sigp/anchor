//! Provides a metrics server for Anchor
//!
//! This may be a temporary addition, once the Lighthouse VC moves to axum we may be able to group
//! code.

use axum::{
    body::Body,
    extract::State,
    http::Method,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tracing::error;

/// Contains objects which have shared access from inside/outside of the metrics server.
pub struct Shared {
    /// If we know genesis, it is entered here.
    pub genesis_time: Option<u64>,
}

/// Configuration for the HTTP server.
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub enabled: bool,
    pub listen_addr: IpAddr,
    pub listen_port: u16,
    pub allow_origin: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
            listen_port: 5164,
            allow_origin: None,
        }
    }
}

fn create_router(shared_state: Arc<RwLock<Shared>>) -> Router {
    let cors = CorsLayer::new()
        // allow `GET` and `POST` when accessing the resource
        .allow_methods([Method::GET, Method::POST])
        // allow requests from any origin
        .allow_origin(Any);

    Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(shared_state)
        .layer(cors)
}

/// Gets the prometheus metrics
async fn metrics_handler(State(state): State<Arc<RwLock<Shared>>>) -> Response<Body> {
    // Use common lighthouse validator metrics
    use validator_metrics::*;

    let mut buffer = vec![];
    let encoder = TextEncoder::new();

    {
        let shared = state.read();

        if let Some(genesis_time) = shared.genesis_time {
            if let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) {
                let distance = now.as_secs() as i64 - genesis_time as i64;
                set_gauge(&GENESIS_DISTANCE, distance);
            }
        }

        // Duties services
        /*
        if let Some(duties_service) = &shared.duties_service {
            if let Some(slot) = duties_service.slot_clock.now() {
                let current_epoch = slot.epoch(E::slots_per_epoch());
                let next_epoch = current_epoch + 1;

                set_int_gauge(
                    &PROPOSER_COUNT,
                    &[CURRENT_EPOCH],
                    duties_service.proposer_count(current_epoch) as i64,
                );
                set_int_gauge(
                    &ATTESTER_COUNT,
                    &[CURRENT_EPOCH],
                    duties_service.attester_count(current_epoch) as i64,
                );
                set_int_gauge(
                    &ATTESTER_COUNT,
                    &[NEXT_EPOCH],
                    duties_service.attester_count(next_epoch) as i64,
                );
            }
        }
        */
    }

    health_metrics::metrics::scrape_health_metrics();

    encoder.encode(&metrics::gather(), &mut buffer).unwrap();

    match String::from_utf8(buffer) {
        Ok(v) => v.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to encode promethus data: {}", e),
        )
            .into_response(),
    }
}

/// Creates a server that will serve requests using information from `ctx`.
///
/// The server will shut down gracefully when the `shutdown` future resolves.
pub async fn serve(
    listener: TcpListener,
    shared_state: Arc<RwLock<Shared>>,
    shutdown: impl Future<Output = ()> + Send + Sync + 'static,
) {
    // Generate the axum routes
    let router = create_router(shared_state);

    // Start the http api server
    if let Err(e) = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
    {
        error!(?e, "HTTP Metrics server failed");
    }
}
