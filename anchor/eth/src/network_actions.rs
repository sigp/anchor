use super::event_parser::EventDecoder;
use super::gen::SSVContract;
use alloy::primitives::Address;
use alloy::{rpc::types::Log, sol_types::SolEvent};
use ssv_types::OperatorId;

#[derive(Debug, PartialEq)]
pub enum NetworkAction {
    StopValidator {
        //pubkey: PublicKey,
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
        //pubkey: PublicKey,
        //block_number: u64,
        //validator_index: u64,
        //own_validator: bool,
    },
    NoOp,
}

/// Parse a network log into an action to be executed
impl TryFrom<&Log> for NetworkAction {
    type Error = String;
    fn try_from(source: &Log) -> Result<NetworkAction, Self::Error> {
        let topic0 = source.topic0().expect("The log should have a topic0");
        match *topic0 {
            SSVContract::ValidatorRemoved::SIGNATURE_HASH => {
                let _validator_removed_log =
                    SSVContract::ValidatorRemoved::decode_from_log(source)?;
                Ok(NetworkAction::StopValidator {})
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
                let _validator_exited_log = SSVContract::ValidatorExited::decode_from_log(source)?;
                Ok(NetworkAction::ExitValidator {})
            }
            _ => Ok(NetworkAction::NoOp),
        }
    }
}
