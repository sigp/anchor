use crate::error::ExecutionError;
use crate::event_parser::EventDecoder;
use crate::gen::SSVContract;
use crate::network_actions::NetworkAction;
use crate::util::*;

use alloy::primitives::B256;
use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use database::{NetworkDatabase, UniqueIndex};
use ssv_types::{Cluster, Operator, OperatorId};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, error, info, instrument, trace, warn};
use types::PublicKey;

// Specific Handler for a log type
type EventHandler = fn(&EventProcessor, &Log) -> Result<(), ExecutionError>;

/// The Event Processor. This handles all verification and recording of events.
/// It will be passed logs from the sync layer to be processed and saved into the database
pub struct EventProcessor {
    /// Function handlers for event processing
    handlers: HashMap<B256, EventHandler>,
    /// Reference to the database
    pub db: Arc<NetworkDatabase>,
}

impl EventProcessor {
    /// Construct a new EventProcessor
    pub fn new(db: Arc<NetworkDatabase>) -> Self {
        // Register log handlers for easy dispatch
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

        Self { handlers, db }
    }

    /// Process a new set of logs
    #[instrument(skip(self, logs), fields(logs_count = logs.len()))]
    pub fn process_logs(&self, logs: Vec<Log>, live: bool) {
        info!(logs_count = logs.len(), "Starting log processing");
        for (index, log) in logs.iter().enumerate() {
            trace!(log_index = index, topic = ?log.topic0(), "Processing individual log");

            // extract the topic0 to retrieve log handler
            let topic0 = log.topic0().expect("Log should always have a topic0");
            let handler = self
                .handlers
                .get(topic0)
                .expect("Handler should always exist");

            // Handle the log and emit warning for any malformed events
            if let Err(e) = handler(self, log) {
                warn!("Malformed event: {e}");
                continue;
            }

            // If live is true, then we are currently in a live sync and want to take some action in
            // response to the log. Parse the log into a network action and send to be processed;
            if live {
                let action = match log.try_into() {
                    Ok(action) => action,
                    Err(e) => {
                        error!("Failed to convert log into NetworkAction {e}");
                        NetworkAction::NoOp
                    }
                };
                if action != NetworkAction::NoOp && live {
                    debug!(action = ?action, "Network action ready for processing");
                    // todo!() send off somewhere
                }
            }
        }

        info!(logs_count = logs.len(), "Completed processing logs");
    }

    // A new Operator has been registered in the network.
    #[instrument(skip(self, log), fields(operator_id, owner))]
    fn process_operator_added(&self, log: &Log) -> Result<(), ExecutionError> {
        // Destructure operator added event
        let SSVContract::OperatorAdded {
            operatorId, // The ID of the newly registered operator
            owner,      // The EOA owner address
            publicKey,  // The RSA public key
            ..
        } = SSVContract::OperatorAdded::decode_from_log(log)?;
        let operator_id = OperatorId(operatorId);

        debug!(operator_id = ?operator_id, owner = ?owner, "Processing operator added");

        // Confirm that this operator does not already exist
        if self.db.operator_exists(&operator_id) {
            return Err(ExecutionError::Duplicate(format!(
                "Operator with id {:?} already exists in database",
                operator_id
            )));
        }

        // Parse ABI encoded public key string and trim off 0x prefix for hex decoding
        let public_key_str = publicKey.to_string();
        let public_key_str = public_key_str.trim_start_matches("0x");
        let data = hex::decode(public_key_str).map_err(|e| {
            debug!(operator_id = ?operator_id, error = %e, "Failed to decode public key data from hex");
            ExecutionError::InvalidEvent(
                format!("Failed to decode public key data from hex: {e}")
            )
        })?;

        // Make sure the data is the expected length
        if data.len() != 704 {
            debug!(operator_id = ?operator_id, expected = 704, actual = data.len(), "Invalid public key data length");
            return Err(ExecutionError::InvalidEvent(String::from(
                "Invalid public key data length. Expected 704, got {data.len()}",
            )));
        }

        // Remove abi encoding information and then convert to valid utf8 string
        let data = &data[64..];
        let data = String::from_utf8(data.to_vec()).map_err(|e| {
            debug!(operator_id = ?operator_id, error = %e, "Failed to convert to UTF8 String");
            ExecutionError::InvalidEvent(format!("Failed to convert to UTF8 String: {e}"))
        })?;
        let data = data.trim_matches(char::from(0)).to_string();

        // Construct the Operator and insert it into the database
        let operator = Operator::new(&data, operator_id, owner).map_err(|e| {
            debug!(
                operator_pubkey = ?publicKey,
                operator_id = ?operator_id,
                error = %e,
                "Failed to construct operator"
            );
            ExecutionError::InvalidEvent(format!("Failed to construct operator: {e}"))
        })?;
        self.db.insert_operator(&operator).map_err(|e| {
            debug!(
                operator_id = ?operator_id,
                error = %e,
                "Failed to insert operator into database"
            );
            ExecutionError::Database(format!("Failed to insert operator into database: {e}"))
        })?;

        debug!(
            operator_id = ?operator_id,
            owner = ?owner,
            "Successfully registered operator"
        );
        Ok(())
    }

