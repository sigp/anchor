use std::{
    collections::{HashMap, HashSet},
    num::{NonZeroU8, NonZeroUsize},
    ops::ControlFlow,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use fork::ForkLifecycle;
use futures::StreamExt;
use gossipsub::{IdentTopic, PublishError};
use libp2p::{
    Multiaddr, PeerId, Swarm, SwarmBuilder, TransportError,
    core::{
        muxing::StreamMuxerBox,
        transport::{Boxed, ListenerId},
    },
    futures,
    identity::Keypair,
    multiaddr::Protocol,
    swarm::{SwarmEvent, dial_opts::DialOpts},
    upnp::Event,
};
use message_receiver::{MessageReceiver, Outcome, TopicContext};
use prometheus_client::registry::Registry;
use subnet_service::{SUBNET_COUNT, SubnetId, TopicEvent, topic};
use task_executor::TaskExecutor;
use thiserror::Error;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, trace, warn};
use types::{ChainSpec, EthSpec};

use crate::{
    Config, Enr,
    behaviour::{AnchorBehaviour, AnchorBehaviourEvent, BehaviourError},
    discovery::{DiscoveredPeers, Discovery, DiscoveryError},
    handshake,
    keypair_utils::load_private_key,
    network::NetworkError::SwarmConfig,
    peer_manager::{self, ConnectActions, PeerManager},
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
    topic_event_receiver: mpsc::Receiver<TopicEvent>,
    /// Receiver for outgoing messages. Tuple of (topic string, message bytes).
    /// Per SIP-43, the topic is determined by the message sender based on message slot.
    message_rx: mpsc::Receiver<(String, Vec<u8>)>,
    peer_id: PeerId,
    message_receiver: Arc<R>,
    outcome_rx: mpsc::Receiver<Outcome>,
    metrics_registry: Option<Registry>,
    spec: Arc<ChainSpec>,
    is_dynamic_target_peers: bool,
    subnet_subscription_counts: HashMap<SubnetId, usize>,
    /// Receiver for fork lifecycle state changes.
    /// Used to update ENR domain type on fork activation.
    lifecycle_rx: watch::Receiver<ForkLifecycle>,
}

