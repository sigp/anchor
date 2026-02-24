mod codec;
mod envelope;
pub mod node_info;

use std::{
    collections::VecDeque,
    task::{Context, Poll},
};

use discv5::libp2p_identity::Keypair;
use fork::ForkLifecycle;
use libp2p::{
    PeerId, StreamProtocol,
    request_response::{
        Behaviour as RequestResponseBehaviour, Config, Event as RequestResponseEvent,
        InboundFailure, Message, OutboundFailure, ProtocolSupport, ResponseChannel,
    },
    swarm::{NetworkBehaviour, THandlerInEvent, ToSwarm},
};
use tokio::sync::watch;
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
    lifecycle_rx: watch::Receiver<ForkLifecycle>,
    metadata: node_info::NodeMetadata,
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
    pub fn new(
        keypair: Keypair,
        lifecycle_rx: watch::Receiver<ForkLifecycle>,
        metadata: node_info::NodeMetadata,
    ) -> Self {
        let protocol = StreamProtocol::new("/ssv/info/0.0.1");
        let inner = RequestResponseBehaviour::with_codec(
            Codec::new(keypair),
            [(protocol, ProtocolSupport::Full)],
            Config::default(),
        );
        Self {
            inner,
            lifecycle_rx,
            metadata,
            events: VecDeque::new(),
        }
    }

    /// Construct our [`NodeInfo`] from the current shared domain type and metadata.
    fn our_node_info(&self) -> NodeInfo {
        NodeInfo {
            domain_type: self
                .lifecycle_rx
                .borrow()
                .current_fork_config()
                .domain_type
                .into(),
            metadata: Some(self.metadata.clone()),
        }
    }

    fn verify_and_emit_event(&mut self, peer_id: PeerId, their_info: NodeInfo) {
        let our_info = self.our_node_info();
        match verify_node_info(&our_info, &their_info) {
            Ok(()) => {
                // Log handshake completion and record metrics
                if let Some(metadata) = &their_info.metadata {
                    let our_metadata = self.node_metadata();
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
            .send_response(channel, self.our_node_info())
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

    pub fn node_metadata(&self) -> &node_info::NodeMetadata {
        &self.metadata
    }

    pub fn node_metadata_mut(&mut self) -> &mut node_info::NodeMetadata {
        &mut self.metadata
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
    if ours.domain_type != theirs.domain_type {
        return Err(Error::NetworkMismatch {
            ours: ours.domain_type.clone(),
            theirs: theirs.domain_type.clone(),
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
            self.inner.send_request(peer_id, self.our_node_info());
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
    use fork::{Fork, ForkConfig, ForkLifecycle};
    use libp2p::swarm::Swarm;
    use libp2p_swarm_test::{SwarmExt, drive};
    use ssv_types::domain_type::DomainType;
    use tokio::sync::watch;
    use types::Epoch;

    use super::*;
    use crate::handshake::node_info::NodeMetadata;

    const DOMAIN_A: DomainType = DomainType([0, 0, 0, 1]);
    const DOMAIN_B: DomainType = DomainType([0, 0, 0, 2]);

    fn lifecycle_normal(domain_type: DomainType) -> ForkLifecycle {
        ForkLifecycle::Normal {
            current: ForkConfig::new(Fork::Alan, Epoch::new(0), domain_type),
        }
    }

    fn test_metadata(version: &str) -> NodeMetadata {
        NodeMetadata {
            node_version: version.to_string(),
            execution_node: "".to_string(),
            consensus_node: "".to_string(),
            subnets: "".to_string(),
        }
    }

    fn create_test_swarm(
        keypair: Keypair,
        domain_type: DomainType,
        version: &str,
    ) -> Swarm<Behaviour> {
        let (_tx, rx) = watch::channel(lifecycle_normal(domain_type));
        let metadata = test_metadata(version);
        Swarm::new_ephemeral_tokio(|_| Behaviour::new(keypair, rx, metadata))
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

        let mut local_swarm = create_test_swarm(Keypair::generate_ed25519(), DOMAIN_A, "local");
        let mut remote_swarm = create_test_swarm(Keypair::generate_ed25519(), DOMAIN_A, "remote");

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

        let mut local_swarm = create_test_swarm(Keypair::generate_ed25519(), DOMAIN_A, "local");
        let mut remote_swarm = create_test_swarm(Keypair::generate_ed25519(), DOMAIN_A, "remote");

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

        let mut local_swarm = create_test_swarm(Keypair::generate_ed25519(), DOMAIN_A, "local");
        let mut remote_swarm = create_test_swarm(Keypair::generate_ed25519(), DOMAIN_B, "remote");

        let domain_a_hex: String = DOMAIN_A.into();
        let domain_b_hex: String = DOMAIN_B.into();

        tokio::spawn(async move {
            local_swarm.listen().with_memory_addr_external().await;
            remote_swarm.connect(&mut local_swarm).await;

            let ([local_event], [remote_event]): ([Event; 1], [Event; 1]) =
                drive(&mut local_swarm, &mut remote_swarm).await;

            assert_network_mismatch(
                local_event,
                *remote_swarm.local_peer_id(),
                &domain_a_hex,
                &domain_b_hex,
            );
            assert_network_mismatch(
                remote_event,
                *local_swarm.local_peer_id(),
                &domain_b_hex,
                &domain_a_hex,
            );
        })
        .await
        .expect("test completed");
    }

    fn create_test_swarm_with_lifecycle_rx(
        keypair: Keypair,
        lifecycle_rx: watch::Receiver<ForkLifecycle>,
        version: &str,
    ) -> Swarm<Behaviour> {
        let metadata = test_metadata(version);
        Swarm::new_ephemeral_tokio(|_| Behaviour::new(keypair, lifecycle_rx, metadata))
    }

    /// Tests that updates to the fork lifecycle watch channel propagate to subsequent
    /// handshakes.
    ///
    /// This validates the core fix from PR #814: when a fork activates and updates the
    /// fork lifecycle, all future handshakes use the new domain type. The test
    /// runs through three phases:
    /// 1. Both peers on DOMAIN_A - handshake succeeds
    /// 2. Only local updates to DOMAIN_B - handshake fails with NetworkMismatch
    /// 3. Both peers update to DOMAIN_B - handshake succeeds again
    #[tokio::test]
    async fn watch_lifecycle_updates_propagate_to_handshakes() {
        use futures::future::Either;
        use libp2p::swarm::SwarmEvent;

        *DEBUG;

        let domain_a_hex: String = DOMAIN_A.into();
        let domain_b_hex: String = DOMAIN_B.into();

        // Arrange: Create watch channels externally so we can update them
        let (local_tx, local_rx) = watch::channel(lifecycle_normal(DOMAIN_A));
        let (remote_tx, remote_rx) = watch::channel(lifecycle_normal(DOMAIN_A));

        let local_keypair = Keypair::generate_ed25519();
        let remote_keypair = Keypair::generate_ed25519();

        let mut local_swarm = create_test_swarm_with_lifecycle_rx(local_keypair, local_rx, "local");
        let mut remote_swarm =
            create_test_swarm_with_lifecycle_rx(remote_keypair, remote_rx, "remote");

        tokio::spawn(async move {
            // ==================== Phase 1: Both on DOMAIN_A - handshake succeeds
            // ====================

            local_swarm.listen().with_memory_addr_external().await;
            remote_swarm.listen().with_memory_addr_external().await;
            remote_swarm.connect(&mut local_swarm).await;

            let ([local_event], [remote_event]): ([Event; 1], [Event; 1]) =
                drive(&mut local_swarm, &mut remote_swarm).await;

            assert_completed(local_event, *remote_swarm.local_peer_id(), "remote");
            assert_completed(remote_event, *local_swarm.local_peer_id(), "local");

            // ==================== Phase 2: Only local updates to DOMAIN_B - mismatch
            // ====================

            // Act: Update only the local fork lifecycle (simulates fork activation)
            let _ = local_tx.send(ForkLifecycle::Normal {
                current: ForkConfig::new(Fork::Boole, Epoch::new(100), DOMAIN_B),
            });

            // Disconnect both peers
            let remote_peer = *remote_swarm.local_peer_id();
            let local_peer = *local_swarm.local_peer_id();
            local_swarm
                .disconnect_peer_id(remote_peer)
                .expect("disconnect remote from local");
            remote_swarm
                .disconnect_peer_id(local_peer)
                .expect("disconnect local from remote");

            // Wait for ConnectionClosed on both sides
            let mut local_closed = false;
            let mut remote_closed = false;
            loop {
                match futures::future::select(
                    local_swarm.next_swarm_event(),
                    remote_swarm.next_swarm_event(),
                )
                .await
                {
                    Either::Left((SwarmEvent::ConnectionClosed { .. }, _)) => {
                        local_closed = true;
                    }
                    Either::Right((SwarmEvent::ConnectionClosed { .. }, _)) => {
                        remote_closed = true;
                    }
                    _ => {} // keep polling
                }
                if local_closed && remote_closed {
                    break;
                }
            }

            // Reconnect: remote connects to local to trigger outbound handshake
            remote_swarm.connect(&mut local_swarm).await;

            let ([local_event], [remote_event]): ([Event; 1], [Event; 1]) =
                drive(&mut local_swarm, &mut remote_swarm).await;

            // Assert: Both sides detect network mismatch
            assert_network_mismatch(local_event, remote_peer, &domain_b_hex, &domain_a_hex);
            assert_network_mismatch(remote_event, local_peer, &domain_a_hex, &domain_b_hex);

            // ==================== Phase 3: Both update to DOMAIN_B - handshake succeeds
            // ====================

            // Act: Update remote's fork lifecycle to match
            let _ = remote_tx.send(ForkLifecycle::Normal {
                current: ForkConfig::new(Fork::Boole, Epoch::new(100), DOMAIN_B),
            });

            // Disconnect both peers again
            local_swarm
                .disconnect_peer_id(remote_peer)
                .expect("disconnect remote from local");
            remote_swarm
                .disconnect_peer_id(local_peer)
                .expect("disconnect local from remote");

            // Wait for ConnectionClosed on both sides
            let mut local_closed = false;
            let mut remote_closed = false;
            loop {
                match futures::future::select(
                    local_swarm.next_swarm_event(),
                    remote_swarm.next_swarm_event(),
                )
                .await
                {
                    Either::Left((SwarmEvent::ConnectionClosed { .. }, _)) => {
                        local_closed = true;
                    }
                    Either::Right((SwarmEvent::ConnectionClosed { .. }, _)) => {
                        remote_closed = true;
                    }
                    _ => {} // keep polling
                }
                if local_closed && remote_closed {
                    break;
                }
            }

            // Reconnect
            remote_swarm.connect(&mut local_swarm).await;

            let ([local_event], [remote_event]): ([Event; 1], [Event; 1]) =
                drive(&mut local_swarm, &mut remote_swarm).await;

            // Assert: Both sides complete successfully on the new domain
            assert_completed(local_event, remote_peer, "remote");
            assert_completed(remote_event, local_peer, "local");
        })
        .await
        .expect("test completed");
    }
}
