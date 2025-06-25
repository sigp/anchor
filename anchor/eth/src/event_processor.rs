use std::sync::Arc;

use alloy::{primitives::Address, rpc::types::Log, sol_types::SolEvent};
use database::{NetworkDatabase, UniqueIndex};
use eth2::types::PublicKeyBytes;
use indexmap::IndexSet;
use rusqlite::Transaction;
use ssv_types::{Cluster, ClusterId, Operator, OperatorId, ValidatorIndex};
use tracing::{debug, error, info, instrument, trace, warn};

use crate::{
    error::ExecutionError,
    event_parser::EventDecoder,
    generated::SSVContract,
    index_sync, metrics,
    util::*,
    voluntary_exit_processor::{ExitRequest, ExitTx},
};

/// Configures event processing behaviour.
pub enum Mode {
    /// Process all events fully, and trigger index sync for new validators.
    ///
    /// Intended for node operation.
    Node {
        /// Queue to submit new validators to the index lookup
        index_sync_tx: index_sync::Tx,
        /// Queue to submit validator exits for processing
        exit_tx: ExitTx,
    },
    /// Process added validators only by updating the nonce.
    ///
    /// Intended for key splitting, which requires the nonce but not other data.
    KeySplit,
}

/// The Event Processor. This handles all verification and recording of events.
/// It will be passed logs from the sync layer to be processed and saved into the database
pub struct EventProcessor {
    /// Reference to the database
    pub db: Arc<NetworkDatabase>,
    /// Signal if we should only do relevant keysplitting processing
    mode: Mode,
}

impl EventProcessor {
    /// Construct a new EventProcessor
    pub fn new(db: Arc<NetworkDatabase>, mode: Mode) -> Self {
        Self { db, mode }
    }

    /// Process a new set of logs
    #[instrument(skip(self, logs), fields(logs_count = logs.len()), level = "debug")]
    pub fn process_logs(
        &self,
        logs: Vec<Log>,
        live: bool,
        end_block: u64,
    ) -> Result<(), ExecutionError> {
        debug!(logs_count = logs.len(), "Starting log processing");
        let timer = metrics::start_timer(&metrics::EXECUTION_LOG_PROCESSING_TIME);

        // Open a transaction for the log batch.
        let mut conn = self
            .db
            .connection()
            .map_err(|e| ExecutionError::Database(e.to_string()))?;
        let tx = conn
            .transaction()
            .map_err(|e| ExecutionError::Database(e.to_string()))?;

        for (index, log) in logs.iter().enumerate() {
            trace!(log_index = index, topic = ?log.topic0(), "Processing individual log");

            // Extract the topic0 to identify the event type
            let topic0 = match log.topic0() {
                Some(topic) => topic,
                None => {
                    warn!("Log missing topic0, skipping");
                    continue;
                }
            };

            // Process log based on signature hash
            let result = match *topic0 {
                SSVContract::OperatorAdded::SIGNATURE_HASH => self.process_operator_added(log, &tx),

                SSVContract::OperatorRemoved::SIGNATURE_HASH => {
                    self.process_operator_removed(log, &tx)
                }

                SSVContract::ValidatorAdded::SIGNATURE_HASH => {
                    self.process_validator_added(log, &tx)
                }

                SSVContract::ValidatorRemoved::SIGNATURE_HASH => {
                    self.process_validator_removed(log, &tx)
                }

                SSVContract::ClusterLiquidated::SIGNATURE_HASH => {
                    self.process_cluster_liquidated(log, &tx)
                }

                SSVContract::ClusterReactivated::SIGNATURE_HASH => {
                    self.process_cluster_reactivated(log, &tx)
                }

                SSVContract::FeeRecipientAddressUpdated::SIGNATURE_HASH => {
                    self.process_fee_recipient_updated(log, &tx)
                }

                SSVContract::ValidatorExited::SIGNATURE_HASH if live => {
                    self.process_validator_exited(log)
                }
                _ => {
                    debug!(?topic0, "Unknown event signature, skipping");
                    continue;
                }
            };

            // Handle any errors from the event processing
            if let Err(e) = result {
                if live {
                    warn!("Malformed event: {e}");
                } else {
                    debug!("Malformed event: {e}");
                }
                continue;
            }
        }

        metrics::stop_timer(timer);
        self.db
            .processed_block(end_block, &tx)
            .map_err(|e| ExecutionError::Database(e.to_string()))?;

        // Commit everything!
        tx.commit()
            .map_err(|e| ExecutionError::Database(e.to_string()))?;

        debug!(logs_count = logs.len(), "Completed processing logs");
        Ok(())
    }

