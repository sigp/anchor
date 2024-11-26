
use alloy::primitives::Bytes;

// use types::publicKey
pub struct SharePublickKey([u8; 48]);
pub struct SharePrivateKey([u8; 32]);


// [signature | public keys | encrypted keys]
pub struct RawShares {
    // Uncompressed bls signatures
    signature: [u8; 96],
    // Public keys of the Shares
    public_keys: Vec<SharePublickKey>,
    // Split encrypted private keys
    private_keys: Vec<SharePrivateKey>
}

// Convert from a raw stream of bytes to a structured set of shares
// for a validator
impl TryFrom<Bytes> for RawShares {
    type Error = String;
    fn try_from(source: Bytes) -> Result<RawShares, Self::Error> {
        todo!()
    }
}


pub fn verify_signature() -> Result<(), String>{
    todo!()
}
