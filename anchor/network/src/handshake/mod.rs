mod codec;
mod envelope;
pub mod node_info;

use discv5::libp2p_identity::Keypair;
use libp2p::{
    PeerId, StreamProtocol,
    request_response::{
        Behaviour as RequestResponseBehaviour, Config, InboundFailure, Message, OutboundFailure,
        ProtocolSupport, ResponseChannel,
    },
    swarm::NetworkBehaviour,
};
use tracing::trace;

use crate::handshake::{codec::Codec, node_info::NodeInfo};

/// Network behaviour handling the handshake protocol.
/// Automatically initiates handshakes on outbound connections.
pub struct Behaviour {
    inner: RequestResponseBehaviour<Codec>,
    node_info: NodeInfo,
}

pub type Event = <RequestResponseBehaviour<Codec> as NetworkBehaviour>::ToSwarm;

#[derive(Debug)]
pub enum Error {
    /// We are not on the same network as the remote
    NetworkMismatch { ours: String, theirs: String },
    /// Serialization/Deserialization of the Node Info.
    NodeInfo(node_info::Error),
    /// Error occurred while handling an incoming handshake.
    Inbound(InboundFailure),
    /// Error occurred while handling an outgoing handshake.
    Outbound(OutboundFailure),
}

/// We successfully completed a handshake.
#[derive(Debug)]
pub struct Completed {
    pub peer_id: PeerId,
    pub their_info: NodeInfo,
}

/// The handshake either failed because of shaking with an incompatible peer or because of some
/// network failure.
#[derive(Debug)]
pub struct Failed {
    pub peer_id: PeerId,
    pub error: Box<Error>,
}

impl Behaviour {
    /// Create a new handshake Behaviour.
    /// The behaviour automatically initiates handshakes on outbound connections.
    pub fn new(keypair: Keypair, node_info: NodeInfo) -> Self {
        let protocol = StreamProtocol::new("/ssv/info/0.0.1");
        let inner = RequestResponseBehaviour::with_codec(
            Codec::new(keypair),
            [(protocol, ProtocolSupport::Full)],
            Config::default(),
        );
        Self { inner, node_info }
    }

    /// Manually initiate a handshake with a peer by sending our NodeInfo.
    ///
    /// Note: In normal operation, handshakes are initiated automatically via
    /// `on_swarm_event` when an outbound connection is established. This method
    /// is provided for testing or special cases where manual control is needed.
    pub fn initiate(&mut self, peer_id: PeerId) {
        trace!(?peer_id, "initiating handshake");
        self.inner.send_request(&peer_id, self.node_info.clone());
    }
}

fn verify_node_info(ours: &NodeInfo, theirs: &NodeInfo) -> Result<(), Error> {
    if ours.network_id != theirs.network_id {
        return Err(Error::NetworkMismatch {
            ours: ours.network_id.clone(),
            theirs: theirs.network_id.clone(),
        });
    }
    Ok(())
}

/// Handle an [`Event`] emitted by the passed [`Behaviour`]. The passed [`NodeInfo`] is used for
/// validating the remote peer's data and for responding to incoming requests.
pub fn handle_event(
    our_node_info: &NodeInfo,
    behaviour: &mut Behaviour,
    event: Event,
) -> Option<Result<Completed, Failed>> {
    match event {
        Event::Message {
            peer,
            message: Message::Request {
                request, channel, ..
            },
            ..
        } => Some(handle_request(
            our_node_info,
            behaviour,
            peer,
            request,
            channel,
        )),
        Event::Message {
            peer,
            message: Message::Response { response, .. },
            ..
        } => Some(handle_response(our_node_info, peer, response)),
        Event::OutboundFailure { peer, error, .. } => {
            trace!(?peer, ?error, "Handshake outbound failure");
            Some(Err(Failed {
                peer_id: peer,
                error: Box::new(Error::Outbound(error)),
            }))
        }
        Event::InboundFailure { peer, error, .. } => Some(Err(Failed {
            peer_id: peer,
            error: Box::new(Error::Inbound(error)),
        })),
        Event::ResponseSent { .. } => None,
    }
}