    // A new Operator has been registered in the network.
    #[instrument(skip(self, log), fields(operator_id, owner), level = "debug")]
    fn process_operator_added(
        &self,
        log: &Log,
        tx: &Transaction<'_>,
    ) -> Result<(), ExecutionError> {
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
        if self.db.state().operator_exists(&operator_id) {
            return Err(ExecutionError::Duplicate(format!(
                "Operator with id {operator_id:?} already exists in database"
            )));
        }

        let data = publicKey.as_ref();

        // If the data is 704 bytes, remove the ssv encoding. Else, just parse the key
        let data = if data.len() == 704 {
            let mut data = &data[64..];
            // while there is a 0 at the end of the data, remove it
            while let [rest @ .., 0] = data {
                data = rest;
            }
            data
        } else {
            data
        };

        // Construct the Operator and insert it into the database
        let operator = Operator::new(data, operator_id, owner).map_err(|e| {
            debug!(
                operator_pubkey = ?publicKey,
                operator_id = ?operator_id,
                error = %e,
                "Failed to construct operator"
            );
            ExecutionError::InvalidEvent(format!("Failed to construct operator: {e}"))
        })?;
        self.db.insert_operator(&operator, tx).map_err(|e| {
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
        metrics::inc_counter_vec(&metrics::EXECUTION_EVENTS_PROCESSED, &["operator_added"]);
        Ok(())
    }

    // An Operator has been removed from the network
    #[instrument(skip(self, log), fields(operator_id), level = "debug")]
    fn process_operator_removed(
        &self,
        log: &Log,
        tx: &Transaction<'_>,
    ) -> Result<(), ExecutionError> {
        // Extract the ID of the Operator
        let SSVContract::OperatorRemoved { operatorId } =
            SSVContract::OperatorRemoved::decode_from_log(log)?;
        let operator_id = OperatorId(operatorId);
        debug!(operator_id = ?operator_id, "Processing operator removed");

        // Delete the operator from database and in memory
        self.db.delete_operator(operator_id, tx).map_err(|e| {
            debug!(
                operator_id = ?operator_id,
                error = %e,
                "Failed to remove operator"
            );
            ExecutionError::Database(format!("Failed to remove operator: {e}"))
        })?;

        debug!(operator_id = ?operatorId, "Operator removed from network");
        metrics::inc_counter_vec(&metrics::EXECUTION_EVENTS_PROCESSED, &["operator_removed"]);
        Ok(())
    }

    // A new validator has entered the network. This means that a either a new cluster has formed
    // and this is the first validator for the cluster, or this validator is joining an existing
    // cluster. Perform data verification, store all relevant data, and extract the KeyShare if it
    // belongs to this operator
    #[instrument(
        skip(self, log),
        fields(validator_pubkey, cluster_id, owner),
        level = "debug"
    )]
    fn process_validator_added(
        &self,
        log: &Log,
        tx: &Transaction<'_>,
    ) -> Result<(), ExecutionError> {
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
        let nonce = self.db.bump_and_get_nonce(&owner, tx).map_err(|e| {
            debug!(owner = ?owner, "Failed to bump nonce");
            ExecutionError::Database(format!("Failed to bump nonce: {e}"))
        })?;

        // During keysplitting, we only care about the nonce
        let Mode::Node {
            index_sync_tx: index_lookup_queue,
            ..
        } = &self.mode
        else {
            return Ok(());
        };

        // Process data into a usable form
        let validator_pubkey = parse_validator_pubkey(&publicKey)?;
        let cluster_id = compute_cluster_id(owner, &operatorIds);
        let operator_ids: Vec<_> = operatorIds.into_iter().map(OperatorId).collect();

        // Perform verification on the operator set and make sure they are all registered in the
        // network
        validate_operators(&operator_ids, &cluster_id, &self.db.state())?;

        // Parse the share byte stream into a list of valid Shares and then verify the signature
        debug!(cluster_id = ?cluster_id, "Parsing and verifying shares");
        let (signature, shares) =
            parse_shares(&shares, &operator_ids, &cluster_id, &validator_pubkey).map_err(|e| {
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

        // Get the fee recipient if one has been stored, otherwise default to the owner address
        let fee_recipient = match self.db.fee_recipient_for_owner(&owner, tx) {
            Ok(Some(address)) => address,
            _ => owner,
        };

        // Finally, construct and insert the full cluster and insert into the database
        let cluster = Cluster {
            cluster_id,
            owner,
            fee_recipient,
            liquidated: false,
            cluster_members: IndexSet::from_iter(operator_ids),
        };
        self.db
            .insert_validator(cluster, &validator_metadata, shares, tx)
            .map_err(|e| {
                debug!(cluster_id = ?cluster_id, error = %e, validator_metadata = ?validator_metadata.public_key, "Failed to insert validator into cluster");
                ExecutionError::Database(format!("Failed to insert validator into cluster: {e}"))
            })?;

        // Schedule validator for index lookup
        if let Err(err) = index_lookup_queue.send(validator_pubkey) {
            error!(?err, "Failed to send validator to index lookup");
        }

        debug!(
            cluster_id = ?cluster_id,
            validator_pubkey = %validator_pubkey,
            "Successfully added validator"
        );
        metrics::inc_counter_vec(&metrics::EXECUTION_EVENTS_PROCESSED, &["validator_added"]);
        Ok(())
    }

    // A validator has been removed from the network and its respective cluster
    #[instrument(
        skip(self, log),
        fields(cluster_id, validator_pubkey, owner),
        level = "debug"
    )]
    fn process_validator_removed(
        &self,
        log: &Log,
        tx: &Transaction<'_>,
    ) -> Result<(), ExecutionError> {
        // Parse and destructure log
        let SSVContract::ValidatorRemoved {
            owner,
            operatorIds,
            publicKey,
            ..
        } = SSVContract::ValidatorRemoved::decode_from_log(log)?;
        debug!(owner = ?owner, public_key = ?publicKey, "Processing Validator Removed");

        // Parse the public key
        let validator_pubkey = parse_validator_pubkey(&publicKey)?;

        // Compute the cluster id
        let cluster_id = compute_cluster_id(owner, &operatorIds);

        let state = self.db.state();
        // Get the metadata for this validator
        let metadata = match state.metadata().get_by(&validator_pubkey) {
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
        let cluster = match state.clusters().get_by(&validator_pubkey) {
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
        drop(state);

        // Remove the validator and all corresponding cluster data
        self.db
            .delete_validator(&validator_pubkey, tx)
            .map_err(|e| {
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
        metrics::inc_counter_vec(&metrics::EXECUTION_EVENTS_PROCESSED, &["validator_removed"]);
        Ok(())
    }

    /// A cluster has ran out of operational funds. Set the cluster as liquidated
    #[instrument(skip(self, log), fields(cluster_id, owner, level = "debug"))]
    fn process_cluster_liquidated(
        &self,
        log: &Log,
        tx: &Transaction<'_>,
    ) -> Result<(), ExecutionError> {
        let SSVContract::ClusterLiquidated {
            owner,
            operatorIds: operator_ids,
            ..
        } = SSVContract::ClusterLiquidated::decode_from_log(log)?;

        let cluster_id = compute_cluster_id(owner, &operator_ids);

        debug!(cluster_id = ?cluster_id, "Processing cluster liquidation");

        // Update the status of the cluster to be liquidated
        self.db.update_status(cluster_id, true, tx).map_err(|e| {
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
        metrics::inc_counter_vec(
            &metrics::EXECUTION_EVENTS_PROCESSED,
            &["cluster_liquidated"],
        );
        Ok(())
    }

    // A cluster that was previously liquidated has had more SSV deposited and is now active
    #[instrument(skip(self, log), fields(cluster_id, owner), level = "debug")]
    fn process_cluster_reactivated(
        &self,
        log: &Log,
        tx: &Transaction<'_>,
    ) -> Result<(), ExecutionError> {
        let SSVContract::ClusterReactivated {
            owner,
            operatorIds: operator_ids,
            ..
        } = SSVContract::ClusterReactivated::decode_from_log(log)?;

        let cluster_id = compute_cluster_id(owner, &operator_ids);

        debug!(cluster_id = ?cluster_id, "Processing cluster reactivation");

        // Update the status of the cluster to be active
        self.db.update_status(cluster_id, false, tx).map_err(|e| {
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
        metrics::inc_counter_vec(
            &metrics::EXECUTION_EVENTS_PROCESSED,
            &["cluster_reactivated"],
        );

        Ok(())
    }

    // The fee recipient address of a validator has been changed
    #[instrument(skip(self, log), fields(owner), level = "debug")]
    fn process_fee_recipient_updated(
        &self,
        log: &Log,
        tx: &Transaction<'_>,
    ) -> Result<(), ExecutionError> {
        let SSVContract::FeeRecipientAddressUpdated {
            owner,
            recipientAddress,
        } = SSVContract::FeeRecipientAddressUpdated::decode_from_log(log)?;
        // update the fee recipient address in the database
        self.db
            .update_fee_recipient(owner, recipientAddress, tx)
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
        metrics::inc_counter_vec(
            &metrics::EXECUTION_EVENTS_PROCESSED,
            &["fee_recipient_updated"],
        );
        Ok(())
    }

    // A validator has exited the beacon chain
    #[instrument(skip(self, log), fields(validator_pubkey, owner), level = "debug")]
    fn process_validator_exited(&self, log: &Log) -> Result<(), ExecutionError> {
        // In KeySplit mode, we don't need to process validator exits
        let Mode::Node { exit_tx, .. } = &self.mode else {
            return Ok(());
        };
        let SSVContract::ValidatorExited {
            owner,
            operatorIds,
            publicKey,
        } = SSVContract::ValidatorExited::decode_from_log(log)?;

        let validator_pubkey = parse_validator_pubkey(&publicKey)?;
        let computed_cluster_id = compute_cluster_id(owner, &operatorIds);

        self.verify_validator_owner(&owner, &validator_pubkey, &computed_cluster_id)?;

        let operator_ids: Vec<OperatorId> = operatorIds.iter().map(|id| OperatorId(*id)).collect();

        // Perform verification on the operator set and make sure they are all registered in the
        // network
        validate_operators(&operator_ids, &computed_cluster_id, &self.db.state())?;

        let block_timestamp = log
            .block_timestamp
            .ok_or_else(|| ExecutionError::InvalidEvent("Block timestamp not set".to_string()))?;

        let validator_index = match self.get_validator_index(&validator_pubkey) {
            Ok(Some(value)) => value,
            Ok(None) => return Ok(()),
            Err(value) => return Err(value),
        };

        let is_our_validator = self.is_our_validator(&validator_pubkey);

        // Send to exit processor instead of handling in-place
        let request = ExitRequest {
            validator_pubkey,
            validator_index,
            block_timestamp,
            is_our_validator,
        };

        match exit_tx.send(request) {
            Ok(_) => {
                info!(
                    validator_pubkey = %validator_pubkey,
                    "Queued validator for exit processing"
                );
            }
            Err(err) => {
                // If the channel is closed, we can't send the exit request
                // This is a fatal error and should be handled by the caller
                error!(
                    validator_pubkey = %validator_pubkey,
                    ?err,
                    "Failed to send validator exit request to processor"
                );
                return Err(ExecutionError::Misc(
                    "Failed to send validator exit request to processor".to_string(),
                ));
            }
        }

        Ok(())
    }

    fn is_our_validator(&self, validator_pubkey: &PublicKeyBytes) -> bool {
        self.db.state().shares().get_by(validator_pubkey).is_some()
    }

    /// Retrieves the validator index for a given validator public key from the database.
    ///
    /// # Parameters
    /// * `validator_pubkey` - The public key of the validator to look up
    ///
    /// # Returns
    /// * `Ok(Some(index))` - If the validator exists and has an index assigned
    /// * `Ok(None)` - If the validator exists but has no index assigned yet
    /// * `Err` - If the validator metadata cannot be found in the database
    fn get_validator_index(
        &self,
        validator_pubkey: &PublicKeyBytes,
    ) -> Result<Option<ValidatorIndex>, ExecutionError> {
        // Get the validator metadata including its index
        let validator_metadata = match self.db.state().metadata().get_by(validator_pubkey) {
            Some(metadata) => metadata,
            None => {
                error!(
                    validator_pubkey = %validator_pubkey,
                    "Validator metadata not found"
                );
                return Err(ExecutionError::InvalidEvent(
                    "Validator metadata not found".to_string(),
                ));
            }
        };

        // Check if we have a validator index (required for exits)
        let validator_index = match validator_metadata.index {
            Some(index) => Some(index),
            None => {
                warn!(
                    validator_pubkey = %validator_pubkey,
                    "Cannot exit validator without index"
                );
                return Ok(None);
            }
        };
        Ok(validator_index)
    }

    /// Verifies that the owner specified in a contract event matches the registered owner of a
    /// validator.
    ///
    /// Note that a validator's owner is considered to be the owner of the cluster to which
    /// the validator belongs.
    ///
    /// # Parameters
    /// * `owner` - The address claimed as owner in the contract event
    /// * `validator_pubkey` - The public key of the validator being verified
    /// * `computed_cluster_id` - The cluster ID computed from the owner and operator IDs in the
    ///   event
    ///
    /// # Returns
    /// * `Ok(())` - If the owner is valid and the cluster IDs match
    /// * `Err` - If validation fails due to cluster not found, cluster ID mismatch, or owner
    ///   mismatch
    ///
    /// # Note
    /// If the cluster is already liquidated, the function will return `Ok(())` but issue a warning.
    fn verify_validator_owner(
        &self,
        owner: &Address,
        validator_pubkey: &PublicKeyBytes,
        computed_cluster_id: &ClusterId,
    ) -> Result<(), ExecutionError> {
        // Get validator's metadata from the database
        let state = self.db.state();

        // Get the cluster for this validator to access owner information
        let cluster = match state.clusters().get_by(validator_pubkey) {
            Some(cluster) => cluster,
            None => {
                error!(
                    validator_pubkey = %validator_pubkey,
                    "Cluster not found for validator"
                );
                return Err(ExecutionError::InvalidEvent(
                    "Cluster not found for validator".to_string(),
                ));
            }
        };

        if cluster.cluster_id != *computed_cluster_id {
            error!(
                validator_pubkey = %validator_pubkey,
                computed_cluster_id = ?computed_cluster_id,
                cluster_id = ?cluster.cluster_id,
                "Validator's cluster id is not the same as the computed cluster id"
            );
            return Err(ExecutionError::InvalidEvent(
                "Validator's cluster id is not the same as the computed cluster id".to_string(),
            ));
        }

        if cluster.liquidated {
            warn!(
                validator_pubkey = %validator_pubkey,
                computed_cluster_id = ?computed_cluster_id,
                "Cluster is liquidated, skipping exit processing"
            );
            return Err(ExecutionError::Misc(
                "Cluster is liquidated, skipping exit processing".to_string(),
            ));
        }

        // Verify that the owner from the contract event is the one who registered the validator
        // (which is stored as the cluster's owner in our database)
        if &cluster.owner != owner {
            error!(
                validator_pubkey = %validator_pubkey,
                registered_owner = ?cluster.owner,
                contract_event_owner = ?owner,
                "Owner mismatch: the address in the contract event is not the validator's registered owner"
            );
            return Err(ExecutionError::InvalidEvent(
                "Contract event owner does not match the validator's registered owner".to_string(),
            ));
        }

        Ok(())
    }
}
