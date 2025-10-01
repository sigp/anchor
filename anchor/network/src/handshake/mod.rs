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

pub type Behaviour = RequestResponseBehaviour<Codec>;
pub type Event = <Behaviour as NetworkBehaviour>::ToSwarm;

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

/// Create a libp2p Behaviour to handle handshake requests. Events emitted from this event must be
/// fed into [`handle_event`].
pub fn create_behaviour(keypair: Keypair) -> Behaviour {
    let protocol = StreamProtocol::new("/ssv/info/0.0.1");
    Behaviour::with_codec(
        Codec::new(keypair),
        [(protocol, ProtocolSupport::Full)],
        Config::default(),
    )
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
    let _ = behaviour.send_response(channel, our_node_info.clone());

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

/// Initiate a handshake with a peer by sending our NodeInfo.
///
/// This is the active/outbound side of the handshake protocol:
/// 1. We send our NodeInfo to the peer via the /ssv/info/0.0.1 protocol
/// 2. The peer responds with their NodeInfo
/// 3. We verify their NodeInfo is compatible (handled by handle_response)
///
/// # When to call this
///
/// This should ONLY be called after:
/// 1. We established an outbound connection (we dialed them)
/// 2. The Identify protocol completed successfully
/// 3. Identify confirmed the peer supports /ssv/info/0.0.1
///
/// Calling this too early (e.g., immediately on ConnectionEstablished) can result in
/// OutboundFailure::ConnectionClosed if the connection is closed due to concurrent
/// dial resolution or protocol negotiation.
///
/// The main event loop in network.rs handles the proper sequencing via the
/// pending_handshakes queue.
pub fn initiate(our_node_info: &NodeInfo, behaviour: &mut Behaviour, peer_id: PeerId) {
    trace!(?peer_id, "initiating handshake");
    behaviour.send_request(&peer_id, our_node_info.clone());
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

    #[tokio::test]
    async fn handshake_success() {
        *TRACING;

        let local_key = Keypair::generate_ed25519();
        let remote_key = Keypair::generate_ed25519();

        let mut local_swarm = Swarm::new_ephemeral_tokio(|_| create_behaviour(local_key));
        let local_node_info = node_info("test", "local");
        let mut remote_swarm = Swarm::new_ephemeral_tokio(|_| create_behaviour(remote_key));
        let remote_node_info = node_info("test", "remote");

        tokio::spawn(async move {
            local_swarm.listen().with_memory_addr_external().await;

            remote_swarm.connect(&mut local_swarm).await;

            initiate(
                &remote_node_info,
                remote_swarm.behaviour_mut(),
                *local_swarm.local_peer_id(),
            );

            let mut local_completed = false;
            let mut remote_completed = false;

            while !local_completed && !remote_completed {
                select!(
                    SwarmEvent::Behaviour(e) = local_swarm.next_swarm_event() => {
                        let Some(result) =
                            handle_event(&local_node_info, local_swarm.behaviour_mut(), e) else {
                            continue;
                        };
                        let Completed {
                            peer_id,
                            their_info,
                        } = result.expect("handshake to succeed");
                        assert_eq!(peer_id, *remote_swarm.local_peer_id());
                        assert_eq!(their_info.metadata.unwrap().node_version, "remote");
                        local_completed = true;
                    }
                    SwarmEvent::Behaviour(e) = remote_swarm.next_swarm_event() => {
                        let Some(result) =
                            handle_event(&remote_node_info, remote_swarm.behaviour_mut(), e) else {
                            continue;
                        };
                        let Completed {
                            peer_id,
                            their_info,
                        } = result.expect("handshake to succeed");
                        assert_eq!(peer_id, *local_swarm.local_peer_id());
                        assert_eq!(their_info.metadata.unwrap().node_version, "local");
                        remote_completed = true;
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

        let mut local_swarm = Swarm::new_ephemeral_tokio(|_| create_behaviour(local_key));
        let local_node_info = node_info("test1", "local");
        let mut remote_swarm = Swarm::new_ephemeral_tokio(|_| create_behaviour(remote_key));
        let remote_node_info = node_info("test2", "remote");

        tokio::spawn(async move {
            local_swarm.listen().with_memory_addr_external().await;

            remote_swarm.connect(&mut local_swarm).await;

            initiate(
                &remote_node_info,
                remote_swarm.behaviour_mut(),
                *local_swarm.local_peer_id(),
            );

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
