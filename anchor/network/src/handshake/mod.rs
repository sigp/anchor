mod codec;
mod envelope;
pub mod node_info;

use std::{
    collections::VecDeque,
    task::{Context, Poll},
};

use discv5::libp2p_identity::Keypair;
use libp2p::{
    PeerId, StreamProtocol,
    request_response::{
        Behaviour as RequestResponseBehaviour, Config, Event as RequestResponseEvent,
        InboundFailure, Message, OutboundFailure, ProtocolSupport, ResponseChannel,
    },
    swarm::{NetworkBehaviour, THandlerInEvent, ToSwarm},
};
use tracing::{debug, trace};

use crate::handshake::{codec::Codec, node_info::NodeInfo};

/// Event emitted on handshake completion or failure.
#[derive(Debug)]
pub enum Event {
    Completed {
        peer_id: PeerId,
        their_info: NodeInfo,
    },
    Failed {
        peer_id: PeerId,
        error: Box<Error>,
    },
}

/// Network behaviour handling the handshake protocol.
/// Automatically initiates handshakes on outbound connections.
pub struct Behaviour {
    inner: RequestResponseBehaviour<Codec>,
    node_info: NodeInfo,
    events: VecDeque<Event>,
}

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
        Self {
            inner,
            node_info,
            events: VecDeque::new(),
        }
    }

    fn verify_and_emit_event(&mut self, peer_id: PeerId, their_info: NodeInfo) {
        match verify_node_info(&self.node_info, &their_info) {
            Ok(()) => {
                // Log handshake completion and record metrics
                if let Some(metadata) = &their_info.metadata {
                    if let Some(our_metadata) = self.node_metadata() {
                        let matching_count =
                            count_matching_subnets(&our_metadata.subnets, &metadata.subnets);
                        debug!(
                            %peer_id,
                            our_subnets = %our_metadata.subnets,
                            their_subnets = %metadata.subnets,
                            node_version = %metadata.node_version,
                            matching_subnets = matching_count,
                            "Handshake completed"
                        );

                        // Record subnet match count metric
                        if let Ok(gauge_vec) = crate::metrics::HANDSHAKE_SUBNET_MATCHES.as_ref() {
                            let label = &matching_count.to_string();
                            if let Ok(gauge) = gauge_vec.get_metric_with_label_values(&[label]) {
                                gauge.inc();
                            }
                        }
                    }
                } else {
                    debug!(%peer_id, "Handshake completed without metadata");
                }
                self.events.push_back(Event::Completed {
                    peer_id,
                    their_info,
                });
            }
            Err(error) => {
                self.events.push_back(Event::Failed {
                    peer_id,
                    error: Box::new(error),
                });
            }
        }
    }

    fn handle_request(
        &mut self,
        peer_id: PeerId,
        request: NodeInfo,
        channel: ResponseChannel<NodeInfo>,
    ) {
        trace!(?peer_id, "handling handshake request");

        // Send our info back to the peer
        if self
            .inner
            .send_response(channel, self.node_info.clone())
            .is_err()
        {
            trace!(
                ?peer_id,
                "Failed to send handshake response (channel closed)"
            );
        }

        // Verify network compatibility and emit event
        self.verify_and_emit_event(peer_id, request);
    }

    fn handle_response(&mut self, peer_id: PeerId, response: NodeInfo) {
        trace!(?peer_id, "handling handshake response");

        // Verify network compatibility and emit event
        self.verify_and_emit_event(peer_id, response);
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

    pub fn node_metadata(&self) -> &Option<node_info::NodeMetadata> {
        &self.node_info.metadata
    }

    pub fn node_metadata_mut(&mut self) -> &mut Option<node_info::NodeMetadata> {
        &mut self.node_info.metadata
    }
}

