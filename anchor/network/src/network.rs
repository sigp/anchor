use crate::behaviour::AnchorBehaviour;
use crate::transport::build_transport;
use crate::Config;
use futures::{SinkExt, StreamExt};
use libp2p::core::muxing::StreamMuxerBox;
use libp2p::core::transport::Boxed;
use libp2p::identity::{secp256k1, Keypair};
use libp2p::multiaddr::Protocol;
use libp2p::{futures, identify, ping, PeerId, Swarm, SwarmBuilder};
use std::num::{NonZeroU8, NonZeroUsize};
use std::pin::Pin;
use task_executor::{ShutdownReason, TaskExecutor};
use tracing::{error, info};

pub struct Network {
    swarm: Swarm<AnchorBehaviour>,
    peer_id: PeerId,
}

impl Network {
    pub fn spawn(executor: TaskExecutor, config: &Config) {
        // TODO: generate / load local key
        let secp256k1_kp: secp256k1::Keypair = secp256k1::SecretKey::generate().into();
        let local_keypair: Keypair = secp256k1_kp.into();

        let transport = build_transport(local_keypair.clone(), !config.disable_quic_support);
        let behaviour = build_anchor_behaviour(local_keypair.clone());
        let peer_id = local_keypair.public().to_peer_id();

        let network = Network {
            swarm: build_swarm(
                executor.clone(),
                local_keypair,
                transport,
                behaviour,
                config,
            ),
            peer_id,
        };

        executor.spawn(network.start(config.clone(), executor.clone()), "network");
        // TODO: this function should return input & output channels
    }

    async fn start(mut self, config: Config, executor: TaskExecutor) {
        info!("Network starting");

        for listen_multiaddr in config.listen_addresses.libp2p_addresses() {
            // If QUIC is disabled, ignore listening on QUIC ports
            if config.disable_quic_support && listen_multiaddr.iter().any(|v| v == Protocol::QuicV1)
            {
                continue;
            }

            match self.swarm.listen_on(listen_multiaddr.clone()) {
                Ok(_) => {
                    let mut log_address = listen_multiaddr;
                    log_address.push(Protocol::P2p(self.peer_id));
                    info!(address = %log_address, "Listening established");
                }
                Err(err) => {
                    error!(
                        %listen_multiaddr,
                        error = ?err,
                        "Unable to listen on libp2p address"
                    );
                    let _ = executor
                        .shutdown_sender()
                        .send(ShutdownReason::Failure(
                            "Unable to listen on libp2p address",
                        ))
                        .await;
                    return;
                }
            };
        }
        /*
        TODO
        - Dial peers
        - Subscribe gossip topics
         */

        self.run().await;
    }

    /// Main loop for polling and handling swarm and channels.
    async fn run(mut self) {
        loop {
            tokio::select! {
                _swarm_message = self.swarm.select_next_some() => {
                    // TODO handle and match swarm messages
                }
                // TODO match input channels
            }
        }
    }
}

fn build_anchor_behaviour(local_keypair: Keypair) -> AnchorBehaviour {
    // setup gossipsub
    // discv5
    let identify = {
        let local_public_key = local_keypair.public();
        let identify_config = identify::Config::new("anchor".into(), local_public_key)
            .with_agent_version(version::version_with_platform())
            .with_cache_size(0);
        identify::Behaviour::new(identify_config)
    };

    AnchorBehaviour {
        identify,
        ping: ping::Behaviour::default(),
        // gossipsub: gossipsub::Behaviour::default(),
    }
}

fn build_swarm(
    executor: TaskExecutor,
    local_keypair: Keypair,
    transport: Boxed<(PeerId, StreamMuxerBox)>,
    behaviour: AnchorBehaviour,
    _config: &Config,
) -> Swarm<AnchorBehaviour> {
    // use the executor for libp2p
    struct Executor(task_executor::TaskExecutor);
    impl libp2p::swarm::Executor for Executor {
        fn exec(&self, f: Pin<Box<dyn futures::Future<Output = ()> + Send>>) {
            self.0.spawn(f, "libp2p");
        }
    }

    // TODO: revisit once peer manager is integrated
    // let connection_limits = {
    //     let limits = libp2p::connection_limits::ConnectionLimits::default()
    //         .with_max_pending_incoming(Some(5))
    //         .with_max_pending_outgoing(Some(16))
    //         .with_max_established_incoming(Some(
    //             (config.target_peers as f32
    //                 * (1.0 + PEER_EXCESS_FACTOR - MIN_OUTBOUND_ONLY_FACTOR))
    //                 .ceil() as u32,
    //         ))
    //         .with_max_established_outgoing(Some(
    //             (config.target_peers as f32 * (1.0 + PEER_EXCESS_FACTOR)).ceil() as u32,
    //         ))
    //         .with_max_established(Some(
    //             (config.target_peers as f32 * (1.0 + PEER_EXCESS_FACTOR + PRIORITY_PEER_EXCESS))
    //                 .ceil() as u32,
    //         ))
    //         .with_max_established_per_peer(Some(1));
    //
    //     libp2p::connection_limits::Behaviour::new(limits)
    // };

    let swarm_config = libp2p::swarm::Config::with_executor(Executor(executor))
        .with_notify_handler_buffer_size(NonZeroUsize::new(7).expect("Not zero"))
        .with_per_connection_event_buffer_size(4)
        .with_dial_concurrency_factor(NonZeroU8::new(1).unwrap());

    // TODO add metrics later
    SwarmBuilder::with_existing_identity(local_keypair)
        .with_tokio()
        .with_other_transport(|_key| transport)
        .expect("infalible")
        .with_behaviour(|_| behaviour)
        .expect("infalible")
        .with_swarm_config(|_| swarm_config)
        .build()
}

#[cfg(test)]
mod test {
    use crate::network::Network;
    use crate::Config;
    use task_executor::TaskExecutor;

    #[tokio::test]
    async fn create_network() {
        let handle = tokio::runtime::Handle::current();
        let (_signal, exit) = async_channel::bounded(1);
        let (shutdown_tx, _) = futures::channel::mpsc::channel(1);
        let task_executor = TaskExecutor::new(handle, exit, shutdown_tx);
        Network::spawn(task_executor, &Config::default());
    }
}
