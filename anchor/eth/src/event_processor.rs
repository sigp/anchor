use std::sync::Arc;

use alloy::{primitives::Address, rpc::types::Log, sol_types::SolEvent};
use base64::prelude::*;
use bls::PublicKeyBytes;
use database::{NetworkDatabase, PendingStateUpdates, SlashingProtection};
use indexmap::IndexSet;
use rusqlite::{Connection, Transaction};
use ssv_types::{Cluster, ClusterId, Operator, OperatorId, ValidatorIndex};
use tracing::{debug, error, info, instrument, trace, warn};

use crate::{
    error::{ExecutionError, LogErrorDisposition},
    event_parser::EventDecoder,
    generated::SSVContract,
    index_sync, metrics,
    util::*,
    voluntary_exit_processor::{ExitRequest, ExitTx},
};

/// Configures event processing behaviour.
#[derive(Clone)]
pub enum Mode {
    /// Process all events fully, and trigger index sync for new validators.
    ///
    /// Intended for node operation.
    Node {
        /// Queue to submit new validators to the index lookup
        index_sync_tx: index_sync::Tx,
        /// Queue to submit validator exits for processing
        exit_tx: ExitTx,
        /// Slashing protection implementation for validator registration
        slashing_protection: Arc<dyn SlashingProtection>,
    },
    /// Process added validators only by updating the nonce.
    ///
    /// Intended for key splitting, which requires the nonce but not other data.
    KeySplit,
}

enum PostCommitAction {
    IndexSync(PublicKeyBytes),
    ValidatorExit(ExitRequest),
}

