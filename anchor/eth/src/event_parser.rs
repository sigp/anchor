use super::gen::SSVContract;
use alloy::{rpc::types::Log, sol_types::SolEvent};

// Todo!() need some file that defines all the actions (duties from spec) that the validator should
// perform. Upon receiving an event in the live sync, the event log needs to be transaformed into
// and action, processed & persisted into the database, and then sent off to be executed (Runners in
// the spec).

// todo!() This should be standardized into a common format that will be used client wide
// we do not want to use the contract events structures directly and want to define some types that
// hold all of the relevant data needed for execution
pub enum NetworkAction {
    ValidatorAdded(SSVContract::ValidatorAdded),
}

// Convert (parse) an rpc::Log into an Action
impl From<Log> for NetworkAction {
    fn from(source: Log) -> NetworkAction {
        let topic0 = source.topic0().expect("The log should have a topic0");
        match *topic0 {
            SSVContract::OperatorAdded::SIGNATURE_HASH => todo!(),
            SSVContract::OperatorRemoved::SIGNATURE_HASH => todo!(),
            SSVContract::ValidatorAdded::SIGNATURE_HASH => todo!(),
            SSVContract::ValidatorRemoved::SIGNATURE_HASH => todo!(),
            SSVContract::ClusterLiquidated::SIGNATURE_HASH => todo!(),
            SSVContract::ClusterReactivated::SIGNATURE_HASH => todo!(),
            SSVContract::FeeRecipientAddressUpdated::SIGNATURE_HASH => todo!(),
            SSVContract::ValidatorExited::SIGNATURE_HASH => todo!(),
            _ => panic!("Received an unexpected event log"),
        }
    }
}
