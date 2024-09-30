mod config;
mod router;

pub use config::Config;
use slot_clock::SlotClock;
use std::net::SocketAddr;
use std::path::PathBuf;
use task_executor::TaskExecutor;
use tokio::net::TcpListener;
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

/// Runs the HTTP API server
pub async fn run(config: Config) -> Result<(), String> {
    if !config.enabled {
        info!("HTTP API Disabled");
        return Ok(());
    }

    // Generate the axum routes
    let router = router::new();

    // Set up a listening address

    let socket = SocketAddr::new(config.listen_addr, config.listen_port);
    let listener = TcpListener::bind(socket).await.map_err(|e| e.to_string())?;

    // Start the http api server
    axum::serve(listener, router)
        .await
        .map_err(|e| format!("{}", e))
}