/// Count the number of matching subnet bits between two hex-encoded subnet strings
fn count_matching_subnets(our_subnets: &str, their_subnets: &str) -> usize {
    // Decode both subnet strings
    let our_bytes = match hex::decode(our_subnets) {
        Ok(bytes) => bytes,
        Err(_) => return 0,
    };
    let their_bytes = match hex::decode(their_subnets) {
        Ok(bytes) => bytes,
        Err(_) => return 0,
    };

    // Count matching bits using bitwise AND
    our_bytes
        .iter()
        .zip(their_bytes.iter())
        .map(|(a, b)| (a & b).count_ones() as usize)
        .sum()
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
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        // Process events from inner request-response behaviour
        while let Poll::Ready(event) = self.inner.poll(cx) {
            match event {
                ToSwarm::GenerateEvent(req_resp_event) => match req_resp_event {
                    RequestResponseEvent::Message {
                        peer,
                        message:
                            Message::Request {
                                request, channel, ..
                            },
                        ..
                    } => {
                        trace!("Received handshake request");
                        self.handle_request(peer, request, channel);
                    }
                    RequestResponseEvent::Message {
                        peer,
                        message: Message::Response { response, .. },
                        ..
                    } => {
                        trace!(?response, "Received handshake response");
                        self.handle_response(peer, response);
                    }
                    RequestResponseEvent::OutboundFailure { peer, error, .. } => {
                        self.events.push_back(Event::Failed {
                            peer_id: peer,
                            error: Box::new(Error::Outbound(error)),
                        });
                    }
                    RequestResponseEvent::InboundFailure { peer, error, .. } => {
                        self.events.push_back(Event::Failed {
                            peer_id: peer,
                            error: Box::new(Error::Inbound(error)),
                        });
                    }
                    RequestResponseEvent::ResponseSent { .. } => {}
                },
                other => {
                    // Bubble up all other ToSwarm events (Dial, NotifyHandler, CloseConnection,
                    // etc.) These events don't contain GenerateEvent, so
                    // map_out's closure is never called. This is safe because
                    // we've exhaustively handled all GenerateEvent variants above.
                    return Poll::Ready(
                        other.map_out(|_| unreachable!("GenerateEvent already handled")),
                    );
                }
            }
        }

        // Emit queued events
        if let Some(event) = self.events.pop_front() {
            return Poll::Ready(ToSwarm::GenerateEvent(event));
        }

        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    // Init tracing
    static DEBUG: LazyLock<()> = LazyLock::new(|| {
        let env_filter = tracing_subscriber::EnvFilter::new("debug");
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    });

    use std::sync::LazyLock;

    use discv5::libp2p_identity::Keypair;
    use libp2p::swarm::Swarm;
    use libp2p_swarm_test::{SwarmExt, drive};

    use super::*;
    use crate::handshake::node_info::NodeMetadata;

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

    fn assert_completed(event: Event, expected_peer: PeerId, expected_version: &str) {
        match event {
            Event::Completed {
                peer_id,
                their_info,
            } => {
                assert_eq!(peer_id, expected_peer);
                assert_eq!(their_info.metadata.unwrap().node_version, expected_version);
            }
            Event::Failed { error, .. } => panic!("Expected Completed, got Failed: {:?}", error),
        }
    }

    fn assert_network_mismatch(
        event: Event,
        expected_peer: PeerId,
        expected_ours: &str,
        expected_theirs: &str,
    ) {
        match event {
            Event::Failed { peer_id, error } => {
                assert_eq!(peer_id, expected_peer);
                match *error {
                    Error::NetworkMismatch { ours, theirs } => {
                        assert_eq!(ours, expected_ours);
                        assert_eq!(theirs, expected_theirs);
                    }
                    _ => panic!("Expected NetworkMismatch, got {:?}", error),
                }
            }
            Event::Completed { .. } => panic!("Expected Failed, got Completed"),
        }
    }

    #[tokio::test]
    async fn handshake_success() {
        *DEBUG;

        let mut local_swarm =
            create_test_swarm(Keypair::generate_ed25519(), node_info("test", "local"));
        let mut remote_swarm =
            create_test_swarm(Keypair::generate_ed25519(), node_info("test", "remote"));

        tokio::spawn(async move {
            local_swarm.listen().with_memory_addr_external().await;
            remote_swarm.connect(&mut local_swarm).await;

            let ([local_event], [remote_event]): ([Event; 1], [Event; 1]) =
                drive(&mut local_swarm, &mut remote_swarm).await;

            assert_completed(local_event, *remote_swarm.local_peer_id(), "remote");
            assert_completed(remote_event, *local_swarm.local_peer_id(), "local");
        })
        .await
        .expect("test completed");
    }

    /// Test that verifies only ONE handshake happens during concurrent dials.
    ///
    /// Without the `other_established == 0` check, this test would see BOTH peers
    /// initiate handshakes, leading to duplicate requests. With the check, only
    /// the first ConnectionEstablished triggers a handshake initiation.
    #[tokio::test]
    async fn concurrent_dials_only_one_handshake() {
        *DEBUG;

        let mut local_swarm =
            create_test_swarm(Keypair::generate_ed25519(), node_info("test", "local"));
        let mut remote_swarm =
            create_test_swarm(Keypair::generate_ed25519(), node_info("test", "remote"));

        tokio::spawn(async move {
            local_swarm.listen().with_memory_addr_external().await;
            remote_swarm.listen().with_memory_addr_external().await;

            // Force both peers to dial each other
            let local_addr = local_swarm.external_addresses().next().unwrap().clone();
            let remote_addr = remote_swarm.external_addresses().next().unwrap().clone();

            local_swarm.dial(remote_addr).unwrap();
            remote_swarm.dial(local_addr).unwrap();

            // Drive until both complete - expecting exactly 1 event per peer
            let ([local_event], [remote_event]): ([Event; 1], [Event; 1]) =
                drive(&mut local_swarm, &mut remote_swarm).await;

            // Both should have completed successfully
            assert_completed(local_event, *remote_swarm.local_peer_id(), "remote");
            assert_completed(remote_event, *local_swarm.local_peer_id(), "local");

            // Key assertion: If we try to drive again with a timeout,
            // there should be NO more events (no duplicate handshakes)
            use tokio::time::{timeout, Duration};

            let result = timeout(Duration::from_millis(100), async {
                let ([_local], [_remote]): ([Event; 1], [Event; 1]) =
                    drive(&mut local_swarm, &mut remote_swarm).await;
            }).await;

            // Should timeout - no more handshake events should occur
            assert!(result.is_err(), "Expected no more handshake events, but got some! This means duplicate handshakes occurred.");
        })
        .await
        .expect("test completed");
    }

    #[tokio::test]
    async fn mismatched_networks_handshake_failed() {
        *DEBUG;

        let mut local_swarm =
            create_test_swarm(Keypair::generate_ed25519(), node_info("test1", "local"));
        let mut remote_swarm =
            create_test_swarm(Keypair::generate_ed25519(), node_info("test2", "remote"));

        tokio::spawn(async move {
            local_swarm.listen().with_memory_addr_external().await;
            remote_swarm.connect(&mut local_swarm).await;

            let ([local_event], [remote_event]): ([Event; 1], [Event; 1]) =
                drive(&mut local_swarm, &mut remote_swarm).await;

            assert_network_mismatch(local_event, *remote_swarm.local_peer_id(), "test1", "test2");
            assert_network_mismatch(remote_event, *local_swarm.local_peer_id(), "test2", "test1");
        })
        .await
        .expect("test completed");
    }
}
