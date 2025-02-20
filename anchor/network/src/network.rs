use std::collections::HashMap;
use std::num::{NonZeroU8, NonZeroUsize};
use std::pin::Pin;
use std::time::{Duration, Instant};

use futures::StreamExt;
use libp2p::core::muxing::StreamMuxerBox;
use libp2p::core::transport::Boxed;
use libp2p::core::ConnectedPoint;
use libp2p::gossipsub::{ConfigBuilderError, IdentTopic, MessageAuthenticity, ValidationMode};
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::SwarmEvent;
use libp2p::{
    futures, gossipsub, identify, ping, Multiaddr, PeerId, Swarm, SwarmBuilder, TransportError,
};
use lighthouse_network::discovery::DiscoveredPeers;
use lighthouse_network::discv5::enr::k256::sha2::{Digest, Sha256};
use ssv_types::message::SignedSSVMessage;
use ssz::Decode;
use subnet_tracker::{SubnetEvent, SubnetId};
use task_executor::TaskExecutor;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

use crate::behaviour::AnchorBehaviour;
use crate::behaviour::AnchorBehaviourEvent;
use crate::discovery::{Discovery, DiscoveryError, FIND_NODE_QUERY_CLOSEST_PEERS};
use crate::handshake::node_info::{NodeInfo, NodeMetadata};
use crate::keypair_utils::load_private_key;
use crate::peer_manager::{PeerManager, SubnetConnectActions};
use crate::transport::build_transport;
use crate::{handshake, Config, Enr};

use crate::network::NetworkError::{Gossipsub, SwarmConfig};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("Unable to listen on address {address}: {source}")]
    Listen {
        address: Multiaddr,
        #[source]
        source: TransportError<std::io::Error>,
    },

    #[error("Gossipsub config error: {0}")]
    GossipsubConfig(#[from] ConfigBuilderError),

    #[error("Gossipsub error: {0}")]
    Gossipsub(String),

    #[error("Discovery error: {0}")]
    Discovery(#[from] DiscoveryError),

    #[error("Swarm config error: {0}")]
    SwarmConfig(String),
}

pub struct Network {
    swarm: Swarm<AnchorBehaviour>,
    subnet_event_receiver: mpsc::Receiver<SubnetEvent>,
    peer_id: PeerId,
    node_info: NodeInfo,
}

impl Network {
    // Creates an instance of the Network struct to start sending and receiving information on the
    // p2p network.
    pub async fn try_new(
        config: &Config,
        subnet_event_receiver: mpsc::Receiver<SubnetEvent>,
        executor: TaskExecutor,
    ) -> Result<Network, NetworkError> {
        let local_keypair: Keypair = load_private_key(&config.network_dir);

        let transport = build_transport(local_keypair.clone(), !config.disable_quic_support);

        let behaviour = build_anchor_behaviour(local_keypair.clone(), config).await?;

        let peer_id = local_keypair.public().to_peer_id();
        let domain_type: String = config.domain_type.clone().into();
        let node_info = NodeInfo::new(
            domain_type,
            Some(NodeMetadata {
                node_version: "1.0.0".to_string(),
                execution_node: "geth/v1.10.8".to_string(),
                consensus_node: "lighthouse/v1.5.0".to_string(),
                subnets: "ffffffffffffffffffffffffffffffff".to_string(),
            }),
        );

        let mut network = Network {
            swarm: build_swarm(
                executor.clone(),
                local_keypair,
                transport,
                behaviour,
                config,
            )?,
            subnet_event_receiver,
            peer_id,
            node_info,
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
        /*
        TODO
        - Dial peers
        - Subscribe gossip topics
         */

        // TODO: Return channels for input/output
        Ok(network)
    }

    /// Main loop for polling and handling swarm and channels.
    pub async fn run(mut self) {
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
                                        debug!(
                                            source = ?propagation_source,
                                            id = ?message_id,
                                            "Received SignedSSVMessage"
                                        );
                                        match SignedSSVMessage::from_ssz_bytes(&message.data) {
                                            Ok(deserialized_message) => {
                                                debug!(msg = ?deserialized_message, "SignedSSVMessage deserialized");
                                            }
                                            Err(e) => {
                                                error!("error" = ?e, "Failed to deserialize SignedSSVMessage");
                                            }
                                        }
                                    }
                                    // TODO handle gossipsub events
                                    _ => {
                                        debug!(event = ?ge, "Unhandled gossipsub event");
                                    }
                                }
                                // TODO handle gossipsub events
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
                            // TODO handle other behaviour events
                            _ => {
                                debug!(event = ?behaviour_event, "Unhandled behaviour event");
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
                        // TODO handle other swarm events
                        _ => {
                            debug!(event = ?swarm_message, "Unhandled swarm event");
                        }
                    }
                },
                event = self.subnet_event_receiver.recv() => {
                    match event {
                        Some(event) => self.on_subnet_tracker_event(event),
                        None => {
                            error!("subnet tracker has quit");
                            return;
                        }
                    }
                }
                // TODO match input channels
            }
        }
    }

    fn on_discovered_peers(&mut self, peers: HashMap<Enr, Option<Instant>>) {
        debug!(peers =  ?peers, "Peers discovered");
        let manager = self.peer_manager();
        // need to collect to avoid double borrow
        let to_dial = peers
            .into_iter()
            .filter_map(|(enr, _)| manager.discovered_peer(enr))
            .collect::<Vec<_>>();
        for dial in to_dial {
            let _ = self.swarm.dial(dial);
        }
    }

    fn on_subnet_tracker_event(&mut self, event: SubnetEvent) {
        match event {
            SubnetEvent::Join(subnet) => {
                if let Err(err) = self
                    .swarm
                    .behaviour_mut()
                    .gossipsub
                    .subscribe(&subnet_to_topic(subnet))
                {
                    error!(?err, subnet = *subnet, "can't subscribe");
                }
                let SubnetConnectActions { dial, discover } =
                    self.peer_manager().join_subnet(subnet);
                for peer in dial {
                    let _ = self.swarm.dial(peer);
                }
                if discover {
                    self.swarm
                        .behaviour_mut()
                        .discovery
                        .start_subnet_query(vec![subnet]);
                }
            }
            SubnetEvent::Leave(subnet) => {
                self.swarm
                    .behaviour_mut()
                    .gossipsub
                    .unsubscribe(&subnet_to_topic(subnet));
            }
        }
    }

    fn peer_manager(&mut self) -> &mut PeerManager {
        &mut self.swarm.behaviour_mut().peer_manager
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
}