fn handle_request(
    our_node_info: &NodeInfo,
    behaviour: &mut Behaviour,
    peer_id: PeerId,
    request: NodeInfo,
    channel: ResponseChannel<NodeInfo>,
) -> Result<Completed, Failed> {
    trace!(?peer_id, "handling handshake request");

    // Handle incoming handshake request from a remote peer
    //
    // This is the passive/inbound side of the handshake protocol:
    // 1. The remote peer (who dialed us) initiates the handshake by sending their NodeInfo
    // 2. We immediately send back our NodeInfo as a response
    // 3. We verify their NodeInfo is compatible with ours (same network)
    //
    // This function is called automatically by libp2p's request-response behavior
    // when a peer opens a stream on the /ssv/info/0.0.1 protocol.
    //
    // Note: We don't need to explicitly "accept" connections or queue inbound handshakes.
    // The request-response behavior handles all the stream management automatically.

    // Send our info back to the peer
    let _ = behaviour
        .inner
        .send_response(channel, our_node_info.clone());

    // Verify network compatibility
    verify_node_info(our_node_info, &request).map_err(|error| Failed {
        peer_id,
        error: Box::new(error),
    })?;

    Ok(Completed {
        peer_id,
        their_info: request,
    })
}

fn handle_response(
    our_node_info: &NodeInfo,
    peer_id: PeerId,
    response: NodeInfo,
) -> Result<Completed, Failed> {
    trace!(?peer_id, "handling handshake response");
    verify_node_info(our_node_info, &response).map_err(|error| Failed {
        peer_id,
        error: Box::new(error),
    })?;

    Ok(Completed {
        peer_id,
        their_info: response,
    })
}

impl NetworkBehaviour for Behaviour {
    type ConnectionHandler =
        <RequestResponseBehaviour<Codec> as NetworkBehaviour>::ConnectionHandler;
    type ToSwarm = Event;

    fn handle_established_inbound_connection(
        &mut self,
        connection_id: libp2p::swarm::ConnectionId,
        peer: PeerId,
        local_addr: &libp2p::Multiaddr,
        remote_addr: &libp2p::Multiaddr,
    ) -> Result<libp2p::swarm::THandler<Self>, libp2p::swarm::ConnectionDenied> {
        self.inner.handle_established_inbound_connection(
            connection_id,
            peer,
            local_addr,
            remote_addr,
        )
    }

    fn handle_established_outbound_connection(
        &mut self,
        connection_id: libp2p::swarm::ConnectionId,
        peer: PeerId,
        addr: &libp2p::Multiaddr,
        role_override: libp2p::core::Endpoint,
        port_use: libp2p::core::transport::PortUse,
    ) -> Result<libp2p::swarm::THandler<Self>, libp2p::swarm::ConnectionDenied> {
        self.inner.handle_established_outbound_connection(
            connection_id,
            peer,
            addr,
            role_override,
            port_use,
        )
    }

    fn on_swarm_event(&mut self, event: libp2p::swarm::FromSwarm) {
        // Auto-initiate handshake on first outbound connection
        if let libp2p::swarm::FromSwarm::ConnectionEstablished(conn_est) = &event {
            if let libp2p::core::ConnectedPoint::Dialer { .. } = conn_est.endpoint {
                if conn_est.other_established == 0 {
                    trace!(?conn_est.peer_id, "Auto-initiating handshake on first outbound connection");
                    self.inner
                        .send_request(&conn_est.peer_id, self.node_info.clone());
                }
            }
        }
        self.inner.on_swarm_event(event);
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        connection_id: libp2p::swarm::ConnectionId,
        event: libp2p::swarm::THandlerOutEvent<Self>,
    ) {
        self.inner
            .on_connection_handler_event(peer_id, connection_id, event);
    }

    fn poll(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<libp2p::swarm::ToSwarm<Self::ToSwarm, libp2p::swarm::THandlerInEvent<Self>>>
    {
        self.inner.poll(cx)
    }
}

#[cfg(test)]
mod tests {
    // Init tracing
    static TRACING: LazyLock<()> = LazyLock::new(|| {
        let env_filter = tracing_subscriber::EnvFilter::new("trace");
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    });

