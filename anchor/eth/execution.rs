use base64::prelude::*;
use database::NetworkDatabase;
use eth::{Config, Network, SsvEventSyncer};
use openssl::rsa::Rsa;
use std::path::Path;
use std::sync::Arc;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

// This is a test binary to execute event syncing for the SSV Network
#[tokio::main]
async fn main() {
    // Setup a log filter & tracing
    let filter = EnvFilter::builder()
            .parse("info,hyper=off,hyper_util=off,alloy_transport_http=off,reqwest=off,alloy_rpc_client=off")
            .expect("filter should be valid");
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();

    // Dummy configuration with endpoint and network
    let rpc_endpoint = "http://127.0.0.1:8545";
    let _ws_endpoint = "ws://127.0.0.1:8546";
    let ws_endpoint = "wss://eth.merkle.io";
    let beacon_endpoint = "http://127.0.0.1:5052";
    let config = Config {
        http_url: String::from(rpc_endpoint),
        ws_url: String::from(ws_endpoint),
        beacon_url: String::from(beacon_endpoint),
        network: Network::Mainnet,
    };

    // Setup mock operator data
    let path = Path::new("db.sqlite");
    let pem_data = "LS0tLS1CRUdJTiBSU0EgUFVCTElDIEtFWS0tLS0tCk1JSUJJakFOQmdrcWhraUc5dzBCQVFFRkFBT0NBUThBTUlJQkNnS0NBUUVBMVg2MUFXY001QUNLaGN5MTlUaEIKby9HMWlhN1ByOVUralJ5aWY5ZjAyRG9sd091V2ZLLzdSVUlhOEhEbHBvQlVERDkwRTVQUGdJSy9sTXB4RytXbwpwQ2N5bTBpWk9UT0JzNDE5bEh3TzA4bXFja1JsZEg5WExmbmY2UThqWFR5Ym1yYzdWNmwyNVprcTl4U0owbHR1CndmTnVTSzNCZnFtNkQxOUY0aTVCbmVaSWhjRVJTYlFLWDFxbWNqYnZFL2cyQko4TzhaZUgrd0RzTHJiNnZXQVIKY3BYWG1uelE3Vlp6ZklHTGVLVU1CTTh6SW0rcXI4RGZ4SEhSeVU1QTE3cFU4cy9MNUp5RXE1RGJjc2Q2dHlnbQp5UE9BYUNzWldVREI3UGhLOHpUWU9WYi9MM1lnSTU4bjFXek5IM0s5cmFreUppTmUxTE9GVVZzQTFDUnhtQ2YzCmlRSURBUUFCCi0tLS0tRU5EIFJTQSBQVUJMSUMgS0VZLS0tLS0K";
    let pem_decoded = BASE64_STANDARD.decode(pem_data).unwrap();
    let mut pem_string = String::from_utf8(pem_decoded).unwrap();
    pem_string = pem_string
        .replace(
            "-----BEGIN RSA PUBLIC KEY-----",
            "-----BEGIN PUBLIC KEY-----",
        )
        .replace("-----END RSA PUBLIC KEY-----", "-----END PUBLIC KEY-----");
    let rsa_pubkey = Rsa::public_key_from_pem(pem_string.as_bytes())
        .map_err(|e| format!("Failed to parse RSA public key: {}", e))
        .unwrap();

    // The event syncer is spawned into a background task since it is long running and should never
    // exist. It will communicate with the rest of the system via processor channels and constantly
    // keep the database up to date with new data for the rest of the system
    let db = Arc::new(NetworkDatabase::new(path, &rsa_pubkey).unwrap());
    let mut event_syncer = SsvEventSyncer::new(db.clone(), config)
        .await
        .expect("Failed to construct event syncer");
    tokio::spawn(async move {
        // this should never return, if it does we should gracefully handle it and shutdown the
        // client.
        event_syncer.sync().await
    });
    loop {
        let _ = tokio::time::sleep(std::time::Duration::from_secs(100)).await;
    }
}
