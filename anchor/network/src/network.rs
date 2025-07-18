use std::{
    collections::HashMap,
    num::{NonZeroU8, NonZeroUsize},
    pin::Pin,
    sync::Arc,
    time::Instant,
};

use futures::StreamExt;
use gossipsub::{IdentTopic, PublishError};
use libp2p::{
    Multiaddr, PeerId, Swarm, SwarmBuilder, TransportError,
    core::{ConnectedPoint, muxing::StreamMuxerBox, transport::Boxed},
    futures,
    identity::Keypair,
    multiaddr::Protocol,
    swarm::SwarmEvent,
};
use lighthouse_network::{discovery::DiscoveredPeers, prometheus_client::registry::Registry};
use message_receiver::{MessageReceiver, Outcome};
use ssv_types::domain_type::DomainType;
use subnet_service::{SUBNET_COUNT, SubnetEvent, SubnetId};
use task_executor::TaskExecutor;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};
use types::{ChainSpec, EthSpec};
use version::version_with_platform;

use crate::{
    Config, Enr,
    behaviour::{AnchorBehaviour, AnchorBehaviourEvent, BehaviourError},
    discovery::{Discovery, DiscoveryError},
    handshake,
    handshake::node_info::{NodeInfo, NodeMetadata},
    keypair_utils::load_private_key,
    network::NetworkError::SwarmConfig,
    peer_manager,
    peer_manager::{ConnectActions, PeerManager},
    scoring::topic_score_config::topic_score_params_for_subnet_with_rate,
    transport::build_transport,
};

const MAX_TRANSMIT_SIZE_BYTES: usize = 5_000_000;