/// The Event Processor. This handles all verification and recording of events.
/// It will be passed logs from the sync layer to be processed and saved into the database
///
/// It is basically only an Arc and some queue senders, so cloning it is fine.
#[derive(Clone)]
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

    /// Process a fetched batch of logs.
    ///
    /// The fetch layer can hand us logs spanning multiple blocks, but PR2 commits and publishes
    /// one block at a time. We therefore buffer each block's logs, flush them when the block
    /// number changes, and finally flush `end_block` even if it had no relevant logs so
    /// `last_processed_block` still advances across empty blocks.
    #[instrument(skip(self, logs), fields(logs_count = logs.len()), level = "debug")]
    pub fn process_logs(
        &self,
        logs: Vec<Log>,
        live: bool,
        end_block: u64,
    ) -> Result<(), ExecutionError> {
        let logs_count = logs.len();
        debug!(logs_count, "Starting log processing");
        let timer = metrics::start_timer(&metrics::EXECUTION_LOG_PROCESSING_TIME);

        // Counters for summary logging
        let mut validators_added = 0;
        let mut validators_removed = 0;

        let mut conn = self
            .db
            .connection()
            .map_err(|e| ExecutionError::Database(e.to_string()))?;

        // Buffer the current block so each transaction/process step owns exactly one block.
        let mut current_block = None;
        let mut block_logs = Vec::new();

        for log in logs {
            let block_number = log.block_number.unwrap_or(end_block);

            // We hit a block boundary, so flush the previous block before buffering this one.
            if current_block
                .is_some_and(|current_block_number| current_block_number != block_number)
            {
                self.flush_current_block_if_any(
                    &mut conn,
                    &mut current_block,
                    &mut block_logs,
                    live,
                    &mut validators_added,
                    &mut validators_removed,
                )?;
            }

            current_block = Some(block_number);
            block_logs.push(log);
        }

        // Flush the final buffered block. The loop above only flushes when it sees the next block.
        let last_flushed_block = self.flush_current_block_if_any(
            &mut conn,
            &mut current_block,
            &mut block_logs,
            live,
            &mut validators_added,
            &mut validators_removed,
        )?;

        if last_flushed_block != Some(end_block) {
            // The fetched range ended on a block with no relevant logs, so flush an empty block
            // to still advance `last_processed_block` to `end_block`.
            self.process_block_logs(
                &mut conn,
                &[],
                live,
                end_block,
                &mut validators_added,
                &mut validators_removed,
            )?;
        }

        metrics::stop_timer(timer);

        // Log summaries for validator operations
        if validators_added > 0 {
            debug!(count = validators_added, "Added validators");
        }
        if validators_removed > 0 {
            debug!(count = validators_removed, "Removed validators");
        }

        debug!(logs_count, "Completed processing logs");
        Ok(())
    }

    fn flush_current_block_if_any(
        &self,
        conn: &mut Connection,
        current_block: &mut Option<u64>,
        block_logs: &mut Vec<Log>,
        live: bool,
        validators_added: &mut u64,
        validators_removed: &mut u64,
    ) -> Result<Option<u64>, ExecutionError> {
        let Some(block_number) = current_block.take() else {
            return Ok(None);
        };

        self.process_block_logs(
            conn,
            block_logs,
            live,
            block_number,
            validators_added,
            validators_removed,
        )?;
        block_logs.clear();
        Ok(Some(block_number))
    }

    fn process_block_logs(
        &self,
        conn: &mut Connection,
        logs: &[Log],
        live: bool,
        block_number: u64,
        validators_added: &mut u64,
        validators_removed: &mut u64,
    ) -> Result<(), ExecutionError> {
        let tx = conn
            .transaction()
            .map_err(|e| ExecutionError::Database(e.to_string()))?;
        let mut state_updates = PendingStateUpdates::default();
        let mut post_commit_actions = Vec::new();

        for (index, log) in logs.iter().enumerate() {
            trace!(
                block_number,
                log_index = index,
                topic = ?log.topic0(),
                "Processing individual log"
            );

            let topic0 = match log.topic0() {
                Some(topic) => topic,
                None => {
                    warn!(block_number, "Log missing topic0, skipping");
                    continue;
                }
            };

            let result = match *topic0 {
                SSVContract::OperatorAdded::SIGNATURE_HASH => {
                    self.process_operator_added(log, &tx, &mut state_updates)
                }
                SSVContract::OperatorRemoved::SIGNATURE_HASH => {
                    self.process_operator_removed(log, &tx, &mut state_updates)
                }
                SSVContract::ValidatorAdded::SIGNATURE_HASH => self
                    .process_validator_added(log, &tx, &mut state_updates, &mut post_commit_actions)
                    .inspect(|_| *validators_added += 1),
                SSVContract::ValidatorRemoved::SIGNATURE_HASH => self
                    .process_validator_removed(log, &tx, &mut state_updates)
                    .inspect(|_| *validators_removed += 1),
                SSVContract::ClusterLiquidated::SIGNATURE_HASH => {
                    self.process_cluster_liquidated(log, &tx, &mut state_updates)
                }
                SSVContract::ClusterReactivated::SIGNATURE_HASH => {
                    self.process_cluster_reactivated(log, &tx, &mut state_updates)
                }
                SSVContract::FeeRecipientAddressUpdated::SIGNATURE_HASH => {
                    self.process_fee_recipient_updated(log, &tx, &mut state_updates)
                }
                SSVContract::ValidatorExited::SIGNATURE_HASH => {
                    self.process_validator_exited(log, &tx, live, &mut post_commit_actions)
                }
                _ => {
                    debug!(block_number, ?topic0, "Unknown event signature, skipping");
                    continue;
                }
            };

            if let Err(e) = result {
                let tx_hash = log
                    .transaction_hash
                    .map(|hash| hash.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                match e.log_disposition() {
                    LogErrorDisposition::SkipMalformed => {
                        if live {
                            warn!(block_number, tx_hash, error = %e, "Malformed event");
                        } else {
                            trace!(block_number, tx_hash, error = %e, "Malformed event");
                        }
                    }
                    LogErrorDisposition::SkipExpected => {
                        if live {
                            debug!(block_number, tx_hash, error = %e, "Skipping event");
                        } else {
                            trace!(block_number, tx_hash, error = %e, "Skipping event");
                        }
                    }
                    LogErrorDisposition::SkipAmbiguous => {
                        if live {
                            warn!(
                                block_number,
                                tx_hash,
                                error = %e,
                                "Skipping event with missing committed state"
                            );
                        } else {
                            trace!(
                                block_number,
                                tx_hash,
                                error = %e,
                                "Skipping event with missing committed state"
                            );
                        }
                    }
                    LogErrorDisposition::AbortBatch => {
                        error!(block_number, tx_hash, error = %e, "Event processing failed");
                        return Err(e);
                    }
                }
            }
        }

        self.db
            .processed_block_tx(block_number, &tx, &mut state_updates)
            .map_err(|e| ExecutionError::Database(e.to_string()))?;

        tx.commit()
            .map_err(|e| ExecutionError::Database(e.to_string()))?;
        self.db.publish_pending_state_updates(state_updates);
        self.execute_post_commit_actions(post_commit_actions);
        Ok(())
    }

    fn execute_post_commit_actions(&self, post_commit_actions: Vec<PostCommitAction>) {
        let Mode::Node {
            index_sync_tx,
            exit_tx,
            ..
        } = &self.mode
        else {
            debug_assert!(post_commit_actions.is_empty());
            return;
        };

        // These side effects run after the block transaction commits and NetworkState is
        // published, so failures are logged but not returned to the sync loop. Retrying from the
        // caller would only reprocess already-committed events.
        for action in post_commit_actions {
            match action {
                PostCommitAction::IndexSync(validator_pubkey) => {
                    if let Err(err) = index_sync_tx.send(validator_pubkey) {
                        error!(?err, "Failed to send validator to index lookup");
                    }
                }
                PostCommitAction::ValidatorExit(request) => {
                    let validator_pubkey = request.validator_pubkey;

                    if let Err(err) = exit_tx.send(request) {
                        error!(
                            validator_pubkey = %validator_pubkey,
                            ?err,
                            "Failed to send validator exit request to processor"
                        );
                    } else {
                        info!(
                            validator_pubkey = %validator_pubkey,
                            "Queued validator for exit processing"
                        );
                    }
                }
            }
        }
    }

    // A new Operator has been registered in the network.
    fn process_operator_added(
        &self,
        log: &Log,
        tx: &Transaction<'_>,
        state_updates: &mut PendingStateUpdates,
    ) -> Result<(), ExecutionError> {
        // Destructure operator added event
        let SSVContract::OperatorAdded {
            operatorId, // The ID of the newly registered operator
            owner,      // The EOA owner address
            publicKey,  // The RSA public key
            ..
        } = SSVContract::OperatorAdded::decode_from_log(log)?;
        let operator_id = OperatorId(operatorId);

        trace!(operator_id = ?operator_id, owner = ?owner, "Processing operator added");

        // Confirm that this operator does not already exist
        if self
            .db
            .operator_exists_tx(operator_id, tx)
            .map_err(|e| ExecutionError::Database(e.to_string()))?
        {
            return Err(ExecutionError::Duplicate(format!(
                "Operator with id {operator_id:?} already exists in database"
            )));
        }

        let max_seen = self
            .db
            .get_max_operator_id_seen_tx(tx)
            .map_err(|e| ExecutionError::Database(e.to_string()))?;

        // Only check for missing operators if we have a previous max (not a migrated database)
        if let Some(max_seen) = max_seen
            && max_seen != operatorId - 1
        {
            return Err(ExecutionError::InvalidEvent(format!(
                "Missing OperatorAdded events: database has only seen up to id {max_seen}, \
                but got operator {operator_id}."
            )));
        }

        self.db
            .set_max_operator_id_seen_tx(operatorId, tx, state_updates)
            .map_err(|e| ExecutionError::Database(e.to_string()))?;

        let operator = match parse_operator_public_key(publicKey.as_ref(), operator_id, owner) {
            Ok(operator) => operator,
            Err(reason) => {
                self.db
                    .insert_skipped_operator_add_tx(operator_id, &reason, tx)
                    .map_err(|e| ExecutionError::Database(e.to_string()))?;
                return Err(ExecutionError::InvalidEvent(reason));
            }
        };

        if let Some(existing_operator_id) = self
            .db
            .get_any_operator_id_by_public_key_tx(&operator.rsa_pubkey, tx)
            .map_err(|e| ExecutionError::Database(e.to_string()))?
        {
            let reason =
                format!("Operator public key already exists as operator {existing_operator_id}");
            self.db
                .insert_skipped_operator_add_tx(operator_id, &reason, tx)
                .map_err(|e| ExecutionError::Database(e.to_string()))?;
            return Err(ExecutionError::InvalidEvent(reason));
        }

        self.db
            .insert_operator_tx(&operator, tx, state_updates)
            .map_err(|e| {
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
    fn process_operator_removed(
        &self,
        log: &Log,
        tx: &Transaction<'_>,
        state_updates: &mut PendingStateUpdates,
    ) -> Result<(), ExecutionError> {
        // Extract the ID of the Operator
        let SSVContract::OperatorRemoved { operatorId } =
            SSVContract::OperatorRemoved::decode_from_log(log)?;
        let operator_id = OperatorId(operatorId);
        trace!(operator_id = ?operator_id, "Processing operator removed");

        if self
            .db
            .delete_skipped_operator_add_tx(operator_id, tx)
            .map_err(|e| ExecutionError::Database(e.to_string()))?
        {
            return Err(ExecutionError::SkippedEvent(format!(
                "Operator {operator_id} was previously skipped during registration"
            )));
        }

        // Delete the operator from database and in memory
        self.db
            .delete_operator_tx(operator_id, tx, state_updates)
            .map_err(|e| ExecutionError::Database(format!("Failed to remove operator: {e}")))?;

        debug!(operator_id = ?operatorId, "Operator removed from network");
        metrics::inc_counter_vec(&metrics::EXECUTION_EVENTS_PROCESSED, &["operator_removed"]);
        Ok(())
    }

    // A new validator has entered the network. This means that a either a new cluster has formed
    // and this is the first validator for the cluster, or this validator is joining an existing
    // cluster. Perform data verification, store all relevant data, and extract the KeyShare if it
    // belongs to this operator
    fn process_validator_added(
        &self,
        log: &Log,
        tx: &Transaction<'_>,
        state_updates: &mut PendingStateUpdates,
        post_commit_actions: &mut Vec<PostCommitAction>,
    ) -> Result<(), ExecutionError> {
        // Parse and destructure log
        let SSVContract::ValidatorAdded {
            owner,
            operatorIds,
            publicKey,
            shares,
            ..
        } = SSVContract::ValidatorAdded::decode_from_log(log)?;
        trace!(owner = ?owner, operator_count = operatorIds.len(), "Processing validator addition");

        // Get the expected nonce and then increment it. This will happen regardless of if the
        // event is malformed or not
        let nonce = self
            .db
            .bump_and_get_nonce_tx(&owner, tx, state_updates)
            .map_err(|e| ExecutionError::Database(format!("Failed to bump nonce: {e}")))?;

        // During keysplitting, we only care about the nonce
        let Mode::Node {
            slashing_protection,
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
        validate_operators(&operator_ids, &cluster_id, &self.db, tx)?;

        // Parse the share byte stream into a list of valid Shares and then verify the signature
        trace!(cluster_id = ?cluster_id, "Parsing and verifying shares");
        let (signature, shares) =
            parse_shares(&shares, &operator_ids, &cluster_id, &validator_pubkey).map_err(|e| {
                ExecutionError::InvalidEvent(format!("Failed to parse shares. {e}"))
            })?;

        if !verify_signature(signature, nonce, &owner, &validator_pubkey) {
            return Err(ExecutionError::InvalidEvent(
                "Signature verification failed".to_string(),
            ));
        }

        self.validate_validator_added_conflict(&owner, &validator_pubkey, &cluster_id, tx)?;

        // Fetch the validator metadata
        let validator_metadata = construct_validator_metadata(&validator_pubkey, &cluster_id)
            .map_err(|e| {
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

        // First, do the slashing protection database...
        slashing_protection
            .register_validator(validator_pubkey)
            .map_err(|e| {
                ExecutionError::Database(format!(
                    "Failed to insert validator into slashing db: {e}"
                ))
            })?;

        // ...then the main database.
        self.db
            .insert_validator_tx(cluster, &validator_metadata, shares, tx, state_updates)
            .map_err(|e| {
                ExecutionError::Database(format!("Failed to insert validator into cluster: {e}"))
            })?;

        post_commit_actions.push(PostCommitAction::IndexSync(validator_pubkey));

        trace!(
            cluster_id = ?cluster_id,
            validator_pubkey = %validator_pubkey,
            "Successfully added validator"
        );
        metrics::inc_counter_vec(&metrics::EXECUTION_EVENTS_PROCESSED, &["validator_added"]);
        Ok(())
    }

    /// Rejects `ValidatorAdded` events that conflict with validator state already reconstructed by
    /// Anchor.
    ///
    /// Anchor and the Go SSV node both key reconstructed validator state globally by validator
    /// pubkey, even though the contract stores registrations per `(owner, pubkey)`. Until the
    /// wider data model changes, treat a second owner for an existing pubkey as malformed and skip
    /// it rather than aborting historical sync on a DB uniqueness error.
    fn validate_validator_added_conflict(
        &self,
        owner: &Address,
        validator_pubkey: &PublicKeyBytes,
        computed_cluster_id: &ClusterId,
        tx: &Transaction<'_>,
    ) -> Result<(), ExecutionError> {
        let Some(_) = self
            .db
            .get_validator_metadata_tx(validator_pubkey, tx)
            .map_err(|e| ExecutionError::Database(e.to_string()))?
        else {
            return Ok(());
        };

        let Some(existing_cluster) = self
            .db
            .get_cluster_by_validator_tx(validator_pubkey, tx)
            .map_err(|e| ExecutionError::Database(e.to_string()))?
        else {
            return Err(ExecutionError::MissingCommittedState(
                "Failed to fetch cluster for existing validator metadata".to_string(),
            ));
        };

        if existing_cluster.owner != *owner {
            return Err(ExecutionError::InvalidEvent(format!(
                "Validator already exists with different owner address. Expected {}. Got {}",
                existing_cluster.owner, owner
            )));
        }

        if existing_cluster.cluster_id != *computed_cluster_id {
            return Err(ExecutionError::InvalidEvent(format!(
                "Validator already exists with different cluster id. Expected {:?}. Got {:?}",
                existing_cluster.cluster_id, computed_cluster_id
            )));
        }

        Err(ExecutionError::Duplicate(format!(
            "Validator already exists for owner {} and cluster {:?}",
            owner, computed_cluster_id
        )))
    }

    // A validator has been removed from the network and its respective cluster
    fn process_validator_removed(
        &self,
        log: &Log,
        tx: &Transaction<'_>,
        state_updates: &mut PendingStateUpdates,
    ) -> Result<(), ExecutionError> {
        // Parse and destructure log
        let SSVContract::ValidatorRemoved {
            owner,
            operatorIds,
            publicKey,
            ..
        } = SSVContract::ValidatorRemoved::decode_from_log(log)?;
        trace!(owner = ?owner, public_key = ?publicKey, "Processing Validator Removed");

        // Parse the public key
        let validator_pubkey = parse_validator_pubkey(&publicKey)?;

        // Compute the cluster id
        let cluster_id = compute_cluster_id(owner, &operatorIds);

        // Get the metadata for this validator
        let metadata = match self
            .db
            .get_validator_metadata_tx(&validator_pubkey, tx)
            .map_err(|e| ExecutionError::Database(e.to_string()))?
        {
            Some(data) => data,
            None => {
                return Err(ExecutionError::MissingCommittedState(
                    "Failed to fetch validator metadata from database".to_string(),
                ));
            }
        };

        // Get the cluster that this validator is in
        let cluster = match self
            .db
            .get_cluster_by_validator_tx(&validator_pubkey, tx)
            .map_err(|e| ExecutionError::Database(e.to_string()))?
        {
            Some(data) => data,
            None => {
                return Err(ExecutionError::MissingCommittedState(
                    "Failed to fetch cluster from database".to_string(),
                ));
            }
        };

        // Make sure the right owner is removing this validator
        if owner != cluster.owner {
            return Err(ExecutionError::InvalidEvent(format!(
                "Cluster already exists with a different owner address. Expected {}. Got {}",
                cluster.owner, owner
            )));
        }

        // Make sure this is the correct validator
        if validator_pubkey != metadata.public_key {
            return Err(ExecutionError::InvalidEvent(
                "Validator does not match".to_string(),
            ));
        }

        // Remove the validator and all corresponding cluster data
        self.db
            .delete_validator_tx(&validator_pubkey, tx, state_updates)
            .map_err(|e| ExecutionError::Database(format!("Failed to delete validator: {e}")))?;

        trace!(
            cluster_id = ?cluster_id,
            validator_pubkey = %validator_pubkey,
            "Successfully removed validator and cluster"
        );
        metrics::inc_counter_vec(&metrics::EXECUTION_EVENTS_PROCESSED, &["validator_removed"]);
        Ok(())
    }

    /// A cluster has ran out of operational funds. Set the cluster as liquidated
    fn process_cluster_liquidated(
        &self,
        log: &Log,
        tx: &Transaction<'_>,
        state_updates: &mut PendingStateUpdates,
    ) -> Result<(), ExecutionError> {
        let SSVContract::ClusterLiquidated {
            owner,
            operatorIds: operator_ids,
            ..
        } = SSVContract::ClusterLiquidated::decode_from_log(log)?;

        let cluster_id = compute_cluster_id(owner, &operator_ids);

        trace!(cluster_id = ?cluster_id, "Processing cluster liquidation");

        // Update the status of the cluster to be liquidated
        self.db
            .update_status_tx(cluster_id, true, tx, state_updates)
            .map_err(|e| {
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
    fn process_cluster_reactivated(
        &self,
        log: &Log,
        tx: &Transaction<'_>,
        state_updates: &mut PendingStateUpdates,
    ) -> Result<(), ExecutionError> {
        let SSVContract::ClusterReactivated {
            owner,
            operatorIds: operator_ids,
            ..
        } = SSVContract::ClusterReactivated::decode_from_log(log)?;

        let cluster_id = compute_cluster_id(owner, &operator_ids);

        trace!(cluster_id = ?cluster_id, "Processing cluster reactivation");

        // Update the status of the cluster to be active
        self.db
            .update_status_tx(cluster_id, false, tx, state_updates)
            .map_err(|e| {
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
    fn process_fee_recipient_updated(
        &self,
        log: &Log,
        tx: &Transaction<'_>,
        state_updates: &mut PendingStateUpdates,
    ) -> Result<(), ExecutionError> {
        let SSVContract::FeeRecipientAddressUpdated {
            owner,
            recipientAddress,
        } = SSVContract::FeeRecipientAddressUpdated::decode_from_log(log)?;
        // update the fee recipient address in the database
        self.db
            .update_fee_recipient_tx(owner, recipientAddress, tx, state_updates)
            .map_err(|e| {
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
    fn process_validator_exited(
        &self,
        log: &Log,
        tx: &Transaction<'_>,
        live: bool,
        post_commit_actions: &mut Vec<PostCommitAction>,
    ) -> Result<(), ExecutionError> {
        // In KeySplit mode, we don't need to process validator exits
        let Mode::Node { .. } = &self.mode else {
            return Ok(());
        };
        let SSVContract::ValidatorExited {
            owner,
            operatorIds,
            publicKey,
        } = SSVContract::ValidatorExited::decode_from_log(log)?;

        let validator_pubkey = parse_validator_pubkey(&publicKey)?;
        let computed_cluster_id = compute_cluster_id(owner, &operatorIds);

        self.verify_validator_owner(&owner, &validator_pubkey, &computed_cluster_id, tx)?;

        let operator_ids: Vec<OperatorId> = operatorIds.iter().map(|id| OperatorId(*id)).collect();

        // Perform verification on the operator set and make sure they are all registered in the
        // network
        validate_operators(&operator_ids, &computed_cluster_id, &self.db, tx)?;

        let block_timestamp = log
            .block_timestamp
            .ok_or_else(|| ExecutionError::InvalidEvent("Block timestamp not set".to_string()))?;

        let validator_index = match self.get_validator_index(&validator_pubkey, tx) {
            Ok(Some(value)) => value,
            Ok(None) => return Ok(()),
            Err(value) => return Err(value),
        };

        let is_our_validator = self.is_our_validator(&validator_pubkey, tx)?;

        if !live {
            if is_our_validator {
                debug!(
                    %validator_index,
                    "Ignoring historic validator exit for validator assigned to us"
                );
            } else {
                trace!(%validator_index, "Ignoring historic validator exit");
            }
            return Ok(());
        }

        // Send to exit processor instead of handling in-place
        let request = ExitRequest {
            validator_pubkey,
            validator_index,
            block_timestamp,
            is_our_validator,
        };

        post_commit_actions.push(PostCommitAction::ValidatorExit(request));

        Ok(())
    }

    fn is_our_validator(
        &self,
        validator_pubkey: &PublicKeyBytes,
        tx: &Transaction<'_>,
    ) -> Result<bool, ExecutionError> {
        self.db
            .has_own_share_tx(validator_pubkey, tx)
            .map_err(|e| ExecutionError::Database(e.to_string()))
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
        tx: &Transaction<'_>,
    ) -> Result<Option<ValidatorIndex>, ExecutionError> {
        // Get the validator metadata including its index
        let validator_metadata = match self
            .db
            .get_validator_metadata_tx(validator_pubkey, tx)
            .map_err(|e| ExecutionError::Database(e.to_string()))?
        {
            Some(metadata) => metadata,
            None => {
                return Err(ExecutionError::InvalidEvent(
                    "Validator metadata not found".to_string(),
                ));
            }
        };

        // Check if we have a validator index (required for exits)
        let validator_index = match validator_metadata.index {
            Some(index) => Some(index),
            None => {
                trace!(
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
    /// If the cluster is already liquidated, the function returns `SkippedEvent` so the caller can
    /// skip it without aborting the batch.
    fn verify_validator_owner(
        &self,
        owner: &Address,
        validator_pubkey: &PublicKeyBytes,
        computed_cluster_id: &ClusterId,
        tx: &Transaction<'_>,
    ) -> Result<(), ExecutionError> {
        // Get the cluster for this validator to access owner information
        let cluster = match self
            .db
            .get_cluster_by_validator_tx(validator_pubkey, tx)
            .map_err(|e| ExecutionError::Database(e.to_string()))?
        {
            Some(cluster) => cluster,
            None => {
                return Err(ExecutionError::InvalidEvent(
                    "Cluster not found for validator".to_string(),
                ));
            }
        };

        if cluster.cluster_id != *computed_cluster_id {
            return Err(ExecutionError::InvalidEvent(
                "Validator's cluster id is not the same as the computed cluster id".to_string(),
            ));
        }

        if cluster.liquidated {
            return Err(ExecutionError::SkippedEvent(
                "Cluster is liquidated, skipping exit processing".to_string(),
            ));
        }

        // Verify that the owner from the contract event is the one who registered the validator
        // (which is stored as the cluster's owner in our database)
        if &cluster.owner != owner {
            return Err(ExecutionError::InvalidEvent(
                "Contract event owner does not match the validator's registered owner".to_string(),
            ));
        }

        Ok(())
    }
}

fn parse_operator_public_key(
    data: &[u8],
    operator_id: OperatorId,
    owner: Address,
) -> Result<Operator, String> {
    let data = unwrap_operator_public_key(data);
    let data = normalize_operator_public_key_bytes(data)?;
    Operator::new(&data, operator_id, owner)
        .map_err(|e| format!("Failed to construct operator: {e}"))
}

fn unwrap_operator_public_key(data: &[u8]) -> &[u8] {
    abi_decode_single_dynamic_bytes(data).unwrap_or(data)
}

fn normalize_operator_public_key_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    if let Some(pem_base64) = decode_hex_encoded_operator_pem(data)? {
        return Ok(pem_base64);
    }

    Ok(data.to_vec())
}

fn decode_hex_encoded_operator_pem(data: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let text = match std::str::from_utf8(data) {
        Ok(text) => text,
        Err(_) => return Ok(None),
    };
    let text = text.strip_prefix("0x").unwrap_or(text);

    if text.is_empty() || !text.len().is_multiple_of(2) {
        return Ok(None);
    }
    if !text.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        return Ok(None);
    }

    let pem = hex::decode(text)
        .map_err(|e| format!("Failed to decode hex-encoded operator public key: {e}"))?;
    if !pem.starts_with(b"-----BEGIN") {
        return Err("Hex-encoded operator public key did not decode to PEM".to_string());
    }

    Ok(Some(BASE64_STANDARD.encode(pem).into_bytes()))
}

fn abi_decode_single_dynamic_bytes(data: &[u8]) -> Option<&[u8]> {
    if data.len() < 64 || !data.len().is_multiple_of(32) {
        return None;
    }

    let offset = abi_word_to_usize(&data[..32])?;
    if offset != 32 {
        return None;
    }

    let len = abi_word_to_usize(&data[32..64])?;
    let padded_len = len.checked_add(31)?.checked_div(32)?.checked_mul(32)?;
    let end = 64usize.checked_add(len)?;
    if data.len() != 64 + padded_len || end > data.len() {
        return None;
    }

    Some(&data[64..end])
}

fn abi_word_to_usize(word: &[u8]) -> Option<usize> {
    if word.len() != 32 || word[..24].iter().any(|byte| *byte != 0) {
        return None;
    }

    usize::try_from(u64::from_be_bytes(word[24..].try_into().ok()?)).ok()
}

#[cfg(test)]
mod tests {
    use base64::Engine;

    use super::*;

    fn create_base64_operator_public_key() -> Vec<u8> {
        let rsa_key = database::test_utils::generators::pubkey::random_rsa();
        BASE64_STANDARD
            .encode(
                rsa_key
                    .public_key_to_pem()
                    .expect("Failed to serialize RSA public key"),
            )
            .into_bytes()
    }

    fn wrap_dynamic_bytes(data: &[u8]) -> Vec<u8> {
        let padded_len = data.len().div_ceil(32) * 32;
        let mut encoded = vec![0u8; 64 + padded_len];

        encoded[31] = 32;
        encoded[56..64].copy_from_slice(&(data.len() as u64).to_be_bytes());
        encoded[64..64 + data.len()].copy_from_slice(data);

        encoded
    }

    #[test]
    fn parse_operator_public_key_normalizes_wrapped_base64_and_hex_payloads() {
        // Arrange: encode the same PEM key once as wrapped base64 and once as wrapped ASCII hex.
        let base64_public_key = create_base64_operator_public_key();
        let pem_bytes = BASE64_STANDARD
            .decode(&base64_public_key)
            .expect("Failed to decode base64 operator key");
        let wrapped_base64 = wrap_dynamic_bytes(&base64_public_key);
        let wrapped_hex = wrap_dynamic_bytes(hex::encode(pem_bytes).as_bytes());

        // Act: parse both payload shapes through the operator-key normalization path.
        let base64_operator =
            parse_operator_public_key(&wrapped_base64, OperatorId(1), Address::random())
                .expect("Wrapped base64 operator key should parse");
        let hex_operator =
            parse_operator_public_key(&wrapped_hex, OperatorId(2), Address::random())
                .expect("Wrapped hex operator key should parse");

        // Assert: both payloads normalize to the same canonical RSA key.
        assert_eq!(
            base64_operator
                .rsa_pubkey
                .public_key_to_pem()
                .expect("Failed to serialize parsed base64 operator key"),
            hex_operator
                .rsa_pubkey
                .public_key_to_pem()
                .expect("Failed to serialize parsed hex operator key"),
        );
    }

    #[test]
    fn unwrap_operator_public_key_returns_original_bytes_for_non_abi_input() {
        // Arrange: build a plain base64 operator key without the outer ABI wrapper.
        let public_key = create_base64_operator_public_key();

        // Act/Assert: non-ABI input should pass through unchanged.
        assert_eq!(
            unwrap_operator_public_key(&public_key),
            public_key.as_slice()
        );
    }

    #[test]
    fn decode_hex_encoded_operator_pem_rejects_non_pem_hex_payload() {
        // Arrange: build hex text that decodes successfully but does not contain PEM bytes.
        let hex_payload = hex::encode("not a pem");

        // Act: attempt to normalize it as a hex-encoded operator key.
        let error = decode_hex_encoded_operator_pem(hex_payload.as_bytes())
            .expect_err("Non-PEM hex should be rejected");

        // Assert: the helper rejects it explicitly rather than silently accepting garbage.
        assert!(
            error.contains("did not decode to PEM"),
            "Unexpected error: {error}"
        );
    }

    #[test]
    fn decode_hex_encoded_operator_pem_accepts_0x_prefixed_hex_payload() {
        // Arrange: encode a valid PEM payload as hex and add the optional 0x prefix.
        let base64_public_key = create_base64_operator_public_key();
        let pem_bytes = BASE64_STANDARD
            .decode(&base64_public_key)
            .expect("Failed to decode base64 operator key");
        let hex_payload = format!("0x{}", hex::encode(pem_bytes));

        // Act: normalize the prefixed hex payload.
        let decoded = decode_hex_encoded_operator_pem(hex_payload.as_bytes())
            .expect("0x-prefixed PEM hex should parse")
            .expect("0x-prefixed PEM hex should normalize to base64 PEM");

        // Assert: the normalized bytes match the canonical base64 PEM form.
        assert_eq!(decoded, base64_public_key);
    }

    #[test]
    fn decode_hex_encoded_operator_pem_returns_none_for_non_hex_like_inputs() {
        // Arrange: gather malformed inputs that should be ignored as "not hex", not rejected.
        for input in [&b""[..], b"abc", b"zzzz", &[0xff, 0xfe]] {
            // Act/Assert: all of them should fall back to the non-hex path.
            assert!(
                decode_hex_encoded_operator_pem(input)
                    .expect("Malformed non-hex inputs should not error")
                    .is_none(),
                "Input {input:?} should not be treated as hex-encoded PEM"
            );
        }
    }

    #[test]
    fn abi_decode_single_dynamic_bytes_rejects_invalid_offset() {
        // Arrange: encode a valid payload, then corrupt the ABI offset word.
        let mut encoded = wrap_dynamic_bytes(b"test");
        encoded[31] = 0;
        encoded[30] = 64;

        // Act/Assert: non-standard offsets should be rejected.
        assert!(
            abi_decode_single_dynamic_bytes(&encoded).is_none(),
            "ABI payload with a non-standard offset should be rejected"
        );
    }

    #[test]
    fn abi_decode_single_dynamic_bytes_rejects_length_mismatch_buffer() {
        // Arrange: encode a short payload, then lie about the dynamic length word.
        let mut encoded = wrap_dynamic_bytes(b"test");
        encoded[56..64].copy_from_slice(&40u64.to_be_bytes());

        // Act/Assert: buffers whose declared length does not match the padded body are rejected.
        assert!(
            abi_decode_single_dynamic_bytes(&encoded).is_none(),
            "ABI payloads whose declared length does not match the padded buffer should be rejected"
        );
    }

    #[test]
    fn abi_word_to_usize_rejects_non_zero_high_bytes() {
        // Arrange: create a 32-byte ABI word with non-zero high-order bytes.
        let mut word = [0u8; 32];
        word[0] = 1;
        word[31] = 32;

        // Act/Assert: values that do not fit the narrow ABI decoding contract are rejected.
        assert!(
            abi_word_to_usize(&word).is_none(),
            "ABI words with non-zero high-order bytes should be rejected"
        );
    }
}
