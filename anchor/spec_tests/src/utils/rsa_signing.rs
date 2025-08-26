use openssl::{
    hash::MessageDigest,
    pkey::{PKey, Private},
    rsa::Rsa,
    sign::Signer,
};
use ssv_types::{OperatorId, consensus::UnsignedSSVMessage, message::SignedSSVMessage};
use ssz::Encode;

/// Sign a message with RSA key using SHA256
pub fn sign_message_with_rsa(
    message_bytes: &[u8],
    rsa_key: &Rsa<Private>,
) -> Result<[u8; 256], String> {
    // Convert RSA key to PKey for signing
    let pkey = PKey::from_rsa(rsa_key.clone())
        .map_err(|e| format!("Failed to convert RSA to PKey: {:?}", e))?;

    // Create a signer with SHA256 and PKCS1 padding (matching Go's approach)
    let mut signer = Signer::new(MessageDigest::sha256(), &pkey)
        .map_err(|e| format!("Failed to create signer: {:?}", e))?;

    // Set PKCS1 padding explicitly to match Go's rsa.SignPKCS1v15
    signer
        .set_rsa_padding(openssl::rsa::Padding::PKCS1)
        .map_err(|e| format!("Failed to set RSA padding: {:?}", e))?;

    // Sign the message - this should be deterministic
    signer
        .update(message_bytes)
        .map_err(|e| format!("Failed to update signer: {:?}", e))?;

    let signature = signer
        .sign_to_vec()
        .map_err(|e| format!("Failed to sign message: {:?}", e))?;

    if signature.len() != 256 {
        return Err(format!("Signature length {} != 256", signature.len()));
    }

    let mut sig_array = [0u8; 256];
    sig_array.copy_from_slice(&signature[..256]);

    Ok(sig_array)
}

/// Sign an SSZ-encodable message with RSA key
pub fn sign_ssz_message_with_rsa<T: Encode>(
    message: &T,
    rsa_key: &Rsa<Private>,
) -> Result<[u8; 256], String> {
    sign_message_with_rsa(&message.as_ssz_bytes(), rsa_key)
}

/// Sign an UnsignedSSVMessage with full data to create a SignedSSVMessage
pub fn sign_message_with_full_data(
    unsigned: UnsignedSSVMessage,
    full_data: Vec<u8>,
    rsa_key: &Rsa<Private>,
    op_id: &OperatorId,
) -> SignedSSVMessage {
    let signature =
        sign_ssz_message_with_rsa(&unsigned.ssv_message, rsa_key).expect("Failed to sign message");

    SignedSSVMessage::new(
        vec![signature],
        vec![*op_id],
        unsigned.ssv_message,
        full_data,
    )
    .expect("Failed to create signed message")
}
