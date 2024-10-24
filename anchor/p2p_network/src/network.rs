use crate::behaviour::AnchorBehaviour;
use crate::transport::build_transport;
use libp2p::core::muxing::StreamMuxerBox;
use libp2p::core::transport::Boxed;
use libp2p::identity::{secp256k1, Keypair};
use libp2p::{futures, identify, ping, PeerId, Swarm, SwarmBuilder};
use std::num::{NonZeroU8, NonZeroUsize};
use std::pin::Pin;
use task_executor::TaskExecutor;

pub struct Network {
    #[allow(dead_code)]
    pub swarm: Swarm<AnchorBehaviour>,
}

#[derive(Debug)]
pub enum NetworkError {}

impl Network {
    async fn new(executor: TaskExecutor) -> Result<Self, NetworkError> {
        // TODO: generate / load local key
        let secp256k1_kp: secp256k1::Keypair = secp256k1::SecretKey::generate().into();
        let local_keypair: Keypair = secp256k1_kp.into();

        let transport = build_transport(local_keypair.clone());
        let behaviour = build_anchor_behaviour(local_keypair.clone());
        let network = Network {
            swarm: build_swarm(executor, local_keypair, transport, behaviour),
        };
        network.start().await?;

        Ok(network)
    }

    async fn start(&self) -> Result<(), NetworkError> {
        /*
        - Set up listening address
        - Dial peers
        - Subscribe gossip topics
         */
        Ok(())
    }

    // implement `poll_network` from `self.swarm`.
}

fn build_anchor_behaviour(local_keypair: Keypair) -> AnchorBehaviour {
    // setup gossipsub
    // discv5
    // TODO: update protocol and agent versions.
    let identify = {
        let local_public_key = local_keypair.public();
        let identify_config = identify::Config::new("ssv/1.0.0".into(), local_public_key)
            .with_agent_version("0.0.1".to_string())
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
) -> Swarm<AnchorBehaviour> {
    // use the executor for libp2p
    struct Executor(task_executor::TaskExecutor);
    impl libp2p::swarm::Executor for Executor {
        fn exec(&self, f: Pin<Box<dyn futures::Future<Output = ()> + Send>>) {
            self.0.spawn(f, "libp2p");
        }
    }

    let swarm_config = libp2p::swarm::Config::with_executor(Executor(executor))
        .with_notify_handler_buffer_size(NonZeroUsize::new(7).expect("Not zero"))
        .with_per_connection_event_buffer_size(4)
        .with_dial_concurrency_factor(NonZeroU8::new(1).unwrap());

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
    use task_executor::TaskExecutor;

    #[tokio::test]
    async fn create_network() {
        let handle = tokio::runtime::Handle::current();
        let (_signal, exit) = async_channel::bounded(1);
        let (shutdown_tx, _) = futures::channel::mpsc::channel(1);
        let task_executor = TaskExecutor::new(handle, exit, shutdown_tx);
        let _network = Network::new(task_executor)
            .await
            .expect("network should start");
    }
}
