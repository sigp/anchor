use super::event_parser::EventDecoder;
use super::gen::SSVContract;
use alloy::primitives::Address;
use alloy::{rpc::types::Log, sol_types::SolEvent};

//use types::{SSVShare, OperatorID}

// Todo!() need some file that defines all the actions that the validator should
// perform. Upon receiving an event in the live sync, the event log needs to be transformed into
// and action, processed & persisted into the database, and then sent off to be executed (execute
// trait in the impl)

// todo!() This should be standardized into a common format that will be used client wide
// we do not want to use the contract events structures directly and want to define some types that
// hold all of the relevant data needed for execution

#[derive(Debug, PartialEq)]
pub enum NetworkAction {
    StopValidator {
        //pubkey:  bls::PublicKey
    },
    LiquidateCluster {
        owner: Address,
        //operator_ids: Vec<OperatorID>,
        //to_liquidate: Vec<SSVShare>
    },
    ReactivateCluster {
        owner: Address,
        //operator_ids: Vec<OperatorID>
        //to_reactivate: Vec<SSVShare>
    },
    UpdateFeeRecipient {
        owner: Address,
        recipient: Address,
    },
    ExitValidator {
        //pubkey: bls::PublicKey
        //block_number: u64,
        //validator_index: u64,
        //own_validator: bool,
    },
    NoOp,
}

/// Parse a network log into an action to be executed
impl TryFrom<Log> for NetworkAction {
    type Error = String;

    fn try_from(source: Log) -> Result<NetworkAction, Self::Error> {
        let topic0 = source.topic0().expect("The log should have a topic0");
        match *topic0 {
            SSVContract::ValidatorRemoved::SIGNATURE_HASH => {
                let _validator_removed_log =
                    SSVContract::ValidatorRemoved::decode_from_log(&source)?;
                Ok(NetworkAction::StopValidator {})
            }
            SSVContract::ClusterLiquidated::SIGNATURE_HASH => {
                let cluster_liquidated_log =
                    SSVContract::ClusterLiquidated::decode_from_log(&source)?;
                Ok(NetworkAction::LiquidateCluster {
                    owner: cluster_liquidated_log.owner,
                })
            }
            SSVContract::ClusterReactivated::SIGNATURE_HASH => {
                let cluster_reactivated_log =
                    SSVContract::ClusterReactivated::decode_from_log(&source)?;
                Ok(NetworkAction::ReactivateCluster {
                    owner: cluster_reactivated_log.owner,
                })
            }
            SSVContract::FeeRecipientAddressUpdated::SIGNATURE_HASH => {
                let recipient_updated_log =
                    SSVContract::FeeRecipientAddressUpdated::decode_from_log(&source)?;
                Ok(NetworkAction::UpdateFeeRecipient {
                    owner: recipient_updated_log.owner,
                    recipient: recipient_updated_log.recipientAddress,
                })
            }
            SSVContract::ValidatorExited::SIGNATURE_HASH => {
                let _validator_exited_log = SSVContract::ValidatorExited::decode_from_log(&source)?;
                Ok(NetworkAction::ExitValidator {})
            }
            _ => Ok(NetworkAction::NoOp),
        }
    }
}