fn subnet_to_topic(subnet: SubnetId) -> IdentTopic {
    IdentTopic::new(format!("ssv.v2.{}", *subnet))
}

async fn build_anchor_behaviour(
    local_keypair: Keypair,
    network_config: &Config,
) -> Result<AnchorBehaviour, NetworkError> {
    let identify = {
        let local_public_key = local_keypair.public();
        let identify_config = identify::Config::new("anchor".into(), local_public_key)
            .with_agent_version(version::version_with_platform())
            .with_cache_size(0);
        identify::Behaviour::new(identify_config)
    };

    // TODO those values might need to be parameterized based on the network
    let slots_per_epoch = 32;
    let seconds_per_slot = 12;
    let duplicate_cache_time = Duration::from_secs(slots_per_epoch * seconds_per_slot); // 6.4 min

    let gossip_message_id = move |message: &gossipsub::Message| {
        gossipsub::MessageId::from(&Sha256::digest(&message.data)[..20])
    };

    // TODO Add Topic Message Validator and Subscription Filter
    let config = gossipsub::ConfigBuilder::default()
        .duplicate_cache_time(duplicate_cache_time)
        .message_id_fn(gossip_message_id)
        .flood_publish(false)
        .validation_mode(ValidationMode::Permissive)
        .mesh_n(8) //D
        .mesh_n_low(6) // Dlo
        .mesh_n_high(12) // Dhi
        .mesh_outbound_min(4) // Dout
        .heartbeat_interval(Duration::from_millis(700))
        .history_length(6)
        .history_gossip(4)
        .max_ihave_length(1500)
        .max_ihave_messages(32)
        .build()?;

    let gossipsub =
        gossipsub::Behaviour::new(MessageAuthenticity::Signed(local_keypair.clone()), config)
            .map_err(|e| Gossipsub(e.to_string()))?;

    let discovery = {
        // Build and start the discovery sub-behaviour
        let mut discovery = Discovery::new(local_keypair.clone(), network_config).await?;
        // start searching for peers
        discovery.discover_peers(FIND_NODE_QUERY_CLOSEST_PEERS);
        discovery
    };

    let peer_manager = PeerManager::new(network_config);

    let handshake = handshake::create_behaviour(local_keypair);

    Ok(AnchorBehaviour {
        identify,
        ping: ping::Behaviour::default(),
        gossipsub,
        discovery,
        peer_manager,
        handshake,
    })
}

fn build_swarm(
    executor: TaskExecutor,
    local_keypair: Keypair,
    transport: Boxed<(PeerId, StreamMuxerBox)>,
    behaviour: AnchorBehaviour,
    _config: &Config,
) -> Result<Swarm<AnchorBehaviour>, NetworkError> {
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

    // TODO Add metrics later
    let swarm = SwarmBuilder::with_existing_identity(local_keypair)
        .with_tokio()
        .with_other_transport(|_key| transport)
        .expect("infallible") // This operation can't fail because the error type is Infallible.
        .with_behaviour(|_| behaviour)
        .expect("infallible") // Again, this can't fail.
        .with_swarm_config(|_| swarm_config)
        .build();

    Ok(swarm)
}

#[cfg(test)]
mod test {
    use crate::network::Network;
    use crate::Config;
    use std::time::Duration;
    use subnet_tracker::test_tracker;
    use task_executor::TaskExecutor;

    #[tokio::test]
    async fn create_network() {
        let handle = tokio::runtime::Handle::current();
        let (_signal, exit) = async_channel::bounded(1);
        let (shutdown_tx, _) = futures::channel::mpsc::channel(1);
        let task_executor = TaskExecutor::new(handle, exit, shutdown_tx);
        let subnet_tracker = test_tracker(task_executor.clone(), vec![], Duration::ZERO);
        assert!(
            Network::try_new(&Config::default(), subnet_tracker, task_executor)
                .await
                .is_ok()
        );
    }
}
