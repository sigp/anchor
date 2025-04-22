use std::time::Duration;

use futures::future::Either;
use libp2p::{
    PeerId, Transport,
    core::{muxing::StreamMuxerBox, transport::Boxed},
    identity::Keypair,
    noise, quic, tcp, yamux,
};

use crate::network::NetworkError;

/// The implementation supports TCP/IP, QUIC over UDP, noise as the encryption layer, and
/// yamux as the multiplexing layer (when using TCP).
pub(crate) fn build_transport(
    local_private_key: Keypair,
    quic_support: bool,
) -> Result<Boxed<(PeerId, StreamMuxerBox)>, NetworkError> {
    let yamux_config = yamux::Config::default();

    let tcp = tcp::tokio::Transport::new(tcp::Config::default().nodelay(true))
        .upgrade(libp2p::core::upgrade::Version::V1)
        .authenticate(generate_noise_config(&local_private_key))
        .multiplex(yamux_config)
        .timeout(Duration::from_secs(10));

    let transport = if quic_support {
        let quic_config = quic::Config::new(&local_private_key);
        let quic = quic::tokio::Transport::new(quic_config);
        let transport = tcp
            .or_transport(quic)
            .map(|either_output, _| match either_output {
                Either::Left((peer_id, muxer)) => (peer_id, StreamMuxerBox::new(muxer)),
                Either::Right((peer_id, muxer)) => (peer_id, StreamMuxerBox::new(muxer)),
            });
        transport.boxed()
    } else {
        tcp.boxed()
    };

    libp2p::dns::tokio::Transport::system(transport)
        .map_err(NetworkError::DnsTransport)
        .map(|t| t.boxed())
}

/// Generate authenticated XX Noise config from identity keys
fn generate_noise_config(identity_keypair: &Keypair) -> noise::Config {
    noise::Config::new(identity_keypair).expect("signing can fail only once during starting a node")
}
