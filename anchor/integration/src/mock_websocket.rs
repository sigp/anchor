use axum::{
    extract::ws::{WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Router,
};
use std::net::SocketAddr;
use tracing::info;

// To be able to run successfully, the anchor nodes need to bind to a websocket. This is typically
// used for live sycning blocks, but this is not needed in the simulator. This is a mock
// server that acts as a dummy endpoint for the nodes to bind to.
pub struct MockServer {
    pub url: String,
    _server_handle: tokio::task::JoinHandle<()>,
}

impl MockServer {
    pub async fn start() -> Result<Self, String> {
        // Create a simple WebSocket handler function
        async fn handle_socket(ws: WebSocketUpgrade) -> impl IntoResponse {
            ws.on_upgrade(|_socket: WebSocket| async {
                // Connection established, but we don't need to do anything with it
            })
        }

        // Set up the router with our WebSocket handler
        let app = Router::new().route("/", get(handle_socket));

        // Find an available port by binding to port 0
        let socket: SocketAddr = ([127, 0, 0, 1], 0).into();
        let listener = tokio::net::TcpListener::bind(socket)
            .await
            .map_err(|e| format!("Failed to bind to socket: {}", e))?;

        // Get the actual bound address
        let addr = listener
            .local_addr()
            .map_err(|e| format!("Failed to get local address: {}", e))?;

        let url = format!("ws://localhost:{}", addr.port());
        info!("Mock server started at {}", url);

        // Spawn the server in the background with a shutdown signal that never completes
        let server = axum::serve(listener, app);
        let handle = tokio::spawn(async move {
            server.await.unwrap_or_else(|e| {
                tracing::error!("Server error: {}", e);
            });
        });

        Ok(Self {
            url,
            _server_handle: handle,
        })
    }
}
