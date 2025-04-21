mod config;
mod router;

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

pub use config::Config;
use database::NetworkState;
use parking_lot::RwLock;
use slot_clock::SlotClock;
use task_executor::TaskExecutor;
use tokio::{net::TcpListener, sync::watch};
use tracing::info;
/// A wrapper around all the items required to spawn the HTTP server.
///
/// The server will gracefully handle the case where any fields are `None`.
pub struct Context<T: SlotClock> {
    pub task_executor: TaskExecutor,
    // TODO: Protect the API endpoint
    // pub api_secret: ApiSecret,
    pub secrets_dir: Option<PathBuf>,
    // TODO: Handle graffiti
    // pub graffiti_file: Option<GraffitiFile>,
    // pub graffiti_flag: Option<Graffiti>,
    // TODO:Add differing chainspecs
    // pub spec: ChainSpec,
    pub config: Config,
    pub slot_clock: T,
}

pub struct Shared {
    pub database_state: Option<watch::Receiver<NetworkState>>,
}

/// Runs the HTTP API server
pub async fn run(config: Config, shared_state: Arc<RwLock<Shared>>) -> Result<(), String> {
    if !config.enabled {
        info!("HTTP API Disabled");
        return Ok(());
    }

    // Generate the axum routes
    let router = router::new(shared_state);

    // Set up a listening address

    let socket = SocketAddr::new(config.listen_addr, config.listen_port);
    let listener = TcpListener::bind(socket).await.map_err(|e| e.to_string())?;

    // Start the http api server
    axum::serve(listener, router)
        .await
        .map_err(|e| format!("{}", e))
}
