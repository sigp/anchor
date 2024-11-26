use alloy::primitives::Bytes;

// Offsets to parse the share bytes
const SIG_LEN: usize = 96;
const PUBKEY_LEN: usize = 48;
const ENCRYPTEDKEY_LEN: usize = 32;

// use types::(PublicKey, PrivateKey),
pub struct SharePublickKey([u8; 48]);
pub struct SharePrivateKey([u8; 32]);

// All of the public keys and encrypted private keys for a
// validator key that has been broken into N shares.
pub struct ShareKeys {
    // Uncompressed bls signatures
    signature: [u8; 96],
    // Public keys of the Shares
    public_keys: Vec<SharePublickKey>,
    // Encrypted private key of the shares
    encrypted_keys: Vec<SharePrivateKey>,
}

// Convert from a raw stream of bytes to a structured set of keys.
// Event contains a bytes stream of the form
// [signature | public keys | encrypted keys].
impl TryFrom<Bytes> for ShareKeys {
    type Error = String;
    fn try_from(source: Bytes) -> Result<ShareKeys, Self::Error> {
        todo!()
    }
}

// Verify that the signature over the share data is correct
pub fn verify_signature() -> Result<(), String> {
    todo!()
}