    // An Operator has been removed from the network
    #[instrument(skip(self, log), fields(operator_id))]
    fn process_operator_removed(&self, log: &Log) -> Result<(), ExecutionError> {
        // Extract the ID of the Operator
        let SSVContract::OperatorRemoved { operatorId } =
            SSVContract::OperatorRemoved::decode_from_log(log)?;
        let operator_id = OperatorId(operatorId);
        debug!(operator_id = ?operator_id, "Processing operator removed");

        // Delete the operator from database and in memory
        self.db.delete_operator(operator_id).map_err(|e| {
            debug!(
                operator_id = ?operator_id,
                error = %e,
                "Failed to remove operator"
            );
            ExecutionError::Database(format!("Failed to remove operator: {e}"))
        })?;

        debug!(operator_id = ?operatorId, "Operator removed from network");
        Ok(())
    }

    // A new validator has entered the network. This means that a either a new cluster has formed
    // and this is the first validator for the cluster, or this validator is joining an existing
    // cluster. Perform data verification, store all relevant data, and extract the KeyShare if it
    // belongs to this operator
    #[instrument(skip(self, log), fields(validator_pubkey, cluster_id, owner))]
    fn process_validator_added(&self, log: &Log) -> Result<(), ExecutionError> {
        // Parse and destructure log
        let SSVContract::ValidatorAdded {
            owner,
            operatorIds,
            publicKey,
            shares,
            ..
        } = SSVContract::ValidatorAdded::decode_from_log(log)?;
        debug!(owner = ?owner, operator_count = operatorIds.len(), "Processing validator addition");

        // Get the expected nonce and then increment it. This will happen regardless of if the
        // event is malformed or not
        let nonce = self.db.get_next_nonce(&owner);
        self.db.bump_nonce(&owner).map_err(|e| {
            debug!(owner = ?owner, "Failed to bump nonce");
            ExecutionError::Database(format!("Failed to bump nonce: {e}"))
        })?;

        // Process data into a usable form
        let validator_pubkey = PublicKey::from_str(&publicKey.to_string()).map_err(|e| {
            debug!(
                validator_pubkey = %publicKey,
                error = %e,
                "Failed to create PublicKey"
            );
            ExecutionError::InvalidEvent(format!("Failed to create PublicKey: {e}"))
        })?;
        let cluster_id = compute_cluster_id(owner, operatorIds.clone());
        let operator_ids: Vec<OperatorId> = operatorIds.iter().map(|id| OperatorId(*id)).collect();

        // Perform verification on the operator set and make sure they are all registered in the
        // network
        debug!(cluster_id = ?cluster_id, "Validating operators");
        validate_operators(&operator_ids).map_err(|e| {
            ExecutionError::InvalidEvent(format!("Failed to validate operators: {e}"))
        })?;
        if operator_ids.iter().any(|id| !self.db.operator_exists(id)) {
            error!(cluster_id = ?cluster_id, "One or more operators do not exist");
            return Err(ExecutionError::Database(
                "One or more operators do not exist".to_string(),
            ));
        }

        // Parse the share byte stream into a list of valid Shares and then verify the signature
        debug!(cluster_id = ?cluster_id, "Parsing and verifying shares");
        let (signature, shares) = parse_shares(
            shares.to_vec(),
            &operator_ids,
            &cluster_id,
            &validator_pubkey,
        )
        .map_err(|e| {
            debug!(cluster_id = ?cluster_id, error = %e, "Failed to parse shares");
            ExecutionError::InvalidEvent(format!("Failed to parse shares. {e}"))
        })?;

        if !verify_signature(signature, nonce, &owner, &validator_pubkey) {
            debug!(cluster_id = ?cluster_id, "Signature verification failed");
            return Err(ExecutionError::InvalidEvent(
                "Signature verification failed".to_string(),
            ));
        }

        // Fetch the validator metadata
        let validator_metadata = construct_validator_metadata(&validator_pubkey, &cluster_id)
            .map_err(|e| {
                debug!(validator_pubkey= ?validator_pubkey, "Failed to fetch validator metadata");
                ExecutionError::Database(format!("Failed to fetch validator metadata: {e}"))
            })?;

        // Finally, construct and insert the full cluster and insert into the database
        let cluster = Cluster {
            cluster_id,
            owner,
            fee_recipient: owner,
            faulty: 0,
            liquidated: false,
            cluster_members: HashSet::from_iter(operator_ids),
        };
        self.db
            .insert_validator(cluster, validator_metadata.clone(), shares)
            .map_err(|e| {
                debug!(cluster_id = ?cluster_id, error = %e, validator_metadata = ?validator_metadata.public_key, "Failed to insert validator into cluster");
                ExecutionError::Database(format!("Failed to insert validator into cluster: {e}"))
            })?;

        debug!(
            cluster_id = ?cluster_id,
            validator_pubkey = %validator_pubkey,
            "Successfully added validator"
        );
        Ok(())
    }

