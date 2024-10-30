//! Provides a metrics server for Anchor
//!
//! This may be a temporary addition, once the Lighthouse VC moves to axum we may be able to group
//! code. a

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::time::{SystemTime, UNIX_EPOCH};

/// Contains objects which have shared access from inside/outside of the metrics server.
pub struct Shared {}

/// Configuration for the HTTP server.
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub enabled: bool,
    pub listen_addr: IpAddr,
    pub listen_port: u16,
    pub allow_origin: Option<String>,
    pub allocator_metrics_enabled: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
            listen_port: 5164,
            allow_origin: None,
            allocator_metrics_enabled: true,
        }
    }
}

/// A wrapper around all the items required to spawn the HTTP server.
///
/// The server will gracefully handle the case where any fields are `None`.
pub struct Context {
    pub config: Config,
    pub shared: RwLock<Shared>,
    pub log: Logger,
}

/// Creates a server that will serve requests using information from `ctx`.
///
/// The server will shut down gracefully when the `shutdown` future resolves.
///
/// ## Returns
///
/// This function will bind the server to the provided address and then return a tuple of:
///
/// - `SocketAddr`: the address that the HTTP server will listen on.
/// - `Future`: the actual server future that will need to be awaited.
///
/// ## Errors
///
/// Returns an error if the server is unable to bind or there is another error during
/// configuration.

pub fn serve<E: EthSpec>(
    ctx: Arc<Context<E>>,
    shutdown: impl Future<Output = ()> + Send + Sync + 'static,
) -> Result<(SocketAddr, impl Future<Output = ()>), Error> {
    let config = &ctx.config;
    let log = ctx.log.clone();

    // Configure CORS.
    let cors_builder = {
        let builder = warp::cors()
            .allow_method("GET")
            .allow_headers(vec!["Content-Type"]);

        warp_utils::cors::set_builder_origins(
            builder,
            config.allow_origin.as_deref(),
            (config.listen_addr, config.listen_port),
        )?
    };

    // Sanity check.
    if !config.enabled {
        crit!(log, "Cannot start disabled metrics HTTP server");
        return Err(Error::Other(
            "A disabled metrics server should not be started".to_string(),
        ));
    }

    let inner_ctx = ctx.clone();
    let routes = warp::get()
        .and(warp::path("metrics"))
        .map(move || inner_ctx.clone())
        .and_then(|ctx: Arc<Context<E>>| async move {
            Ok::<_, warp::Rejection>(
                gather_prometheus_metrics(&ctx)
                    .map(|body| {
                        Response::builder()
                            .status(200)
                            .header("Content-Type", "text/plain")
                            .body(body)
                            .unwrap()
                    })
                    .unwrap_or_else(|e| {
                        Response::builder()
                            .status(500)
                            .header("Content-Type", "text/plain")
                            .body(format!("Unable to gather metrics: {:?}", e))
                            .unwrap()
                    }),
            )
        })
        // Add a `Server` header.
        .map(|reply| warp::reply::with_header(reply, "Server", &version_with_platform()))
        .with(cors_builder.build());

    let (listening_socket, server) = warp::serve(routes).try_bind_with_graceful_shutdown(
        SocketAddr::new(config.listen_addr, config.listen_port),
        async {
            shutdown.await;
        },
    )?;

    info!(
        log,
        "Metrics HTTP server started";
        "listen_address" => listening_socket.to_string(),
    );

    Ok((listening_socket, server))
}

pub fn gather_prometheus_metrics<E: EthSpec>(
    ctx: &Context<E>,
) -> std::result::Result<String, String> {
    use validator_metrics::*;

    let mut buffer = vec![];
    let encoder = TextEncoder::new();

    {
        let shared = ctx.shared.read();

        if let Some(genesis_time) = shared.genesis_time {
            if let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) {
                let distance = now.as_secs() as i64 - genesis_time as i64;
                set_gauge(&GENESIS_DISTANCE, distance);
            }
        }

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
    }

    // It's important to ensure these metrics are explicitly enabled in the case that users aren't
    // using glibc and this function causes panics.
    if ctx.config.allocator_metrics_enabled {
        scrape_allocator_metrics();
    }

    warp_utils::metrics::scrape_health_metrics();

    encoder
        .encode(&lighthouse_metrics::gather(), &mut buffer)
        .unwrap();

    String::from_utf8(buffer).map_err(|e| format!("Failed to encode prometheus info: {:?}", e))
}
