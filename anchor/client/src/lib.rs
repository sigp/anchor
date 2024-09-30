// use tracing::{debug, info};

pub mod config;

use config::Config;
use task_executor::TaskExecutor;

pub struct Client {}

impl Client {
    /// Instantiates the Anchor client
    pub async fn new(_executor: TaskExecutor, _config: Config) -> Result<Self, String> {
        /*
                // Attempt to raise soft fd limit. The behavior is OS specific:
                // `linux` - raise soft fd limit to hard
                // `macos` - raise soft fd limit to `min(kernel limit, hard fd limit)`
                // `windows` & rest - noop
                match fdlimit::raise_fd_limit().map_err(|e| format!("Unable to raise fd limit: {}", e))? {
                    fdlimit::Outcome::LimitRaised { from, to } => {
                        debug!(
                            "old_limit" = from,
                            "new_limit" = to
                            "Raised soft open file descriptor resource limit"
                        );
                    }
                    fdlimit::Outcome::Unsupported => {
                        debug!("Raising soft open file descriptor resource limit is not supported");
                    }
                };

                info!(
                    "beacon_nodes" = format!("{:?}", &config.beacon_nodes),
                    "validator_dir" = format!("{:?}", config.validator_dir),
                    "Starting validator client"
                );

                // Optionally start the metrics server.
                let http_metrics_ctx = if config.http_metrics.enabled {
                    let shared = http_metrics::Shared {
                        validator_store: None,
                        genesis_time: None,
                        duties_service: None,
                    };

                    let ctx: Arc<http_metrics::Context<E>> = Arc::new(http_metrics::Context {
                        config: config.http_metrics.clone(),
                        shared: RwLock::new(shared),
                        log: log.clone(),
                    });

                    let exit = context.executor.exit();

                    let (_listen_addr, server) = http_metrics::serve(ctx.clone(), exit)
                        .map_err(|e| format!("Unable to start metrics API server: {:?}", e))?;

                    context
                        .clone()
                        .executor
                        .spawn_without_exit(server, "metrics-api");

                    Some(ctx)
                } else {
                    info!(log, "HTTP metrics server is disabled");
                    None
                };
        */
        Ok(Client {})
    }

    /// Executes the main client logic
    pub async fn run(&mut self) -> Result<(), String> {
        Ok(())
    }
}
