mod generated {
    pub mod message;
}

use discv5::libp2p_identity::PublicKey;
pub use generated::message::Envelope;
use libp2p::identity::DecodingError;
use prost::{DecodeError, EncodeError, Message};
use thiserror::Error;

use crate::handshake::{
    envelope::Error::{InvalidPayloadType, SignatureVerification},
    node_info::NodeInfo,
};

#[derive(Debug, Error)]
pub enum Error {
    #[error("Decoding error: {0}")]
    Decoding(#[from] DecodeError),

    #[error("Encoding error: {0}")]
    Encoding(#[from] EncodeError),

    #[error("Public Key Decoding error: {0}")]
    PublicKeyDecoding(#[from] DecodingError),

    #[error("Signature Verification error: {0}")]
    SignatureVerification(String),

    #[error("Invalid payload type: {0:?}")]
    InvalidPayloadType(Vec<u8>),
}

impl Envelope {
    /// Decodes an Envelope and verify signature.
    pub fn parse_and_verify(bytes: &[u8]) -> Result<Envelope, Error> {
        let env = Envelope::decode(bytes)?;

        let domain = NodeInfo::DOMAIN;
        let payload_type = NodeInfo::CODEC;

        if env.payload_type != payload_type {
            return Err(InvalidPayloadType(env.payload_type));
        }

        let unsigned = make_unsigned(domain.as_bytes(), payload_type, &env.payload);

        let pk = PublicKey::try_decode_protobuf(&env.public_key.to_vec())?;

        if !pk.verify(&unsigned?, &env.signature) {
            return Err(SignatureVerification(
                "signature verification failed".into(),
            ));
        }

        Ok(env)
    }
}

pub fn make_unsigned(
    domain: &[u8],
    payload_type: &[u8],
    payload: &[u8],
) -> Result<Vec<u8>, EncodeError> {
    let mut buf = Vec::new();
    prost::encode_length_delimiter(domain.len(), &mut buf)?;
    buf.extend_from_slice(domain);
    prost::encode_length_delimiter(payload_type.len(), &mut buf)?;
    buf.extend_from_slice(payload_type);
    prost::encode_length_delimiter(payload.len(), &mut buf)?;
    buf.extend_from_slice(payload);
    Ok(buf)
}