    // A validator has been removed from the network and its respective cluster
    #[instrument(skip(self, log), fields(cluster_id, validator_pubkey, owner))]
    fn process_validator_removed(&self, log: &Log) -> Result<(), ExecutionError> {
        // Parse and destructure log
        let SSVContract::ValidatorRemoved {
            owner,
            operatorIds,
            publicKey,
            ..
        } = SSVContract::ValidatorRemoved::decode_from_log(log)?;
        debug!(owner = ?owner, public_key = ?publicKey, "Processing Validator Removed");

        // Parse the public key
        let validator_pubkey = PublicKey::from_str(&publicKey.to_string()).map_err(|e| {
            debug!(
                validator_pubkey = %publicKey,
                error = %e,
                "Failed to construct validator pubkey in removal"
            );
            ExecutionError::InvalidEvent(format!("Failed to create PublicKey: {e}"))
        })?;

        // Compute the cluster id
        let cluster_id = compute_cluster_id(owner, operatorIds.clone());

        // Get the metadata for this validator
        let metadata = match self.db.metadata().get_by(&validator_pubkey) {
            Some(data) => data,
            None => {
                debug!(
                    cluster_id = ?cluster_id,
                    "Failed to fetch validator metadata from database"
                );
                return Err(ExecutionError::Database(
                    "Failed to fetch validator metadata from database".to_string(),
                ));
            }
        };

        // Get the cluster that this validator is in
        let cluster = match self.db.clusters().get_by(&validator_pubkey) {
            Some(data) => data,
            None => {
                debug!(
                    cluster_id = ?cluster_id,
                    "Failed to fetch cluster from database"
                );
                return Err(ExecutionError::Database(
                    "Failed to fetch cluster from database".to_string(),
                ));
            }
        };

        // Make sure the right owner is removing this validator
        if owner != cluster.owner {
            debug!(
                cluster_id = ?cluster_id,
                expected_owner = ?cluster.owner,
                actual_owner = ?owner,
                "Owner mismatch for validator removal"
            );
            return Err(ExecutionError::InvalidEvent(format!(
                "Cluster already exists with a different owner address. Expected {}. Got {}",
                cluster.owner, owner
            )));
        }

        // Make sure this is the correct validator
        if validator_pubkey != metadata.public_key {
            debug!(
                cluster_id = ?cluster_id,
                expected_pubkey = %metadata.public_key,
                actual_pubkey = %validator_pubkey,
                "Validator pubkey mismatch"
            );
            return Err(ExecutionError::InvalidEvent(
                "Validator does not match".to_string(),
            ));
        }

        // Check if we are a member of this cluster, if so we are storing the share and have to
        // remove it
        if self.db.member_of_cluster(&cluster_id) {
            debug!(cluster_id = ?cluster_id, "Removing cluster from local keystore");
            // todo!(): Remove it from the internal keystore when it is made
        }

        // Remove the validator and all corresponding cluster data
        self.db.delete_validator(&validator_pubkey).map_err(|e| {
            debug!(
                cluster_id = ?cluster_id,
                pubkey = ?validator_pubkey,
                error = %e,
                "Failed to delete valiidator from database"
            );
            ExecutionError::Database(format!("Failed to validator cluster: {e}"))
        })?;

        debug!(
            cluster_id = ?cluster_id,
            validator_pubkey = %validator_pubkey,
            "Successfully removed validator and cluster"
        );
        Ok(())
    }

