use base64::prelude::*;
use openssl::{pkey::Public, rsa::Rsa};

// Parse from a RSA public key string into the associated RSA representation
pub fn parse_rsa(pem_data: &[u8]) -> Result<Rsa<Public>, String> {
    // First decode the base64 data
    let pem_decoded = BASE64_STANDARD
        .decode(pem_data)
        .map_err(|e| format!("Unable to decode base64 pem data: {e}"))?;

    // Parse the PEM string into an RSA public key using PKCS8 format
    let rsa_pubkey = Rsa::public_key_from_pem_pkcs1(&pem_decoded)
        .map_err(|e| format!("Failed to parse RSA public key: {e}"))?;

    Ok(rsa_pubkey)
}
