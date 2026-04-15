use std::str::FromStr;

use bls::PublicKeyBytes;
use serde::Deserialize;
use types::{ChainSpec, DepositMessage, Domain, Hash256, SignedRoot};

use crate::{SpecTest, utils::deserializers::deserialize_hex_hash256};

/// Mirrors Go's `GenerateETHDepositData()`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct BeaconDepositDataSpecTest {
    #[serde(rename = "ValidatorPK")]
    validator_pk: String,
    withdrawal_credentials: String,
    fork_version: String,
    #[serde(deserialize_with = "deserialize_hex_hash256")]
    expected_signing_root: Hash256,
}

impl SpecTest for BeaconDepositDataSpecTest {
    fn run(&self) -> Result<(), String> {
        // Fixture has no 0x prefix.
        let pubkey = PublicKeyBytes::from_str(&format!("0x{}", self.validator_pk))
            .map_err(|e| format!("Failed to parse ValidatorPK: {e}"))?;

        let wc_bytes = hex::decode(&self.withdrawal_credentials)
            .map_err(|e| format!("Failed to decode WithdrawalCredentials: {e}"))?;
        if wc_bytes.len() != 32 {
            return Err(format!(
                "WithdrawalCredentials must be 32 bytes, got {}",
                wc_bytes.len()
            ));
        }
        let withdrawal_credentials = Hash256::from_slice(&wc_bytes);

        let fv_bytes = hex::decode(&self.fork_version)
            .map_err(|e| format!("Failed to decode ForkVersion: {e}"))?;
        let fork_version: [u8; 4] = fv_bytes
            .try_into()
            .map_err(|_| "ForkVersion must be 4 bytes".to_string())?;

        let spec = ChainSpec::mainnet();
        let deposit_msg = DepositMessage {
            pubkey,
            withdrawal_credentials,
            amount: spec.max_effective_balance,
        };
        let domain = spec.compute_domain(Domain::Deposit, fork_version, Hash256::ZERO);
        let signing_root = deposit_msg.signing_root(domain);

        if signing_root != self.expected_signing_root {
            return Err(format!(
                "Signing root mismatch: got {}, expected {}",
                hex::encode(signing_root.as_slice()),
                hex::encode(self.expected_signing_root.as_slice()),
            ));
        }
        Ok(())
    }
}