    /// A cluster has ran out of operational funds. Set the cluster as liquidated
    #[instrument(skip(self, log), fields(cluster_id, owner))]
    fn process_cluster_liquidated(&self, log: &Log) -> Result<(), ExecutionError> {
        let SSVContract::ClusterLiquidated {
            owner,
            operatorIds: operator_ids,
            ..
        } = SSVContract::ClusterLiquidated::decode_from_log(log)?;

        let cluster_id = compute_cluster_id(owner, operator_ids);

        debug!(cluster_id = ?cluster_id, "Processing cluster liquidation");

        // Update the status of the cluster to be liquidated
        self.db.update_status(cluster_id, true).map_err(|e| {
            debug!(
                cluster_id = ?cluster_id,
                error = %e,
                "Failed to mark cluster as liquidated"
            );
            ExecutionError::Database(format!("Failed to mark cluster as liquidated: {e}"))
        })?;

        debug!(
            cluster_id = ?cluster_id,
            owner = ?owner,
            "Cluster marked as liquidated"
        );
        Ok(())
    }

    // A cluster that was previously liquidated has had more SSV deposited and is now active
    #[instrument(skip(self, log), fields(cluster_id, owner))]
    fn process_cluster_reactivated(&self, log: &Log) -> Result<(), ExecutionError> {
        let SSVContract::ClusterReactivated {
            owner,
            operatorIds: operator_ids,
            ..
        } = SSVContract::ClusterReactivated::decode_from_log(log)?;

        let cluster_id = compute_cluster_id(owner, operator_ids);

        debug!(cluster_id = ?cluster_id, "Processing cluster reactivation");

        // Update the status of the cluster to be active
        self.db.update_status(cluster_id, false).map_err(|e| {
            debug!(
                cluster_id = ?cluster_id,
                error = %e,
                "Failed to mark cluster as active"
            );
            ExecutionError::Database(format!("Failed to mark cluster as active: {e}"))
        })?;

        debug!(
            cluster_id = ?cluster_id,
            owner = ?owner,
            "Cluster reactivated"
        );
        Ok(())
    }

    // The fee recipient address of a validator has been changed
    #[instrument(skip(self, log), fields(owner))]
    fn process_fee_recipient_updated(&self, log: &Log) -> Result<(), ExecutionError> {
        let SSVContract::FeeRecipientAddressUpdated {
            owner,
            recipientAddress,
        } = SSVContract::FeeRecipientAddressUpdated::decode_from_log(log)?;
        // update the fee recipient address in the database
        self.db
            .update_fee_recipient(owner, recipientAddress)
            .map_err(|e| {
                debug!(
                    owner = ?owner,
                    error = %e,
                    "Failed to update fee recipient"
                );
                ExecutionError::Database(format!("Failed to update fee recipient: {e}"))
            })?;
        debug!(
            owner = ?owner,
            new_recipient = ?recipientAddress,
            "Fee recipient address updated"
        );
        Ok(())
    }

    // A validator has exited the beacon chain
    #[instrument(skip(self, log), fields(validator_pubkey, owner))]
    fn process_validator_exited(&self, log: &Log) -> Result<(), ExecutionError> {
        let SSVContract::ValidatorExited {
            owner,
            operatorIds,
            publicKey,
        } = SSVContract::ValidatorExited::decode_from_log(log)?;
        // just create a validator exit task
        debug!(
            owner = ?owner,
            validator_pubkey = ?publicKey,
            operator_count = operatorIds.len(),
            "Validator exited from network"
        );
        Ok(())
    }
}
