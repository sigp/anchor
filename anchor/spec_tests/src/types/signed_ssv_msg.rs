use openssl::{hash::MessageDigest, pkey::PKey, sign::Verifier};
use serde::Deserialize;
use ssv_types::{
    OperatorId,
    message::{SSVMessage, SignedSSVMessage},
};
use ssz::Encode;

use crate::{
    SpecTest,
    utils::{deserializers::deserialize_base64_list, dtos::RawSSVMessage, error_codes},
};

/// Fixture container for a single signed message in the `Messages` array.
/// Holds raw test data that gets assembled with error code mapping.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SignedSSVMessageFixture {
    #[serde(deserialize_with = "deserialize_base64_list")]
    signatures: Vec<Vec<u8>>,
    #[serde(rename = "OperatorIDs")]
    operator_ids: Vec<u64>,
    #[serde(rename = "SSVMessage")]
    ssv_message: Option<RawSSVMessage>,
    #[serde(
        rename = "FullData",
        deserialize_with = "crate::utils::deserializers::deserialize_hex_option",
        default
    )]
    full_data: Option<Vec<u8>>,
}

/// Top-level test fixture for `SignedSSVMessageTest`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SignedSSVMessageTest {
    messages: Vec<SignedSSVMessageFixture>,
    expected_error_code: i64,
    #[serde(rename = "RSAPublicKey", default)]
    rsa_public_key: Option<Vec<String>>,
}

impl SpecTest for SignedSSVMessageTest {
    fn run(&self) -> Result<(), String> {
        for msg in &self.messages {
            let actual_code = match self.validate_message(msg) {
                Ok(()) => error_codes::NO_ERROR,
                Err(code) => code,
            };
            error_codes::assert_error_code(self.expected_error_code, actual_code)?;
        }
        Ok(())
    }
}

impl SignedSSVMessageTest {
    /// Validate via `SignedSSVMessage::new()` (which calls `validate()` internally).
    fn validate_message(&self, msg: &SignedSSVMessageFixture) -> Result<(), i64> {
        let ssv_message = msg
            .ssv_message
            .as_ref()
            .ok_or(error_codes::NIL_SSV_MESSAGE)?;

        let ssv_message: SSVMessage = ssv_message
            .try_into()
            .map_err(|_: String| error_codes::UNMAPPED_ERROR_CODE)?;

        let signatures = Self::prepare_signatures(&msg.signatures)?;
        let operator_ids: Vec<OperatorId> =
            msg.operator_ids.iter().copied().map(OperatorId).collect();

        let signed_msg = SignedSSVMessage::new(
            signatures,
            operator_ids,
            ssv_message.clone(),
            msg.full_data.clone().unwrap_or_default(),
        )
        .map_err(|e| error_codes::signed_ssv_message_error_code(&e))?;

        self.verify_rsa_signatures(&signed_msg, &ssv_message)
    }

    /// Normalize signatures to `[u8; 256]`: reject empty/oversized, zero-pad short ones.
    fn prepare_signatures(signatures: &[Vec<u8>]) -> Result<Vec<[u8; 256]>, i64> {
        signatures
            .iter()
            .map(|sig| {
                if sig.is_empty() {
                    return Err(error_codes::EMPTY_SIGNATURE);
                }
                if sig.len() > 256 {
                    return Err(error_codes::SSV_MESSAGE_HAS_INVALID_SIGNATURE);
                }
                let mut arr = [0u8; 256];
                arr[..sig.len()].copy_from_slice(sig);
                Ok(arr)
            })
            .collect()
    }

    /// Verify RSA signatures against the SSZ-encoded `SSVMessage`.
    fn verify_rsa_signatures(
        &self,
        signed_msg: &SignedSSVMessage,
        ssv_message: &SSVMessage,
    ) -> Result<(), i64> {
        let Some(ref pk_strings) = self.rsa_public_key else {
            return Ok(());
        };

        let ssz_bytes = ssv_message.as_ssz_bytes();
        let signatures = signed_msg.signatures();

        for (i, pk_b64) in pk_strings.iter().enumerate() {
            let sig: &[u8] = signatures
                .get(i)
                .ok_or(error_codes::SSV_MESSAGE_HAS_INVALID_SIGNATURE)?;

            Self::verify_single_rsa(sig, &ssz_bytes, pk_b64)
                .map_err(|_| error_codes::SSV_MESSAGE_HAS_INVALID_SIGNATURE)?;
        }

        Ok(())
    }

    /// Verify a single RSA-PKCS1v15-SHA256 signature.
    fn verify_single_rsa(
        sig: &[u8],
        msg: &[u8],
        pk_b64: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let rsa_key = operator_key::public::from_base64(pk_b64.as_bytes())?;
        let pkey = PKey::from_rsa(rsa_key)?;
        let mut verifier = Verifier::new(MessageDigest::sha256(), &pkey)?;
        verifier.update(msg)?;
        if !verifier.verify(sig)? {
            return Err("RSA signature verification failed".into());
        }
        Ok(())
    }
}
