use super::event_parser::EventDecoder;
use super::action::NetworkAction;
use super::gen::SSVContract;
use alloy::primitives::B256;
use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use std::collections::HashMap;

// Handler for a log
type EventHandler = fn(&EventProcessor, &Log) -> Result<(), String>;

// Event Processor
pub struct EventProcessor {
    handlers: HashMap<B256, EventHandler>, // reference to the database
}

impl EventProcessor {
    /// Construct a new EventProcessor
    pub fn new() -> Self {
        // register log handlers for easy dispatch
        let mut handlers: HashMap<B256, EventHandler> = HashMap::new();
        handlers.insert(
            SSVContract::OperatorAdded::SIGNATURE_HASH,
            Self::process_operator_added,
        );
        handlers.insert(
            SSVContract::OperatorRemoved::SIGNATURE_HASH,
            Self::process_operator_removed,
        );
        handlers.insert(
            SSVContract::ValidatorAdded::SIGNATURE_HASH,
            Self::process_validator_added,
        );
        handlers.insert(
            SSVContract::ValidatorRemoved::SIGNATURE_HASH,
            Self::process_validator_removed,
        );
        handlers.insert(
            SSVContract::ClusterLiquidated::SIGNATURE_HASH,
            Self::process_cluster_liquidated,
        );
        handlers.insert(
            SSVContract::ClusterReactivated::SIGNATURE_HASH,
            Self::process_cluster_reactivated,
        );
        handlers.insert(
            SSVContract::FeeRecipientAddressUpdated::SIGNATURE_HASH,
            Self::process_fee_recipient_updated,
        );
        handlers.insert(
            SSVContract::ValidatorExited::SIGNATURE_HASH,
            Self::process_validator_exited,
        );

        Self { handlers }
    }

    /// Process a new set of logs
    pub fn process_logs(&self, logs: Vec<Log>, live: bool) -> Result<(), String> {
        for log in logs {
            let topic0 = log.topic0().expect("Log should have a topic0");
            let handler = self.handlers.get(topic0).expect("A handler should exist for this topic");
            handler(self, &log)?;

            let action: NetworkAction = log.try_into()?;
            if action != NetworkAction::NoOp && live {
                // todo!() send off somewhere
            }
        }
        Ok(())
    }

    // Store the operator in the database
    fn process_operator_added(&self, log: &Log) -> Result<(), String> {
        let _decoded = SSVContract::OperatorAdded::decode_from_log(log)?;
        // check to see if and operator with the same id already exists
        // check to see if an operator with the same public key already exists

        // if both pass, save it to database
        //self.db.add_operator(decoded.operatorID, decoded.owner, decoded.publicKey);
        Ok(())
    }

    fn process_operator_removed(&self, log: &Log) -> Result<(), String> {
        let _decoded = SSVContract::OperatorRemoved::decode_from_log(log)?;
        // this method is currently noop in the ref client
        Ok(())
    }

    fn process_validator_added(&self, log: &Log) -> Result<(), String> {
        let _decoded = SSVContract::ValidatorAdded::decode_from_log(log)?;
        // get the next expected nonce
        // increment the nonce
        // validate the operators
        // create shares
        todo!()
    }

    fn process_validator_removed(&self, log: &Log) -> Result<(), String> {
        let _decoded = SSVContract::ValidatorRemoved::decode_from_log(log)?;
        // get the shares
        // Prevent removal of the validator registered with different owner address
        // owner A registers validator with public key X (OK)
        // owner B registers validator with public key X (NOT OK)
        // owner A removes validator with public key X (OK)
        // owner B removes validator with public key X (NOT OK)
        // delete the shares
        todo!()
    }

    fn process_cluster_liquidated(&self, log: &Log) -> Result<(), String> {
        let _decoded = SSVContract::ClusterLiquidated::decode_from_log(log)?;
        // indicate the shares are liquidated
        todo!()
    }

    fn process_cluster_reactivated(&self, log: &Log) -> Result<(), String> {
        let _decoded = SSVContract::ClusterReactivated::decode_from_log(log)?;
        // process cluster event
        // bump slashing protection
        todo!()
    }

    fn process_fee_recipient_updated(&self, log: &Log) -> Result<(), String> {
        let _decoded = SSVContract::FeeRecipientAddressUpdated::decode_from_log(log)?;
        // fetch recipient data
        // create it if needed, then insert
        todo!()
    }

    fn process_validator_exited(&self, log: &Log) -> Result<(), String> {
        let _decoded = SSVContract::ValidatorExited::decode_from_log(log)?;
        // get the shares
        // exit duty
        todo!()
    }
}