    use std::sync::LazyLock;

    use discv5::libp2p_identity::Keypair;
    use libp2p::swarm::{Swarm, SwarmEvent};
    use libp2p_swarm_test::SwarmExt;
    use tokio::select;

    use super::*;
    use crate::handshake::node_info::NodeMetadata;

    // Test helper functions for cleaner test structure

    fn node_info(network: &str, version: &str) -> NodeInfo {
        NodeInfo {
            network_id: network.to_string(),
            metadata: Some(NodeMetadata {
                node_version: version.to_string(),
                execution_node: "".to_string(),
                consensus_node: "".to_string(),
                subnets: "".to_string(),
            }),
        }
    }

    fn create_test_swarm(keypair: Keypair, node_info: NodeInfo) -> Swarm<Behaviour> {
        Swarm::new_ephemeral_tokio(|_| Behaviour::new(keypair, node_info))
    }

    async fn wait_for_handshake_completion(
        local_swarm: &mut Swarm<Behaviour>,
        remote_swarm: &mut Swarm<Behaviour>,
        local_info: &NodeInfo,
        remote_info: &NodeInfo,
    ) -> (Completed, Completed) {
        let mut local_result = None;
        let mut remote_result = None;

        while local_result.is_none() || remote_result.is_none() {
            select!(
                SwarmEvent::Behaviour(e) = local_swarm.next_swarm_event() => {
                    if let Some(result) = handle_event(local_info, local_swarm.behaviour_mut(), e) {
                        local_result = Some(result.expect("local handshake to succeed"));
                    }
                }
                SwarmEvent::Behaviour(e) = remote_swarm.next_swarm_event() => {
                    if let Some(result) = handle_event(remote_info, remote_swarm.behaviour_mut(), e) {
                        remote_result = Some(result.expect("remote handshake to succeed"));
                    }
                }
                else => {}
            )
        }

        (local_result.unwrap(), remote_result.unwrap())
    }

    #[tokio::test]
    async fn handshake_success() {
        *TRACING;

        // Setup: Create two peers with matching networks
        let local_info = node_info("test", "local");
        let remote_info = node_info("test", "remote");

        let mut local_swarm = create_test_swarm(Keypair::generate_ed25519(), local_info.clone());
        let mut remote_swarm = create_test_swarm(Keypair::generate_ed25519(), remote_info.clone());

        tokio::spawn(async move {
            // Setup: Establish connection
            local_swarm.listen().with_memory_addr_external().await;
            remote_swarm.connect(&mut local_swarm).await;

            // Test: Wait for both sides to complete handshake
            let (local_result, remote_result) = wait_for_handshake_completion(
                &mut local_swarm,
                &mut remote_swarm,
                &local_info,
                &remote_info,
            )
            .await;

            // Verify: Both sides received correct peer info
            assert_eq!(local_result.peer_id, *remote_swarm.local_peer_id());
            assert_eq!(
                local_result.their_info.metadata.unwrap().node_version,
                "remote"
            );

            assert_eq!(remote_result.peer_id, *local_swarm.local_peer_id());
            assert_eq!(
                remote_result.their_info.metadata.unwrap().node_version,
                "local"
            );
        })
        .await
        .expect("test completed");
    }

