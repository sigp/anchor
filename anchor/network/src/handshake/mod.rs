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

    /// Handle an event emitted by this behaviour.
    /// Returns `Some` with the handshake result (success or failure) when the handshake completes,
    /// or `None` for events that don't complete a handshake (like ResponseSent).
    pub fn handle_event(&mut self, event: Event) -> Option<Result<Completed, Failed>> {
        match event {
            Event::Message {
                peer,
                message:
                    Message::Request {
                        request, channel, ..
                    },
                ..
            } => Some(self.handle_request(peer, request, channel)),
            Event::Message {
                peer,
                message: Message::Response { response, .. },
                ..
            } => Some(Self::handle_response(&self.node_info, peer, response)),
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
        &mut self,
        peer_id: PeerId,
        request: NodeInfo,
        channel: ResponseChannel<NodeInfo>,
    ) -> Result<Completed, Failed> {
        trace!(?peer_id, "handling handshake request");

        // Send our info back to the peer
        let _ = self.inner.send_response(channel, self.node_info.clone());

        // Verify network compatibility
        verify_node_info(&self.node_info, &request).map_err(|error| Failed {
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

    /// Determines if a handshake should be initiated for this connection.
    ///
    /// Returns `Some(peer_id)` if:
    /// - The event is a ConnectionEstablished event
    /// - The connection is outbound (we are the dialer)
    /// - This is the first established connection to the peer (other_established == 0)
    fn should_initiate_handshake<'a>(
        event: &'a libp2p::swarm::FromSwarm<'a>,
    ) -> Option<&'a PeerId> {
        if let libp2p::swarm::FromSwarm::ConnectionEstablished(conn_est) = event
            && let libp2p::core::ConnectedPoint::Dialer { .. } = conn_est.endpoint
            && conn_est.other_established == 0
        {
            Some(&conn_est.peer_id)
        } else {
            None
        }
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
        if let Some(peer_id) = Self::should_initiate_handshake(&event) {
            trace!(
                ?peer_id,
                "Auto-initiating handshake on first outbound connection"
            );
            self.inner.send_request(peer_id, self.node_info.clone());
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

    /// Helper to wait for both swarms to complete handshake
    async fn wait_for_handshake_completion(
        local_swarm: &mut Swarm<Behaviour>,
        remote_swarm: &mut Swarm<Behaviour>,
    ) -> (Completed, Completed) {
        let mut local_result = None;
        let mut remote_result = None;

        while local_result.is_none() || remote_result.is_none() {
            select!(
                SwarmEvent::Behaviour(e) = local_swarm.next_swarm_event() => {
                    if let Some(result) = local_swarm.behaviour_mut().handle_event(e) {
                        local_result = Some(result.expect("local handshake to succeed"));
                    }
                }
                SwarmEvent::Behaviour(e) = remote_swarm.next_swarm_event() => {
                    if let Some(result) = remote_swarm.behaviour_mut().handle_event(e) {
                        remote_result = Some(result.expect("remote handshake to succeed"));
                    }
                }
                else => {}
            )
        }

        (local_result.unwrap(), remote_result.unwrap())
    }

    /// Expected peer information for test assertions
    struct ExpectedPeer<'a> {
        peer_id: PeerId,
        version: &'a str,
    }

    /// Test state tracking for handshake tests
    struct TestState {
        connections: usize,
        handshakes: usize,
        completed: bool,
    }

    /// Helper to handle swarm events and update tracking state
    fn handle_swarm_event_for_test(
        swarm: &mut Swarm<Behaviour>,
        event: SwarmEvent<Event>,
        expected: &ExpectedPeer,
        state: &mut TestState,
    ) {
        match event {
            SwarmEvent::ConnectionEstablished {
                num_established,
                endpoint,
                ..
            } => {
                state.connections += 1;
                trace!(?endpoint, ?num_established, "ConnectionEstablished");
            }
            SwarmEvent::Behaviour(e) => {
                if let Some(result) = swarm.behaviour_mut().handle_event(e) {
                    match result {
                        Ok(Completed {
                            peer_id,
                            their_info,
                        }) => {
                            state.handshakes += 1;
                            assert_eq!(peer_id, expected.peer_id);
                            assert_eq!(their_info.metadata.unwrap().node_version, expected.version);
                            state.completed = true;
                        }
                        Err(Failed { error, .. }) => {
                            trace!(?error, "Handshake failed");
                        }
                    }
                }
            }
            _ => {}
        }
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
            let (local_result, remote_result) =
                wait_for_handshake_completion(&mut local_swarm, &mut remote_swarm).await;

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

            let expected_remote = ExpectedPeer {
                peer_id: *remote_swarm.local_peer_id(),
                version: "remote",
            };
            let expected_local = ExpectedPeer {
                peer_id: *local_swarm.local_peer_id(),
                version: "local",
            };

            let mut local_state = TestState {
                connections: 0,
                handshakes: 0,
                completed: false,
            };
            let mut remote_state = TestState {
                connections: 0,
                handshakes: 0,
                completed: false,
            };

            while !local_state.completed || !remote_state.completed {
                select!(
                    event = local_swarm.next_swarm_event() => {
                        handle_swarm_event_for_test(
                            &mut local_swarm,
                            event,
                            &expected_remote,
                            &mut local_state,
                        );
                    }
                    event = remote_swarm.next_swarm_event() => {
                        handle_swarm_event_for_test(
                            &mut remote_swarm,
                            event,
                            &expected_local,
                            &mut remote_state,
                        );
                    }
                    else => {}
                )
            }

            // Evidence gathering: Check if we saw concurrent dials and what happened
            trace!(
                local_connections = local_state.connections,
                remote_connections = remote_state.connections,
                local_handshake_initiated = local_state.handshakes,
                remote_handshake_initiated = remote_state.handshakes,
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
                        if let Some(result) = local_swarm.behaviour_mut().handle_event(e) {
                            let Completed { peer_id, their_info } = result.expect("handshake to succeed");
                            assert_eq!(peer_id, *remote_swarm.local_peer_id());
                            assert_eq!(their_info.metadata.unwrap().node_version, "remote");
                            local_completed = true;
                        }
                    }
                    SwarmEvent::Behaviour(e) = remote_swarm.next_swarm_event() => {
                        if let Some(result) = remote_swarm.behaviour_mut().handle_event(e) {
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
                            local_swarm.behaviour_mut().handle_event(e) else {
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
                            remote_swarm.behaviour_mut().handle_event(e) else {
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