impl<R: MessageReceiver> Network<R> {
    // Creates an instance of the Network struct to start sending and receiving information on the
    // p2p network.
    #[expect(clippy::too_many_arguments)]
    pub async fn try_new<E: EthSpec>(
        config: &Config,
        topic_event_receiver: mpsc::Receiver<TopicEvent>,
        message_rx: mpsc::Receiver<(String, Vec<u8>)>,
        message_receiver: Arc<R>,
        outcome_rx: mpsc::Receiver<Outcome>,
        executor: TaskExecutor,
        spec: Arc<ChainSpec>,
        lifecycle_rx: watch::Receiver<ForkLifecycle>,
    ) -> Result<Network<R>, Box<NetworkError>> {
        let local_keypair: Keypair = load_private_key(&config.network_dir.key_file());

        // Determine if we should dynamically adjust target_peers when subnets change.
        // If the user specified a target_peers value, we keep it static. Otherwise, dynamic.
        let is_dynamic_target_peers = config.target_peers.is_none();

        let transport = build_transport(local_keypair.clone(), !config.disable_quic_support)?;

        let mut metrics_registry = Registry::default();

        let behaviour = AnchorBehaviour::new::<E>(
            local_keypair.clone(),
            config,
            &mut metrics_registry,
            &spec,
            lifecycle_rx.clone(),
        )
        .await
        .map_err(|e| Box::new(NetworkError::Behaviour(e)))?;

        let peer_id = local_keypair.public().to_peer_id();

        let mut network = Network {
            swarm: build_swarm(
                executor.clone(),
                local_keypair,
                transport,
                behaviour,
                &mut metrics_registry,
            )?,
            topic_event_receiver,
            message_rx,
            peer_id,
            message_receiver,
            outcome_rx,
            metrics_registry: Some(metrics_registry),
            spec,
            is_dynamic_target_peers,
            subnet_subscription_counts: HashMap::new(),
            lifecycle_rx,
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
                    self.handle_swarm_event(swarm_message);
                }

                Some(event) = self.topic_event_receiver.recv() => {
                    self.on_topic_event::<E>(event)
                }

                event = self.message_rx.recv() => {
                    if let ControlFlow::Break(()) = self.handle_outbound_message(event) {
                        return;
                    }
                }
                event = self.outcome_rx.recv() => {
                    if let ControlFlow::Break(()) = self.handle_validation_outcome(event) {
                        return;
                    }
                }

                Ok(()) = self.lifecycle_rx.changed() => {
                    self.on_fork_lifecycle_change();
                }
            }
        }
    }

    /// Dispatch a libp2p swarm event to the appropriate handler.
    ///
    /// Keeps the main loop readable by isolating protocol-specific handling.
    fn handle_swarm_event(&mut self, swarm_message: SwarmEvent<AnchorBehaviourEvent>) {
        match swarm_message {
            SwarmEvent::Behaviour(behaviour_event) => {
                self.handle_behaviour_event(behaviour_event);
            }
            SwarmEvent::NewListenAddr {
                listener_id,
                address,
            } => {
                self.on_new_listen_addr(listener_id, address);
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                debug!(?peer_id, ?error, "Outgoing connection error");
            }
            SwarmEvent::IncomingConnectionError {
                error,
                send_back_addr,
                ..
            } => {
                debug!(?send_back_addr, ?error, "Incoming connection error");
            }
            SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                if cause.is_some() {
                    debug!(?peer_id, ?cause, "Connection closed with error");
                } else {
                    trace!(?peer_id, "Connection closed");
                }
            }
            _ => {
                trace!(event = ?swarm_message, "Unhandled swarm event");
            }
        }
    }

    /// Handle behaviour events emitted by the libp2p behaviour.
    fn handle_behaviour_event(&mut self, behaviour_event: AnchorBehaviourEvent) {
        match behaviour_event {
            AnchorBehaviourEvent::Gossipsub(ge) => {
                self.handle_gossipsub_event(ge);
            }
            AnchorBehaviourEvent::Discovery(DiscoveredPeers { peers }) => {
                self.on_discovered_peers(peers);
            }
            AnchorBehaviourEvent::Handshake(event) => {
                self.handle_handshake_result(event);
            }
            AnchorBehaviourEvent::Upnp(upnp_event) => {
                self.on_upnp_event(upnp_event);
            }
            AnchorBehaviourEvent::PeerManager(peer_manager::Event::Heartbeat(heartbeat)) => {
                self.handle_peer_manager_heartbeat(heartbeat);
            }
            _ => {
                trace!(event = ?behaviour_event, "Unhandled behaviour event");
            }
        }
    }

    /// Handle gossipsub events (message flow and subscription changes).
    fn handle_gossipsub_event(&mut self, event: gossipsub::Event) {
        match event {
            gossipsub::Event::Message {
                propagation_source,
                message_id,
                message,
            } => {
                self.handle_gossipsub_message(propagation_source, message_id, message);
            }
            gossipsub::Event::Subscribed { peer_id, topic } => {
                if let Some(parsed) = topic::parse_topic(&topic) {
                    self.peer_manager().set_peer_subscription(
                        peer_id,
                        parsed.fork,
                        parsed.subnet_id,
                        true,
                    );
                }
            }
            gossipsub::Event::Unsubscribed { peer_id, topic } => {
                if let Some(parsed) = topic::parse_topic(&topic) {
                    self.peer_manager().set_peer_subscription(
                        peer_id,
                        parsed.fork,
                        parsed.subnet_id,
                        false,
                    );
                }
            }
            _ => {
                trace!(event = ?event, "Unhandled gossipsub event");
            }
        }
    }

    /// Validate and forward an incoming gossipsub message to the receiver.
    fn handle_gossipsub_message(
        &mut self,
        propagation_source: PeerId,
        message_id: gossipsub::MessageId,
        message: gossipsub::Message,
    ) {
        trace!(
            source = ?propagation_source,
            id = ?message_id,
            "Received SignedSSVMessage"
        );

        // Build topic context for fork-aware validation.
        // If we can't parse the topic, reject immediately - we only
        // subscribe to topics we create, so parsing should always succeed.
        let topic_context = match topic::parse_topic(&message.topic) {
            Some(parsed) => TopicContext::Validate { parsed },
            None => {
                warn!(
                    topic = ?message.topic,
                    "Received message on unparseable topic - this is a bug"
                );
                return;
            }
        };

        if let Err(err) =
            self.message_receiver
                .receive(propagation_source, message_id, message, topic_context)
        {
            error!(?err, "Unable to pass message to message receiver");
        }
    }

    /// Process periodic peer-manager heartbeats (peer discovery, pruning, scoring).
    fn handle_peer_manager_heartbeat(&mut self, heartbeat: peer_manager::heartbeat::Event) {
        if let Some(actions) = heartbeat.connect_actions {
            self.handle_connect_actions(actions);
        }

        if heartbeat.check_peer_scores {
            self.check_block_and_prune_peers_by_score();
        }

        // Trigger periodic subnet-aware peer discovery if below target
        let connected_peers = self.swarm.behaviour().peer_manager.connected_peers();
        let target_peers = self.swarm.behaviour().peer_manager.target_peers();
        if connected_peers < target_peers {
            let needed_subnets: Vec<_> = self
                .swarm
                .behaviour()
                .peer_manager
                .needed_subnets()
                .iter()
                .copied()
                .collect();

            if !needed_subnets.is_empty() {
                debug!(
                    connected_peers,
                    target_peers,
                    subnets = ?needed_subnets,
                    "Below target peer count, triggering subnet-aware peer discovery"
                );
                self.swarm
                    .behaviour_mut()
                    .discovery
                    .start_subnet_query(needed_subnets);
            }
        }

        if self.swarm.behaviour().peer_manager.active_subnet_count() > 0 {
            // Disconnect peers that no longer subscribe to any needed subnets
            let to_disconnect = self
                .swarm
                .behaviour()
                .peer_manager
                .peers_to_disconnect_due_to_subnets();

            for peer_id in to_disconnect {
                self.disconnect_peer(&peer_id, "No longer subscribed to any needed subnets");
            }
        }
    }

    /// Publish an outbound message or signal shutdown if the channel closed.
    fn handle_outbound_message(&mut self, event: Option<(String, Vec<u8>)>) -> ControlFlow<()> {
        match event {
            Some((topic_string, message)) => {
                // Topic is determined by message sender based on message slot (per SIP-43)
                let topic = IdentTopic::new(topic_string);
                if let Err(err) = self.gossipsub().publish(topic, message)
                    && !matches!(err, PublishError::Duplicate)
                {
                    error!(?err, "Failed to publish message");
                }
                ControlFlow::Continue(())
            }
            None => {
                error!("message queue was closed");
                ControlFlow::Break(())
            }
        }
    }

    /// Report validation outcomes to gossipsub or signal shutdown if the channel closed.
    fn handle_validation_outcome(&mut self, event: Option<Outcome>) -> ControlFlow<()> {
        match event {
            Some(outcome) => {
                self.gossipsub().report_message_validation_result(
                    &outcome.message_id,
                    &outcome.propagation_source,
                    outcome.action,
                );
                ControlFlow::Continue(())
            }
            None => {
                error!("message validator has quit");
                ControlFlow::Break(())
            }
        }
    }

    /// Handle fork lifecycle state changes.
    ///
    /// When a fork activates (entering GracePeriod), update the ENR domain type
    /// so other nodes can discover us with the new fork's domain.
    fn on_fork_lifecycle_change(&mut self) {
        let lifecycle = self.lifecycle_rx.borrow_and_update().clone();
        let domain_type = lifecycle.current_fork_config().domain_type;
        // Update ENR so other nodes can discover us with the new fork's domain
        if let Err(e) = self.discovery().update_enr_domain_type(domain_type) {
            error!(?e, "Failed to update ENR domain type after fork activation");
        }
    }

    fn on_new_listen_addr(&mut self, listener_id: ListenerId, address: Multiaddr) {
        trace!(
            ?listener_id,
            ?address,
            "Received NewListenAddr event from swarm"
        );

        let mut addr_iter = address.iter();

        let attempt_enr_update = match addr_iter.next() {
            Some(Protocol::Ip4(_)) => match (addr_iter.next(), addr_iter.next()) {
                (Some(Protocol::Tcp(port)), None) => {
                    self.discovery().try_update_port(true, false, port)
                }
                (Some(Protocol::Udp(port)), Some(Protocol::QuicV1)) => {
                    self.discovery().try_update_port(false, false, port)
                }
                _ => {
                    debug!(
                        ?address,
                        "Encountered unacceptable multiaddr for listening (unsupported transport)"
                    );
                    return;
                }
            },
            Some(Protocol::Ip6(_)) => match (addr_iter.next(), addr_iter.next()) {
                (Some(Protocol::Tcp(port)), None) => {
                    self.discovery().try_update_port(true, true, port)
                }
                (Some(Protocol::Udp(port)), Some(Protocol::QuicV1)) => {
                    self.discovery().try_update_port(false, true, port)
                }
                _ => {
                    debug!(
                        ?address,
                        "Encountered unacceptable multiaddr for listening (unsupported transport)"
                    );
                    return;
                }
            },
            _ => {
                debug!(
                    ?address,
                    "Encountered unacceptable multiaddr for listening (no IP)"
                );
                return;
            }
        };

        let local_enr: Enr = self.discovery().local_enr();

        match attempt_enr_update {
            Ok(true) => {
                info!(
                    enr = local_enr.to_base64(),
                    seq = local_enr.seq(),
                    id = %local_enr.node_id(),
                    ip4 = ?local_enr.ip4(),
                    udp4 = ?local_enr.udp4(),
                    tcp4 = ?local_enr.tcp4(),
                    tcp6 = ?local_enr.tcp6(),
                    udp6 = ?local_enr.udp6(),
                    "Updated local ENR"
                )
            }
            Ok(false) => {} // Nothing to do, ENR already configured
            Err(e) => warn!(error = ?e, "Failed to update ENR"),
        }
    }

    fn on_discovered_peers(&mut self, peers: Vec<Enr>) {
        let manager = self.peer_manager();
        // need to collect to avoid double borrow
        let to_dial = peers
            .into_iter()
            .filter_map(|enr| manager.report_discovered_peer(enr))
            .collect::<Vec<_>>();
        for dial in to_dial {
            self.dial(dial);
        }
    }

    /// Update topic score parameters for a subnet with pre-calculated message rate
    fn update_topic_score_for_subnet_with_rate<E: EthSpec>(
        &mut self,
        subnet: SubnetId,
        topic: IdentTopic,
        message_rate: f64,
    ) {
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
                trace!(
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

    fn on_topic_event<E: EthSpec>(&mut self, event: TopicEvent) {
        let is_dynamic_target_peers = self.is_dynamic_target_peers;
        match event {
            TopicEvent::Subscribe {
                topic,
                subnet,
                message_rate,
            } => {
                let ident_topic = IdentTopic::new(&topic);
                if let Err(err) = self.gossipsub().subscribe(&ident_topic) {
                    error!(?err, %topic, "can't subscribe");
                    return;
                }

                // Set topic score parameters if message rate is provided (scoring enabled)
                if let Some(rate) = message_rate {
                    self.update_topic_score_for_subnet_with_rate::<E>(subnet, ident_topic, rate);
                } else {
                    debug!(
                        %topic,
                        "Skipping topic score parameter setup"
                    );
                }

                let is_first = !self.subnet_subscription_counts.contains_key(&subnet);
                if is_first {
                    let actions = self
                        .peer_manager()
                        .join_subnet(subnet, is_dynamic_target_peers);
                    self.handle_connect_actions(actions);
                    self.update_subnet_membership(subnet, true);
                }
                *self.subnet_subscription_counts.entry(subnet).or_insert(0) += 1;
            }
            TopicEvent::Unsubscribe { topic, subnet } => {
                let ident_topic = IdentTopic::new(&topic);
                self.gossipsub().unsubscribe(&ident_topic);

                let should_leave = match self.subnet_subscription_counts.get_mut(&subnet) {
                    Some(count) if *count > 1 => {
                        *count -= 1;
                        false
                    }
                    Some(_) => {
                        self.subnet_subscription_counts.remove(&subnet);
                        true
                    }
                    None => {
                        debug!(subnet = *subnet, "Unsubscribe for unknown subnet");
                        false
                    }
                };

                if should_leave {
                    self.peer_manager()
                        .leave_subnet(subnet, is_dynamic_target_peers);
                    self.update_subnet_membership(subnet, false);
                }
            }
            TopicEvent::RateUpdate {
                topic,
                message_rate,
            } => {
                let ident_topic = IdentTopic::new(&topic);

                // Extract subnet from topic for scoring (needed for per-subnet parameters)
                if let Some(subnet_id) = topic::extract_subnet_id(&topic) {
                    let subnet = SubnetId::new(subnet_id);
                    self.update_topic_score_for_subnet_with_rate::<E>(
                        subnet,
                        ident_topic,
                        message_rate,
                    );
                } else {
                    warn!(%topic, "Could not extract subnet from topic for rate update");
                }
            }
        };
    }

    fn update_subnet_membership(&mut self, subnet: SubnetId, subscribed: bool) {
        self.discovery().set_subscribed(subnet, subscribed);
        let metadata = self.handshake().node_metadata_mut();
        match metadata.set_subscribed(subnet, subscribed) {
            Ok(()) => {
                info!(
                    subnet = *subnet,
                    subscribed = subscribed,
                    subnets_bitfield = %metadata.subnets,
                    "Updated node_info metadata subnet bitfield"
                );
            }
            Err(err) => {
                error!(?err, "unable to update node info");
            }
        }
    }

    fn on_upnp_event(&mut self, event: Event) {
        match event {
            libp2p::upnp::Event::NewExternalAddr(addr) => {
                info!(%addr, "UPnP route established");
                let mut iter = addr.iter();
                let is_ipv6 = {
                    let addr = iter.next();
                    matches!(addr, Some(Protocol::Ip6(_)))
                };
                match iter.next() {
                    Some(Protocol::Udp(udp_port)) => match iter.next() {
                        Some(Protocol::QuicV1) => {
                            if let Err(e) =
                                self.discovery().try_update_port(false, is_ipv6, udp_port)
                            {
                                warn!(error = e, "Failed to update ENR");
                            }
                        }
                        _ => {
                            trace!(%addr, "UPnP address mapped multiaddr from unknown transport");
                        }
                    },
                    Some(Protocol::Tcp(tcp_port)) => {
                        if let Err(e) = self.discovery().try_update_port(true, is_ipv6, tcp_port) {
                            warn!(error = e, "Failed to update ENR");
                        }
                    }
                    _ => {
                        trace!(%addr, "UPnP address mapped multiaddr from unknown transport");
                    }
                }
            }
            libp2p::upnp::Event::ExpiredExternalAddr(addr) => {
                info!(%addr, "UPnP route expired");
            }
            libp2p::upnp::Event::GatewayNotFound => info!("UPnP not available."),
            libp2p::upnp::Event::NonRoutableGateway => {
                info!("UPnP is available but gateway is not exposed to public network")
            }
        }
    }

    fn peer_manager(&mut self) -> &mut PeerManager {
        &mut self.swarm.behaviour_mut().peer_manager
    }

    fn gossipsub(&mut self) -> &mut gossipsub::Behaviour {
        &mut self.swarm.behaviour_mut().gossipsub
    }

    fn handshake(&mut self) -> &mut handshake::Behaviour {
        &mut self.swarm.behaviour_mut().handshake
    }

    fn discovery(&mut self) -> &mut Discovery {
        &mut self.swarm.behaviour_mut().discovery
    }

    fn handle_connect_actions(&mut self, connect_actions: ConnectActions) {
        for peer in connect_actions.dial {
            self.dial(peer);
        }
        if !connect_actions.discover.is_empty() {
            self.swarm
                .behaviour_mut()
                .discovery
                .start_subnet_query(connect_actions.discover);
        }
    }

    fn handle_handshake_result(&mut self, event: handshake::Event) {
        match event {
            handshake::Event::Completed {
                peer_id,
                their_info,
            } => {
                // Record successful handshake
                if let Ok(counter) = crate::metrics::HANDSHAKE_SUCCESSFUL.as_ref() {
                    counter.inc();
                }

                if let Some(metadata) = their_info.metadata {
                    self.peer_manager()
                        .handle_handshake_completed(peer_id, metadata.node_version.clone());
                }
            }
            handshake::Event::Failed { peer_id, error } => {
                // Determine failure reason for metrics
                let failure_reason = match error.as_ref() {
                    handshake::Error::NetworkMismatch { .. } => "network_mismatch",
                    handshake::Error::NodeInfo(_) => "nodeinfo_error",
                    handshake::Error::Inbound(_) => "inbound_failure",
                    handshake::Error::Outbound(outbound_err) => match outbound_err {
                        libp2p::request_response::OutboundFailure::DialFailure => {
                            "outbound_dial_failure"
                        }
                        libp2p::request_response::OutboundFailure::Timeout => "outbound_timeout",
                        libp2p::request_response::OutboundFailure::ConnectionClosed => {
                            "outbound_connection_closed"
                        }
                        libp2p::request_response::OutboundFailure::UnsupportedProtocols => {
                            "outbound_unsupported_protocols"
                        }
                        libp2p::request_response::OutboundFailure::Io(_) => "outbound_io_error",
                    },
                };

                // Record failed handshake with reason
                if let Ok(counter_vec) = crate::metrics::HANDSHAKE_FAILED.as_ref()
                    && let Ok(counter) = counter_vec.get_metric_with_label_values(&[failure_reason])
                {
                    counter.inc();
                }

                debug!(%peer_id, ?error, reason = failure_reason, "Handshake failed");

                // Disconnect the peer on handshake failure
                self.disconnect_peer(&peer_id, "Handshake failed");
            }
        }
    }

    /// Get the list of currently blocked peers.
    pub fn blocked_peers(&self) -> &HashSet<PeerId> {
        self.swarm.behaviour().peer_manager.blocked_peers()
    }

    /// Check gossipsub peer scores and block peers with scores below graylist threshold
    pub fn check_block_and_prune_peers_by_score(&mut self) {
        use crate::scoring::peer_score_config::GRAYLIST_THRESHOLD;

        // ---------- first pass (read-only) ----------
        let mut peer_scores = Vec::new();
        let mut peers_to_block_and_disconnect = HashSet::new();

        {
            // borrow `self.swarm` immutably only inside this block
            let behaviour = self.swarm.behaviour();
            for peer_id in self.swarm.connected_peers().cloned() {
                if let Some(score) = behaviour.gossipsub.peer_score(&peer_id) {
                    if score < GRAYLIST_THRESHOLD {
                        peers_to_block_and_disconnect.insert(peer_id);
                    }
                    peer_scores.push((peer_id, score));
                }
            }
        }

        // ---------- second pass (mutable) ----------
        let target = self.swarm.behaviour().peer_manager.target_peers();
        let excess = self.swarm.connected_peers().count().saturating_sub(target);

        for peer_id in &peers_to_block_and_disconnect {
            self.swarm.behaviour_mut().peer_manager.block_peer(*peer_id);
            self.disconnect_peer(peer_id, "Blocking peer due to low score");
        }

        if excess > 0 {
            peer_scores.sort_by(|a, b| a.1.total_cmp(&b.1));
            let to_disconnect = peer_scores
                .iter()
                .filter(|(p, _)| !peers_to_block_and_disconnect.contains(p))
                .take(excess)
                .map(|(p, _)| *p);

            for peer_id in to_disconnect {
                self.disconnect_peer(&peer_id, "Pruning excess peers by score");
            }
        }
    }

    fn dial(&mut self, opts: DialOpts) {
        let peer_id = opts.get_peer_id();
        if let Err(err) = self.swarm.dial(opts) {
            // Differentiate between expected and unexpected dial failures
            //
            // PeerCondition::NotDialing causes DialPeerConditionFalse when we try to dial
            // a peer we're already connected to or dialing. This is expected and benign,
            // so we log at TRACE level to reduce noise.
            //
            // Other errors (unreachable addresses, transport failures, etc.) are logged
            // at DEBUG level since they indicate actual problems.
            match &err {
                libp2p::swarm::DialError::DialPeerConditionFalse(_) => {
                    trace!(%err, "Dial skipped due to peer condition");
                }
                _ => {
                    debug!(%err, ?peer_id, "Failed to dial peer");
                }
            }
        }
    }

    fn disconnect_peer(&mut self, peer_id: &PeerId, reason: &str) {
        match self.swarm.disconnect_peer_id(*peer_id) {
            Ok(_) => debug!(%peer_id, reason = %reason, "Disconnected peer"),
            Err(_) => trace!(%peer_id, "Peer was already disconnected"),
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
        .with_dial_concurrency_factor(dial_concurrency_factor)
        // Set a non-zero idle connection timeout to allow time for handshake completion
        //
        // libp2p needs time to complete the SSV handshake protocol after connection
        // establishment. 30 seconds provides sufficient time for this flow while still
        // cleaning up truly idle connections. This follows guidance from rust-libp2p
        // maintainers to always set a non-zero idle timeout.
        .with_idle_connection_timeout(Duration::from_secs(30));

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
