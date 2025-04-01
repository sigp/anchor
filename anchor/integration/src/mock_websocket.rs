use std::net::SocketAddr;

use tracing::info;
use warp::Filter;

// To be able to run successfully, the anchor nodes need to bind to a websocket. This is typically
// used for live sycning blocks, but this is not needed in the simulator. This is a mock
// server that acts as a dummy endpoint for the nodes to bind to.
pub struct MockServer {
    pub url: String,
    _server_handle: tokio::task::JoinHandle<()>,
}

impl MockServer {
    pub async fn start() -> Result<Self, String> {
        // Create a simple WebSocket route that just accepts connections
        let ws_route = warp::ws().map(|ws: warp::ws::Ws| {
            ws.on_upgrade(|_websocket| async {
                // Connection established, but we don't need to do anything with it
            })
        });

        // Find an available port
        let socket: SocketAddr = ([127, 0, 0, 1], 0).into();

        // Bind to the socket
        let (addr, server) = warp::serve(ws_route).bind_with_graceful_shutdown(socket, async {
            // This future is never completed, so shutdown only happens when handle is dropped
            std::future::pending::<()>().await;
        });

        let url = format!("ws://localhost:{}", addr.port());
        info!("Mock server started at {}", url);

        // Spawn the server in the background
        let handle = tokio::spawn(server);

        Ok(Self {
            url,
            _server_handle: handle,
        })
    }
}