#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("Unable to listen on address {address}: {source}")]
    Listen {
        address: Multiaddr,
        #[source]
        source: TransportError<std::io::Error>,
    },

    #[error("Behaviour error: {0}")]
    Behaviour(#[from] BehaviourError),

    #[error("Discovery error: {0}")]
    Discovery(#[from] DiscoveryError),

    #[error("Swarm config error: {0}")]
    SwarmConfig(String),

    #[error("DNS transport config error: {0}")]
    DnsTransport(std::io::Error),
}

pub struct Network<R: MessageReceiver> {
    swarm: Swarm<AnchorBehaviour>,
    subnet_event_receiver: mpsc::Receiver<SubnetEvent>,
    message_rx: mpsc::Receiver<(SubnetId, Vec<u8>)>,
    peer_id: PeerId,
    node_info: NodeInfo,
    message_receiver: Arc<R>,
    outcome_rx: mpsc::Receiver<Outcome>,
    domain_type: DomainType,
    metrics_registry: Option<Registry>,
    spec: Arc<ChainSpec>,
}

impl<R: MessageReceiver> Network<R> {
    // Creates an instance of the Network struct to start sending and receiving information on the
    // p2p network.
    pub async fn try_new<E: EthSpec>(
        config: &Config,
        subnet_event_receiver: mpsc::Receiver<SubnetEvent>,
        message_rx: mpsc::Receiver<(SubnetId, Vec<u8>)>,
        message_receiver: Arc<R>,
        outcome_rx: mpsc::Receiver<Outcome>,
        executor: TaskExecutor,
        spec: Arc<ChainSpec>,
    ) -> Result<Network<R>, Box<NetworkError>> {
        let local_keypair: Keypair = load_private_key(&config.network_dir);

        let transport = build_transport(local_keypair.clone(), !config.disable_quic_support)?;

        let mut metrics_registry = Registry::default();

        let behaviour =
            AnchorBehaviour::new::<E>(local_keypair.clone(), config, &mut metrics_registry, &spec)
                .await
                .map_err(|e| Box::new(NetworkError::Behaviour(e)))?;

        let peer_id = local_keypair.public().to_peer_id();
        let domain_type: String = config.domain_type.clone().into();
        let node_info = NodeInfo::new(
            domain_type,
            Some(NodeMetadata {
                node_version: version_with_platform(),
                execution_node: "geth/v1.10.8".to_string(),
                consensus_node: "lighthouse/v1.5.0".to_string(),
                subnets: "00000000000000000000000000000000".to_string(),
            }),
        );

        let mut network = Network {
            swarm: build_swarm(
                executor.clone(),
                local_keypair,
                transport,
                behaviour,
                &mut metrics_registry,
            )?,
            subnet_event_receiver,
            message_rx,
            peer_id,
            node_info,
            message_receiver,
            outcome_rx,
            domain_type: config.domain_type.clone(),
            metrics_registry: Some(metrics_registry),
            spec,
        };

        info!(%peer_id, "Network starting");

        for listen_multiaddr in config.listen_addresses.libp2p_addresses() {
            // If QUIC is disabled, ignore listening on QUIC ports
            if config.disable_quic_support && listen_multiaddr.iter().any(|v| v == Protocol::QuicV1)
            {
                continue;
            }

            network
                .swarm
                .listen_on(listen_multiaddr.clone())
                .map_err(|transport_err| NetworkError::Listen {
                    address: listen_multiaddr.clone(),
                    source: transport_err,
                })?;

            let mut log_address = listen_multiaddr;
            log_address.push(Protocol::P2p(peer_id));
            info!(address = %log_address, "Listening established");
        }

        Ok(network)
    }

    pub fn take_metrics_registry(&mut self) -> Option<Registry> {
        self.metrics_registry.take()
    }

    /// Main loop for polling and handling swarm and channels.
    pub async fn run<E: EthSpec>(mut self) {
        loop {
            tokio::select! {
                swarm_message = self.swarm.select_next_some() => {
                    match swarm_message {
                        SwarmEvent::Behaviour(behaviour_event) => match behaviour_event {
                            AnchorBehaviourEvent::Gossipsub(ge) => {
                                match ge {
                                    gossipsub::Event::Message {
                                        propagation_source,
                                        message_id,
                                        message,
                                    } => {
                                        trace!(
                                            source = ?propagation_source,
                                            id = ?message_id,
                                            "Received SignedSSVMessage"
                                        );
                                        if let Err(err) = self.message_receiver.receive(propagation_source, message_id, message) {
                                            error!(?err, "Unable to pass message to message receiver");
                                        }
                                    }
                                    _ => {
                                        trace!(event = ?ge, "Unhandled gossipsub event");
                                    }
                                }
                            },
                            AnchorBehaviourEvent::Discovery(DiscoveredPeers { peers }) => {
                                self.on_discovered_peers(peers);
                            }
                            AnchorBehaviourEvent::Handshake(event) => {
                                if let Some(result) = handshake::handle_event(
                                    &self.node_info,
                                    &mut self.swarm.behaviour_mut().handshake,
                                    event,
                                ) {
                                    self.handle_handshake_result(result);
                                }
                            }
                            AnchorBehaviourEvent::PeerManager(peer_manager::Event::Heartbeat(heartbeat)) => {
                                if let Some(actions) = heartbeat.connect_actions {
                                    self.handle_connect_actions(actions);
                                }
                                if heartbeat.check_peer_scores {
                                    self.check_and_block_peers_by_score();
                                }
                            }
                            _ => {
                                trace!(event = ?behaviour_event, "Unhandled behaviour event");
                            }
                        },
                        SwarmEvent::ConnectionEstablished {
                            peer_id,
                            endpoint: ConnectedPoint::Dialer { .. },
                            ..
                        } => {
                            handshake::initiate(
                                    &self.node_info,
                                &mut self.swarm.behaviour_mut().handshake,
                                peer_id
                            );
                        }
                        _ => {
                            trace!(event = ?swarm_message, "Unhandled swarm event");
                        }
                    }
                },
                Some(event) = self.subnet_event_receiver.recv() => {
                    self.on_subnet_tracker_event::<E>(event)
                }
                event = self.message_rx.recv() => {
                    match event {
                        Some((subnet_id, message)) => {
                            if let Err(err) = self.gossipsub().publish(subnet_to_topic(subnet_id), message) {
                                if !matches!(err, PublishError::Duplicate) {
                                    error!(?err, "Failed to publish message");
                                }
                            }
                        }
                        None => {
                            error!("message queue was closed");
                            return;
                        }
                    }
                }
                event = self.outcome_rx.recv() => {
                    match event {
                        Some(outcome) => {
                            self.gossipsub()
                                .report_message_validation_result(
                                    &outcome.message_id,
                                    &outcome.propagation_source,
                                    outcome.action,
                                );
                        }
                        None => {
                            error!("message validator has quit");
                            return;
                        }
                    }
                }
            }
        }
    }

    fn on_discovered_peers(&mut self, peers: HashMap<Enr, Option<Instant>>) {
        debug!(peers =  ?peers, "Peers discovered");
        let manager = self.peer_manager();
        // need to collect to avoid double borrow
        let to_dial = peers
            .into_iter()
            .filter_map(|(enr, _)| manager.report_discovered_peer(enr))
            .collect::<Vec<_>>();
        for dial in to_dial {
            let _ = self.swarm.dial(dial);
        }
    }

    /// Update topic score parameters for a subnet with pre-calculated message rate
    fn update_topic_score_for_subnet_with_rate<E: EthSpec>(
        &mut self,
        subnet: SubnetId,
        topic: IdentTopic,
        message_rate: f64,
    ) {
        debug!(
            subnet = *subnet,
            topic = %topic,
            message_rate = message_rate,
            "Setting topic score parameters with pre-calculated message rate"
        );

        // Generate topic-specific score parameters using pre-calculated message rate
        let topic_score_params = topic_score_params_for_subnet_with_rate::<E>(
            subnet,
            SUBNET_COUNT,
            message_rate,
            &self.spec,
        );

        // Apply the score parameters to the topic
        match self
            .swarm
            .behaviour_mut()
            .gossipsub
            .set_topic_params(topic.clone(), topic_score_params)
        {
            Ok(_) => {
                debug!(
                    subnet = *subnet,
                    topic = %topic,
                    message_rate = message_rate,
                    "Successfully updated topic score parameters with pre-calculated rate"
                );
            }
            Err(e) => {
                warn!(
                    subnet = *subnet,
                    topic = %topic,
                    error = %e,
                    "Failed to set topic score params with pre-calculated rate"
                );
            }
        }
    }

    fn on_subnet_tracker_event<E: EthSpec>(&mut self, event: SubnetEvent) {
        let (subnet, subscribed) = match event {
            SubnetEvent::Join(subnet, message_rate_opt) => {
                let topic = subnet_to_topic(subnet);
                if let Err(err) = self.gossipsub().subscribe(&topic) {
                    error!(?err, subnet = *subnet, "can't subscribe");
                    return;
                }

                // Only set topic score parameters if message rate is provided (scoring enabled)
                if let Some(message_rate) = message_rate_opt {
                    self.update_topic_score_for_subnet_with_rate::<E>(subnet, topic, message_rate);
                } else {
                    debug!(
                        subnet = *subnet,
                        "Skipping topic score parameter setup - gossipsub scoring disabled"
                    );
                }

                let actions = self.peer_manager().join_subnet(subnet);
                self.handle_connect_actions(actions);
                (subnet, true)
            }
            SubnetEvent::Leave(subnet) => {
                self.gossipsub().unsubscribe(&subnet_to_topic(subnet));
                (subnet, false)
            }
            SubnetEvent::RateUpdate(subnet, message_rate) => {
                let topic = subnet_to_topic(subnet);

                debug!(
                    subnet = *subnet,
                    message_rate = message_rate,
                    "Updating topic scores for subnet due to rate changes"
                );

                self.update_topic_score_for_subnet_with_rate::<E>(subnet, topic, message_rate);

                // No subscription change needed, just score update
                return;
            }
        };

        // update enr and metadata to new state
        self.discovery().set_subscribed(subnet, subscribed);
        if let Some(metadata) = &mut self.node_info.metadata {
            if let Err(err) = metadata.set_subscribed(subnet, subscribed) {
                error!(?err, "unable to update node info");
            }
        }
    }

    fn peer_manager(&mut self) -> &mut PeerManager {
        &mut self.swarm.behaviour_mut().peer_manager
    }

    fn gossipsub(&mut self) -> &mut gossipsub::Behaviour {
        &mut self.swarm.behaviour_mut().gossipsub
    }

    fn discovery(&mut self) -> &mut Discovery {
        &mut self.swarm.behaviour_mut().discovery
    }

    fn handle_connect_actions(&mut self, connect_actions: ConnectActions) {
        for peer in connect_actions.dial {
            let _ = self.swarm.dial(peer);
        }
        if !connect_actions.discover.is_empty() {
            self.swarm
                .behaviour_mut()
                .discovery
                .start_subnet_query(connect_actions.discover);
        }
    }

    fn handle_handshake_result(&mut self, result: Result<handshake::Completed, handshake::Failed>) {
        match result {
            Ok(handshake::Completed {
                peer_id,
                their_info,
            }) => {
                debug!(%peer_id, ?their_info, "Handshake completed");
                // Update peer store with their_info
            }
            Err(handshake::Failed { peer_id, error }) => {
                debug!(%peer_id, ?error, "Handshake failed");
            }
        }
    }

    /// Get the list of currently blocked peers.
    pub fn blocked_peers(&self) -> &std::collections::HashSet<PeerId> {
        self.swarm.behaviour().peer_manager.blocked_peers()
    }

    /// Check gossipsub peer scores and block peers with scores below graylist threshold
    pub fn check_and_block_peers_by_score(&mut self) {
        use crate::scoring::peer_score_config::GRAYLIST_THRESHOLD;

        let gossipsub = &self.swarm.behaviour().gossipsub;

        // Get all peers with poor scores that should be blocked
        let peers_to_block: Vec<PeerId> = self
            .swarm
            .connected_peers()
            .filter_map(|peer_id| {
                if let Some(score) = gossipsub.peer_score(peer_id) {
                    if score < GRAYLIST_THRESHOLD {
                        Some(*peer_id)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        // Block the peers (connections will be closed automatically)
        for peer_id in peers_to_block {
            self.swarm.behaviour_mut().peer_manager.block_peer(peer_id);
        }
    }
}

fn build_swarm(
    executor: TaskExecutor,
    local_keypair: Keypair,
    transport: Boxed<(PeerId, StreamMuxerBox)>,
    behaviour: AnchorBehaviour,
    metrics_registry: &mut Registry,
) -> Result<Swarm<AnchorBehaviour>, Box<NetworkError>> {
    struct Executor(task_executor::TaskExecutor);
    impl libp2p::swarm::Executor for Executor {
        fn exec(&self, f: Pin<Box<dyn futures::Future<Output = ()> + Send>>) {
            self.0.spawn(f, "libp2p");
        }
    }

    let notify_handler_buffer_size = NonZeroUsize::new(7)
        .ok_or_else(|| SwarmConfig("notify_handler_buffer_size must be > 0".to_string()))?;

    let dial_concurrency_factor = NonZeroU8::new(1)
        .ok_or_else(|| SwarmConfig("dial_concurrency_factor cannot be 0".to_string()))?;

    let swarm_config = libp2p::swarm::Config::with_executor(Executor(executor))
        .with_notify_handler_buffer_size(notify_handler_buffer_size)
        .with_per_connection_event_buffer_size(4)
        .with_dial_concurrency_factor(dial_concurrency_factor);

    let swarm = SwarmBuilder::with_existing_identity(local_keypair)
        .with_tokio()
        .with_other_transport(|_key| transport)
        .expect("infallible") // This operation can't fail because the error type is Infallible.
        .with_bandwidth_metrics(metrics_registry)
        .with_behaviour(|_| behaviour)
        .expect("infallible") // Again, this can't fail.
        .with_swarm_config(|_| swarm_config)
        .build();

    Ok(swarm)
}

fn subnet_to_topic(subnet: SubnetId) -> IdentTopic {
    IdentTopic::new(format!("ssv.v2.{}", *subnet))
}
