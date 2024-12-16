use database::NetworkDatabase;
use eth::{Config, Network, SsvEventSyncer};
use openssl::pkey::Public;
use openssl::rsa::Rsa;
use std::path::Path;
use std::sync::Arc;
use tracing::{info, info_span, Level};
use tracing_subscriber::{EnvFilter, prelude::*, fmt};

#[tokio::main]
async fn main() {
    let filter = EnvFilter::builder()
        .parse("debug,hyper=off,hyper_util=off,alloy_transport_http=off,reqwest=off,alloy_rpc_client=off")
        .expect("filter should be valid");

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();
    let span = info_span!("main");
    let _guard = span.enter();

    let rpc_endpoint = "http://127.0.0.1:8545";
    let ws_endpoint = "ws://127.0.0.1:8546";

    let config = Config {
        http_url: String::from(rpc_endpoint),
        ws_url: String::from(ws_endpoint),
        network: Network::Holesky,
    };

    let priv_key = Rsa::generate(2046).expect("Failed to generate RSA key");
    let pubkey = priv_key
        .public_key_to_pem()
        .and_then(|pem| Rsa::public_key_from_pem(&pem))
        .expect("Failed to process RSA key");
    let path = Path::new("db.sqlite");

    // tie the network into the database impl!()
    let db = Arc::new(NetworkDatabase::new(path, &pubkey).expect("Failed to construct database"));
    info!("Constructed the database");

    let event_syncer = SsvEventSyncer::new(db.clone(), config)
        .await
        .expect("Failed to construct event syncer");
    let _ = event_syncer.sync().await;

    info!("hello");
}
