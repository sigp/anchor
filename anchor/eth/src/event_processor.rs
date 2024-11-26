use super::action::NetworkAction;
use super::event_parser::EventDecoder;
use super::gen::SSVContract;
use super::sync::MAX_OPERATORS;
use super::sigs::{RawShares, verify_signature};
use alloy::primitives::B256;
use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use std::collections::{HashMap, HashSet};


const SIGNATURE_LEN: usize = 96;
const PUBLICKEY_LENGTH: usize = 48;
const ENCRYPTEDKEY_LENGTH: usize = 32;

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
            let handler = self
                .handlers
                .get(topic0)
                .expect("A handler should exist for this topic");
            handler(self, &log)?;

            let action: NetworkAction = log.try_into()?;
            if action != NetworkAction::NoOp && live {
                // todo!() send off somewhere
            }
        }
        Ok(())
    }

    // Store the operator in the database.
    fn process_operator_added(&self, log: &Log) -> Result<(), String> {
        let SSVContract::OperatorAdded {
            operatorId: id,
            owner,
            publicKey: pubkey,
            ..
        } = SSVContract::OperatorAdded::decode_from_log(log)?;

        // Confirm that this operator does not already exist via ID
        //if self.db.operator_exists_id(id)? {
        //  return Err(format!("Operator with id {} already exists", id"));
        //}

        // Confirm that this operator does not already exist via pubkey
        //if self.db.operator_exists_pubkey(pubkey)? {
        //  return Err(format!("Operator with public key {} already exists", pubkey"));
        //}

        // New unique operator, save into the database
        //self.db.add_operator(id, owner, pubkey)?;
        Ok(())
    }

    // Remove an operator from the database
    fn process_operator_removed(&self, log: &Log) -> Result<(), String> {
        let _decoded = SSVContract::OperatorRemoved::decode_from_log(log)?;
        // this method is currently noop in the ref client
        Ok(())
    }

    fn process_validator_added(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ValidatorAdded {
            owner,
            operatorIds: operator_ids,
            publicKey: pubkey,
            shares,
            cluster,
        } = SSVContract::ValidatorAdded::decode_from_log(log)?;

        // Get expected nonce and and increment it. Talk w/ security guys if this is needed. Wont
        // the network handle this? What does it have to do with database
        // todo!()

        // Perform some validator verification, parse the share byte stream into RawShares, and
        // verifiy the signature is correct
        self.validate_operators(operator_ids)?;
        let shares: RawShares = shares.try_into()?;
        verify_signature()?;

        // Walkthrough
        // 1) We want to see if a share for this validator already exists
        // 2) If it does not exist, we want to create it
        //  1a) Create SSVShare struct (specType.share + metadata)
        //  2a) deserialize publickey (bytes) into actual BLS publickey
        //  3a) populate SSVShare w/ publick key above and owner of the share
        //  4a) get the id of THIS operator
        //  5a) go through all of the operator_ids
        //    1b) extract operator id & get its data
        //    2b) add it to the sharemembers (committee for this share)
        //    3b) if the operator id == id of this operator
        //    4b) decrypt the corresponding encryptedKey with RSAPrivkey + Some validation
        //  6a) return the new share and the private key
        //  7a) validate that this share does indeed belong to this operator
        //  8a) save the share in the database


        // Thoughts. Need to think in terms of a THIS operator. Not the network at large.
        // When a new validator is added, all of the operators will get this event and extract their
        // corresponding share private key. The database will reflect state for this operator.
        Ok(())
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

    // Helper functions
    fn validate_operators(&self, operator_ids: Vec<u64>) -> Result<(), String> {
        let num_operators = operator_ids.len();

        // make sure there is a valid number of operators
        if num_operators > MAX_OPERATORS {
            return Err(format!(
                "Validator has too many operators: {}",
                num_operators
            ));
        }
        if num_operators == 0 {
            return Err("Validator has no operators".to_string());
        }

        // make sure count is valid
        let threshold = (num_operators - 1) / 3;
        if (num_operators - 1) % 3 != 0 || !(1..=4).contains(&threshold) {
            return Err(format!("Invalid number of operators: {}", num_operators));
        }

        // make sure there are no duplicates
        let mut seen = HashSet::new();
        let are_duplicates = !operator_ids.iter().all(|x| seen.insert(x));
        if are_duplicates {
            return Err("Operator IDs contain duplicates".to_string());
        }

        // make sure all of the operators exist
        //if operator_ids.iter().any(|id| !self.db.operators_exist(id)) {
        //    return Err("One or more operators do not exist".to_string());
        //}

        Ok(())
    }
}