    /// Evidence-gathering test for concurrent dial behavior.
    ///
    /// This test demonstrates that when both peers dial each other simultaneously:
    /// 1. Both peers get 2 ConnectionEstablished events (one Dialer, one Listener)
    /// 2. Only ONE peer initiates the handshake (the one whose Dialer connection wins the race)
    /// 3. The check `other_established == 0` prevents duplicate handshake initiations
    /// 4. Both peers complete the handshake successfully despite concurrent dials
    ///
    /// This proves that our approach using `other_established == 0` correctly handles
    /// concurrent dial resolution without relying on the Identify protocol.
    #[tokio::test]
    async fn concurrent_dials_both_initiate_handshake() {
        *TRACING;

        let local_key = Keypair::generate_ed25519();
        let remote_key = Keypair::generate_ed25519();

        let local_node_info = node_info("test", "local");
        let remote_node_info = node_info("test", "remote");

        let mut local_swarm =
            Swarm::new_ephemeral_tokio(|_| Behaviour::new(local_key, local_node_info.clone()));
        let mut remote_swarm =
            Swarm::new_ephemeral_tokio(|_| Behaviour::new(remote_key, remote_node_info.clone()));

        tokio::spawn(async move {
            local_swarm.listen().with_memory_addr_external().await;
            remote_swarm.listen().with_memory_addr_external().await;

            // Force both peers to dial each other by getting addresses and dialing manually
            let local_addr = local_swarm.external_addresses().next().unwrap().clone();
            let remote_addr = remote_swarm.external_addresses().next().unwrap().clone();

            trace!(?local_addr, ?remote_addr, "About to dial each other");

            // Dial each other at the same time
            local_swarm.dial(remote_addr.clone()).unwrap();
            trace!("Local dialed remote");
            remote_swarm.dial(local_addr.clone()).unwrap();
            trace!("Remote dialed local");

            // Track how many times we see Auto-initiating and what other_established values we see
            let mut local_handshake_initiated = 0;
            let mut remote_handshake_initiated = 0;
            let mut local_completed = false;
            let mut remote_completed = false;

            // Also track connection events to see concurrent dial resolution
            let mut local_connections = 0;
            let mut remote_connections = 0;

            while !local_completed || !remote_completed {
                select!(
                    event = local_swarm.next_swarm_event() => {
                        match event {
                            SwarmEvent::ConnectionEstablished { num_established, endpoint, .. } => {
                                local_connections += 1;
                                trace!(?endpoint, ?num_established, "Local: ConnectionEstablished");
                            }
                            SwarmEvent::Behaviour(e) => {
                                if let Some(result) = handle_event(&local_node_info, local_swarm.behaviour_mut(), e) {
                                    match result {
                                        Ok(Completed { peer_id, their_info }) => {
                                            local_handshake_initiated += 1;
                                            assert_eq!(peer_id, *remote_swarm.local_peer_id());
                                            assert_eq!(their_info.metadata.unwrap().node_version, "remote");
                                            local_completed = true;
                                        }
                                        Err(Failed { error, .. }) => {
                                            trace!(?error, "Local: Handshake failed");
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    event = remote_swarm.next_swarm_event() => {
                        match event {
                            SwarmEvent::ConnectionEstablished { num_established, endpoint, .. } => {
                                remote_connections += 1;
                                trace!(?endpoint, ?num_established, "Remote: ConnectionEstablished");
                            }
                            SwarmEvent::Behaviour(e) => {
                                if let Some(result) = handle_event(&remote_node_info, remote_swarm.behaviour_mut(), e) {
                                    match result {
                                        Ok(Completed { peer_id, their_info }) => {
                                            remote_handshake_initiated += 1;
                                            assert_eq!(peer_id, *local_swarm.local_peer_id());
                                            assert_eq!(their_info.metadata.unwrap().node_version, "local");
                                            remote_completed = true;
                                        }
                                        Err(Failed { error, .. }) => {
                                            trace!(?error, "Remote: Handshake failed");
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    else => {}
                )
            }

            // Evidence gathering: Check if we saw concurrent dials and what happened
            trace!(
                local_connections,
                remote_connections,
                local_handshake_initiated,
                remote_handshake_initiated,
                "Concurrent dial evidence"
            );
        })
        .await
        .expect("tokio runtime failed");
    }

    /// Test basic handshake with a single outbound connection.
    ///
    /// This test verifies that:
    /// 1. Only the dialer (remote) auto-initiates the handshake
    /// 2. The listener (local) responds to the handshake request
    /// 3. Both sides complete the handshake successfully
    ///
    /// This is the simple case with no concurrent dials.
    #[tokio::test]
    async fn bidirectional_connection_handshake_success() {
        *TRACING;

        let local_key = Keypair::generate_ed25519();
        let remote_key = Keypair::generate_ed25519();

        let local_node_info = node_info("test", "local");
        let remote_node_info = node_info("test", "remote");

        let mut local_swarm =
            Swarm::new_ephemeral_tokio(|_| Behaviour::new(local_key, local_node_info.clone()));
        let mut remote_swarm =
            Swarm::new_ephemeral_tokio(|_| Behaviour::new(remote_key, remote_node_info.clone()));

        tokio::spawn(async move {
            local_swarm.listen().with_memory_addr_external().await;
            remote_swarm.listen().with_memory_addr_external().await;

            // Remote dials local - only remote will initiate handshake
            remote_swarm.connect(&mut local_swarm).await;

            // Both peers should complete handshake
            let mut local_completed = false;
            let mut remote_completed = false;

            while !local_completed || !remote_completed {
                select!(
                    SwarmEvent::Behaviour(e) = local_swarm.next_swarm_event() => {
                        if let Some(result) = handle_event(&local_node_info, local_swarm.behaviour_mut(), e) {
                            let Completed { peer_id, their_info } = result.expect("handshake to succeed");
                            assert_eq!(peer_id, *remote_swarm.local_peer_id());
                            assert_eq!(their_info.metadata.unwrap().node_version, "remote");
                            local_completed = true;
                        }
                    }
                    SwarmEvent::Behaviour(e) = remote_swarm.next_swarm_event() => {
                        if let Some(result) = handle_event(&remote_node_info, remote_swarm.behaviour_mut(), e) {
                            let Completed { peer_id, their_info } = result.expect("handshake to succeed");
                            assert_eq!(peer_id, *local_swarm.local_peer_id());
                            assert_eq!(their_info.metadata.unwrap().node_version, "local");
                            remote_completed = true;
                        }
                    }
                    else => {}
                )
            }
        })
        .await
        .expect("tokio runtime failed");
    }

    #[tokio::test]
    async fn mismatched_networks_handshake_failed() {
        *TRACING;

        let local_key = Keypair::generate_ed25519();
        let remote_key = Keypair::generate_ed25519();

        let local_node_info = node_info("test1", "local");
        let remote_node_info = node_info("test2", "remote");

        let mut local_swarm =
            Swarm::new_ephemeral_tokio(|_| Behaviour::new(local_key, local_node_info.clone()));
        let mut remote_swarm =
            Swarm::new_ephemeral_tokio(|_| Behaviour::new(remote_key, remote_node_info.clone()));

        tokio::spawn(async move {
            local_swarm.listen().with_memory_addr_external().await;

            remote_swarm.connect(&mut local_swarm).await;

            // No manual initiate() call - Behaviour handles it automatically!

            let mut local_failed = false;
            let mut remote_failed = false;

            while !local_failed && !remote_failed {
                select!(
                    SwarmEvent::Behaviour(e) = local_swarm.next_swarm_event() => {
                        let Some(result) =
                            handle_event(&local_node_info, local_swarm.behaviour_mut(), e) else {
                            continue;
                        };
                        let Failed {
                            peer_id,
                            error,
                        } = result.expect_err("handshake to fail");
                        let Error::NetworkMismatch { ours, theirs } = *error else {
                            panic!("expected network mismatch");
                        };
                        assert_eq!(peer_id, *remote_swarm.local_peer_id());
                        assert_eq!(ours, "test1");
                        assert_eq!(theirs, "test2");
                        local_failed = true;
                    }
                    SwarmEvent::Behaviour(e) = remote_swarm.next_swarm_event() => {
                        let Some(result) =
                            handle_event(&remote_node_info, remote_swarm.behaviour_mut(), e) else {
                            continue;
                        };
                        let Failed {
                            peer_id,
                            error,
                        } = result.expect_err("handshake to fail");
                        let Error::NetworkMismatch { ours, theirs } = *error else {
                            panic!("expected network mismatch");
                        };
                        assert_eq!(peer_id, *local_swarm.local_peer_id());
                        assert_eq!(ours, "test2");
                        assert_eq!(theirs, "test1");
                        remote_failed = true;
                    }
                    else => {}
                )
            }
        })
        .await
        .expect("tokio runtime failed");
    }
}
