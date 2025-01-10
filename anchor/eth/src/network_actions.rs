use crate::error::ExecutionError;
use crate::event_parser::EventDecoder;
use crate::gen::SSVContract;

use alloy::primitives::Address;
use alloy::{rpc::types::Log, sol_types::SolEvent};
use ssv_types::OperatorId;
use std::str::FromStr;
use types::PublicKey;

/// Actions that the network has to take in response to a event during the live sync
#[derive(Debug, PartialEq)]
pub enum NetworkAction {
    StopValidator {
        validator_pubkey: PublicKey,
    },
    LiquidateCluster {
        owner: Address,
        operator_ids: Vec<OperatorId>,
    },
    ReactivateCluster {
        owner: Address,
        operator_ids: Vec<OperatorId>,
    },
    UpdateFeeRecipient {
        owner: Address,
        recipient: Address,
    },
    ExitValidator {
        validator_pubkey: PublicKey,
    },
    NoOp,
}

/// Parse a network log into an action to be executed
impl TryFrom<&Log> for NetworkAction {
    type Error = ExecutionError;
    fn try_from(source: &Log) -> Result<NetworkAction, Self::Error> {
        let topic0 = source.topic0().expect("The log should have a topic0");
        match *topic0 {
            SSVContract::ValidatorRemoved::SIGNATURE_HASH => {
                let SSVContract::ValidatorRemoved { publicKey, .. } =
                    SSVContract::ValidatorRemoved::decode_from_log(source)?;
                let validator_pubkey =
                    PublicKey::from_str(&publicKey.to_string()).map_err(|e| {
                        ExecutionError::InvalidEvent(format!("Failed to create PublicKey: {e}"))
                    })?;
                Ok(NetworkAction::StopValidator { validator_pubkey })
            }
            SSVContract::ClusterLiquidated::SIGNATURE_HASH => {
                let SSVContract::ClusterLiquidated {
                    owner, operatorIds, ..
                } = SSVContract::ClusterLiquidated::decode_from_log(source)?;
                Ok(NetworkAction::LiquidateCluster {
                    owner,
                    operator_ids: operatorIds.into_iter().map(OperatorId).collect(),
                })
            }
            SSVContract::ClusterReactivated::SIGNATURE_HASH => {
                let SSVContract::ClusterReactivated {
                    owner, operatorIds, ..
                } = SSVContract::ClusterReactivated::decode_from_log(source)?;
                Ok(NetworkAction::ReactivateCluster {
                    owner,
                    operator_ids: operatorIds.into_iter().map(OperatorId).collect(),
                })
            }
            SSVContract::FeeRecipientAddressUpdated::SIGNATURE_HASH => {
                let recipient_updated_log =
                    SSVContract::FeeRecipientAddressUpdated::decode_from_log(source)?;
                Ok(NetworkAction::UpdateFeeRecipient {
                    owner: recipient_updated_log.owner,
                    recipient: recipient_updated_log.recipientAddress,
                })
            }
            SSVContract::ValidatorExited::SIGNATURE_HASH => {
                let SSVContract::ValidatorExited { publicKey, .. } =
                    SSVContract::ValidatorExited::decode_from_log(source)?;
                let validator_pubkey =
                    PublicKey::from_str(&publicKey.to_string()).map_err(|e| {
                        ExecutionError::InvalidEvent(format!("Failed to create PublicKey: {e}"))
                    })?;
                Ok(NetworkAction::ExitValidator { validator_pubkey })
            }
            _ => Ok(NetworkAction::NoOp),
        }
    }
}
