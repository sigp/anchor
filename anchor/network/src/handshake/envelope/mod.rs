mod codec;
mod generated;

use crate::handshake::node_info::NodeInfo;
use discv5::libp2p_identity::PublicKey;
use libp2p::identity::DecodingError;
use quick_protobuf::{BytesReader, Error as ProtoError, MessageRead, MessageWrite, Writer};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Coding error: {0}")]
    Coding(#[from] ProtoError), // Automatically implements `From<ProtoError> for Error`

    #[error("Public Key Decoding error: {0}")]
    PublicKeyDecoding(#[from] DecodingError),

    #[error("Signature Verification error: {0}")]
    SignatureVerification(String),
}

impl Envelope {
    /// Encode the Envelope to a Protobuf byte array (like `proto.Marshal` in Go).
    pub fn encode_to_vec(&self) -> Result<Vec<u8>, Error> {
        let mut buf = Vec::new();
        let mut writer = Writer::new(&mut buf);
        self.write_message(&mut writer)?;
        Ok(buf)
    }

    /// Decode an Envelope from a Protobuf byte array (like `proto.Unmarshal` in Go).
    pub fn decode_from_slice(data: &[u8]) -> Result<Self, Error> {
        let mut reader = BytesReader::from_bytes(data);
        let env = Envelope::from_reader(&mut reader, data).map_err(Error::Coding)?;
        Ok(env)
    }
}

/// Decodes an Envelope and verify signature.
pub fn parse_envelope(bytes: &[u8]) -> Result<Envelope, Error> {
    let env = Envelope::decode_from_slice(bytes)?;

    let domain = NodeInfo::DOMAIN;
    let payload_type = NodeInfo::CODEC;

    let unsigned = make_unsigned(domain.as_bytes(), payload_type, &env.payload);

    let pk = PublicKey::try_decode_protobuf(&env.public_key.to_vec())?;

    if !pk.verify(&unsigned?, &env.signature) {
        return Err(SignatureVerification(
            "signature verification failed".into(),
        ));
    }

    Ok(env)
}

pub fn make_unsigned(
    domain: &[u8],
    payload_type: &[u8],
    payload: &[u8],
) -> Result<Vec<u8>, ProtoError> {
    let mut buf = Vec::new();
    {
        let mut writer = Writer::new(&mut buf);
        writer.write_bytes(domain)?;
        writer.write_bytes(payload_type)?;
        writer.write_bytes(payload)?;
    }
    Ok(buf)
}

use crate::handshake::envelope::Error::SignatureVerification;
pub use codec::Codec;
pub use generated::message::pb::Envelope;
