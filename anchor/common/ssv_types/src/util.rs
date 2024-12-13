use base64::prelude::*;
use openssl::pkey::Public;
use openssl::rsa::Rsa;

// Parse from a RSA public key string into the associated RSA representation
pub fn parse_rsa(pem_data: &str) -> Result<Rsa<Public>, String> {
    // First decode the base64 data
    let pem_decoded = BASE64_STANDARD
        .decode(pem_data)
        .map_err(|e| format!("Unable to decode base64 pem data: {}", e))?;

    // Convert the decoded data to a string
    let mut pem_string = String::from_utf8(pem_decoded)
        .map_err(|e| format!("Unable to convert decoded pem data into a string: {}", e))?;

    // Fix the header - replace PKCS1 header with PKCS8 header
    pem_string = pem_string
        .replace(
            "-----BEGIN RSA PUBLIC KEY-----",
            "-----BEGIN PUBLIC KEY-----",
        )
        .replace("-----END RSA PUBLIC KEY-----", "-----END PUBLIC KEY-----");

    // Parse the PEM string into an RSA public key using PKCS8 format
    let rsa_pubkey = Rsa::public_key_from_pem(pem_string.as_bytes())
        .map_err(|e| format!("Failed to parse RSA public key: {}", e))?;

    Ok(rsa_pubkey)
}
