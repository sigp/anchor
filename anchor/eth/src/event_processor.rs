use std::sync::Arc;

use alloy::{
    primitives::{Address, B256},
    rpc::types::Log,
    sol_types::SolEvent,
};
use bls::PublicKeyBytes;
use database::{NetworkDatabase, ProcessedEventCursor, SlashingProtection, UniqueIndex};
use ssv_types::{ClusterId, Operator, OperatorId, ValidatorIndex};
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

enum EventActionError {
    Skippable(ExecutionError),
    SkippableCommitted(ExecutionError),
    Fatal(ExecutionError),
}

// Only validator add/remove events feed the high-level processing counters we log at the end of a
// batch. Everything else still mutates state, but does not affect these summary metrics.
#[derive(Default)]
struct ProcessingStats {
    validators_added: usize,
    validators_removed: usize,
}

impl ProcessingStats {
    fn from_counted_event(event: CountedEvent) -> Self {
        match event {
            CountedEvent::ValidatorAdded => Self {
                validators_added: 1,
                validators_removed: 0,
            },
            CountedEvent::ValidatorRemoved => Self {
                validators_added: 0,
                validators_removed: 1,
            },
            CountedEvent::Other => Self::default(),
        }
    }

    fn merge(&mut self, other: Self) {
        self.validators_added += other.validators_added;
        self.validators_removed += other.validators_removed;
    }
}

#[derive(Clone, Copy)]
enum CountedEvent {
    Other,
    ValidatorAdded,
    ValidatorRemoved,
}

impl EventProcessor {
    /// Construct a new EventProcessor
    pub fn new(db: Arc<NetworkDatabase>, mode: Mode) -> Self {
        Self { db, mode }
    }

    /// Process one fetched range of logs.
    ///
    /// This first drops any logs already covered by the committed in-block cursor, then processes
    /// the remaining logs sequentially and finally collapses progress back to a fully processed
    /// block if the whole range succeeded.
    #[instrument(skip(self, logs), fields(logs_count = logs.len()), level = "debug")]
    pub fn process_logs(
        &self,
        logs: Vec<Log>,
        live: bool,
        end_block: u64,
    ) -> Result<(), ExecutionError> {
        debug!(logs_count = logs.len(), "Starting log processing");
        let timer = metrics::start_timer(&metrics::EXECUTION_LOG_PROCESSING_TIME);
        let logs = self.skip_processed_logs(logs);
        let result = self.process_logs_inner(&logs, live, end_block);

        metrics::stop_timer(timer);

        let stats = result?;

        Self::log_processed_counts(&stats);

        debug!(logs_count = logs.len(), "Completed processing logs");
        Ok(())
    }

    /// Process the fetched range sequentially and collapse partial progress only if the whole range
    /// succeeded.
    fn process_logs_inner(
        &self,
        logs: &[Log],
        live: bool,
        end_block: u64,
    ) -> Result<ProcessingStats, ExecutionError> {
        let mut stats = ProcessingStats::default();

        for (index, log) in logs.iter().enumerate() {
            stats.merge(self.process_single_log(log, live, index)?);
        }

        self.advance_processed_block_if_needed(end_block)?;

        Ok(stats)
    }

    /// Process exactly one log by deriving its durable cursor, dispatching to the right handler,
    /// and then finalizing cursor ownership.
    fn process_single_log(
        &self,
        log: &Log,
        live: bool,
        log_index: usize,
    ) -> Result<ProcessingStats, ExecutionError> {
        trace!(log_index, topic = ?log.topic0(), "Processing individual log");

        let cursor = Self::cursor_for_log(log)?;
        let Some(topic0) = log.topic0().copied() else {
            self.mark_event_processed(cursor)?;
            warn!("Log missing topic0, skipping");
            return Ok(ProcessingStats::default());
        };

        let Some((event, result)) = self.dispatch_log(topic0, log, live, cursor) else {
            self.mark_event_processed(cursor)?;
            debug!(?topic0, "Unknown event signature, skipping");
            return Ok(ProcessingStats::default());
        };

        self.finish_processed_log(log, live, cursor, event, result)
    }

    /// Decode the event signature and delegate to the matching handler.
    ///
    /// Each handler is responsible for durably recording its own cursor before returning success.
    fn dispatch_log(
        &self,
        topic0: B256,
        log: &Log,
        live: bool,
        cursor: ProcessedEventCursor,
    ) -> Option<(CountedEvent, Result<(), EventActionError>)> {
        Some(match topic0 {
            SSVContract::OperatorAdded::SIGNATURE_HASH => (
                CountedEvent::Other,
                self.process_operator_added(log, cursor),
            ),
            SSVContract::OperatorRemoved::SIGNATURE_HASH => (
                CountedEvent::Other,
                self.process_operator_removed(log, cursor),
            ),
            SSVContract::ValidatorAdded::SIGNATURE_HASH => (
                CountedEvent::ValidatorAdded,
                self.process_validator_added(log, cursor),
            ),
            SSVContract::ValidatorRemoved::SIGNATURE_HASH => (
                CountedEvent::ValidatorRemoved,
                self.process_validator_removed(log, cursor),
            ),
            SSVContract::ClusterLiquidated::SIGNATURE_HASH => (
                CountedEvent::Other,
                self.process_cluster_liquidated(log, cursor),
            ),
            SSVContract::ClusterReactivated::SIGNATURE_HASH => (
                CountedEvent::Other,
                self.process_cluster_reactivated(log, cursor),
            ),
            SSVContract::FeeRecipientAddressUpdated::SIGNATURE_HASH => (
                CountedEvent::Other,
                self.process_fee_recipient_updated(log, cursor),
            ),
            SSVContract::ValidatorExited::SIGNATURE_HASH => (
                CountedEvent::Other,
                self.process_validator_exited(log, live, cursor),
            ),
            _ => return None,
        })
    }

    /// Handle post-handler bookkeeping for one processed log.
    ///
    /// By the time control reaches this function, successful handlers have already durably
    /// recorded their exact cursor.
    fn finish_processed_log(
        &self,
        log: &Log,
        live: bool,
        cursor: ProcessedEventCursor,
        event: CountedEvent,
        result: Result<(), EventActionError>,
    ) -> Result<ProcessingStats, ExecutionError> {
        match result {
            Ok(()) => Ok(ProcessingStats::from_counted_event(event)),
            Err(EventActionError::Skippable(err)) => {
                self.mark_event_processed(cursor)?;
                Self::log_skipped_event(log, live, &err);
                Ok(ProcessingStats::default())
            }
            Err(EventActionError::SkippableCommitted(err)) => {
                Self::log_skipped_event(log, live, &err);
                Ok(ProcessingStats::default())
            }
            Err(EventActionError::Fatal(err)) => Err(err),
        }
    }

    /// Persist cursor-only progress for one processed or intentionally skipped log.
    fn mark_event_processed(&self, cursor: ProcessedEventCursor) -> Result<(), ExecutionError> {
        self.db
            .mark_event_processed(cursor)
            .map_err(|e| ExecutionError::Database(e.to_string()))
    }

    /// Collapse a partial in-block cursor back to a fully processed block once the whole fetched
    /// range completed successfully.
    ///
    /// The processed block boundary is monotonic, so an unexpected older `end_block` is ignored
    /// instead of silently regressing durable progress.
    fn advance_processed_block_if_needed(&self, end_block: u64) -> Result<(), ExecutionError> {
        let (current_block, has_partial_cursor) = self.db.with_state(|state| {
            (
                state.get_last_processed_block(),
                state.get_last_processed_event().is_some(),
            )
        });

        if end_block < current_block {
            return Ok(());
        }

        let needs_advance = current_block != end_block || has_partial_cursor;

        if needs_advance {
            self.db
                .advance_processed_block(end_block)
                .map_err(|e| ExecutionError::Database(e.to_string()))?;
        }

        Ok(())
    }

    fn log_processed_counts(stats: &ProcessingStats) {
        if stats.validators_added > 0 {
            debug!(count = stats.validators_added, "Added validators");
        }
        if stats.validators_removed > 0 {
            debug!(count = stats.validators_removed, "Removed validators");
        }
    }

    /// Drop logs that were already durably committed inside the current partial block.
    fn skip_processed_logs(&self, logs: Vec<Log>) -> Vec<Log> {
        let Some(cursor) = self.db.with_state(|state| state.get_last_processed_event()) else {
            return logs;
        };

        logs.into_iter()
            .filter(|log| !Self::log_at_or_before_cursor(log, cursor))
            .collect()
    }

    /// Return `true` when a log falls at or before the committed in-block resume cursor.
    fn log_at_or_before_cursor(log: &Log, cursor: ProcessedEventCursor) -> bool {
        let Some(block_number) = log.block_number else {
            return false;
        };
        let Some(transaction_index) = log.transaction_index else {
            return false;
        };
        let Some(log_index) = log.log_index else {
            return false;
        };

        if block_number != cursor.block_number {
            return block_number < cursor.block_number;
        }

        (transaction_index, log_index) <= (cursor.transaction_index, cursor.log_index)
    }

    /// Build the durable processed-event cursor for one execution log.
    fn cursor_for_log(log: &Log) -> Result<ProcessedEventCursor, ExecutionError> {
        let block_number = log
            .block_number
            .ok_or_else(|| ExecutionError::InvalidEvent("Log missing block_number".to_string()))?;
        let transaction_index = log.transaction_index.ok_or_else(|| {
            ExecutionError::InvalidEvent("Log missing transaction_index".to_string())
        })?;
        let log_index = log
            .log_index
            .ok_or_else(|| ExecutionError::InvalidEvent("Log missing log_index".to_string()))?;

        Ok(ProcessedEventCursor {
            block_number,
            transaction_index,
            log_index,
        })
    }

    /// Log one intentionally skipped event at a level appropriate for historical vs live sync.
    fn log_skipped_event(log: &Log, live: bool, error: &ExecutionError) {
        let tx_hash = log
            .transaction_hash
            .map(|hash| hash.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        if live {
            warn!(tx_hash, "Malformed event: {error}");
        } else {
            trace!(tx_hash, "Malformed event: {error}");
        }
    }

    /// Handle one `OperatorAdded` log.
    ///
    /// Duplicate or malformed operator events are skippable, but may still need to advance
    /// `max_operator_id_seen` so later valid operator ids are not blocked forever.
    fn process_operator_added(
        &self,
        log: &Log,
        cursor: ProcessedEventCursor,
    ) -> Result<(), EventActionError> {
        // Destructure operator added event
        let SSVContract::OperatorAdded {
            operatorId, // The ID of the newly registered operator
            owner,      // The EOA owner address
            publicKey,  // The RSA public key
            ..
        } = SSVContract::OperatorAdded::decode_from_log(log)
            .map_err(EventActionError::Skippable)?;
        let operator_id = OperatorId(operatorId);

        trace!(operator_id = ?operator_id, owner = ?owner, "Processing operator added");

        // Confirm that this operator does not already exist
        if self
            .db
            .with_state(|state| state.operator_exists(&operator_id))
        {
            return Err(EventActionError::Skippable(ExecutionError::Duplicate(
                format!("Operator with id {operator_id:?} already exists in database"),
            )));
        }

        let max_seen = self.db.with_state(|state| state.get_max_operator_id_seen());

        // Only check for missing operators if we have a previous max (not a migrated database)
        if let Some(max_seen) = max_seen
            && max_seen != operatorId - 1
        {
            return Err(EventActionError::Skippable(ExecutionError::InvalidEvent(
                format!(
                    "Missing OperatorAdded events: database has only seen up to id {max_seen}, \
                but got operator {operator_id}."
                ),
            )));
        }

        let skip_with_seen_operator = |err| {
            self.db
                .commit_seen_operator_id(operatorId, cursor)
                .map_err(|e| {
                    EventActionError::Fatal(ExecutionError::Database(format!(
                        "Failed to persist max seen operator id: {e}"
                    )))
                })?;
            Err(EventActionError::SkippableCommitted(err))
        };

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
        let operator = match Operator::new(data, operator_id, owner) {
            Ok(operator) => operator,
            Err(e) => {
                debug!(
                    operator_pubkey = ?publicKey,
                    operator_id = ?operator_id,
                    error = %e,
                    "Failed to construct operator"
                );
                return skip_with_seen_operator(ExecutionError::InvalidEvent(format!(
                    "Failed to construct operator: {e}"
                )));
            }
        };
        if let Err(e) = self.db.commit_operator_added(&operator, operatorId, cursor) {
            if e.to_string()
                .contains("UNIQUE constraint failed: operators.public_key")
            {
                return skip_with_seen_operator(ExecutionError::InvalidEvent(format!(
                    "Failed to insert operator into database: {e}"
                )));
            }
            debug!(
                operator_id = ?operator_id,
                error = %e,
                "Failed to insert operator into database"
            );
            return Err(EventActionError::Fatal(ExecutionError::Database(format!(
                "Failed to insert operator into database: {e}"
            ))));
        }

        debug!(
            operator_id = ?operator_id,
            owner = ?owner,
            "Successfully registered operator"
        );
        metrics::inc_counter_vec(&metrics::EXECUTION_EVENTS_PROCESSED, &["operator_added"]);
        Ok(())
    }

    /// Handle one `OperatorRemoved` log.
    fn process_operator_removed(
        &self,
        log: &Log,
        cursor: ProcessedEventCursor,
    ) -> Result<(), EventActionError> {
        // Extract the ID of the Operator
        let SSVContract::OperatorRemoved { operatorId } =
            SSVContract::OperatorRemoved::decode_from_log(log)
                .map_err(EventActionError::Skippable)?;
        let operator_id = OperatorId(operatorId);
        trace!(operator_id = ?operator_id, "Processing operator removed");

        // Delete the operator from database and in memory
        self.db
            .commit_operator_removed(operator_id, cursor)
            .map_err(|e| {
                debug!(
                    operator_id = ?operator_id,
                    error = %e,
                    "Failed to remove operator"
                );
                EventActionError::Fatal(ExecutionError::Database(format!(
                    "Failed to remove operator: {e}"
                )))
            })?;

        debug!(operator_id = ?operatorId, "Operator removed from network");
        metrics::inc_counter_vec(&metrics::EXECUTION_EVENTS_PROCESSED, &["operator_removed"]);
        Ok(())
    }

    /// Handle one `ValidatorAdded` log.
    ///
    /// Malformed validator-add events still consume the owner nonce on-chain, so the main
    /// skippable path for this handler is "commit nonce + cursor, but do not insert validator
    /// rows". Successful events also register slashing protection before the main DB commit.
    fn process_validator_added(
        &self,
        log: &Log,
        cursor: ProcessedEventCursor,
    ) -> Result<(), EventActionError> {
        // Parse and destructure log
        let SSVContract::ValidatorAdded {
            owner,
            operatorIds,
            publicKey,
            shares,
            ..
        } = SSVContract::ValidatorAdded::decode_from_log(log)
            .map_err(EventActionError::Skippable)?;
        trace!(owner = ?owner, operator_count = operatorIds.len(), "Processing validator addition");

        let nonce = self.db.with_state(|state| state.get_next_nonce(&owner));

        let skip_with_nonce = |err| {
            self.db.commit_owner_nonce(owner, cursor).map_err(|e| {
                EventActionError::Fatal(ExecutionError::Database(format!(
                    "Failed to bump nonce: {e}"
                )))
            })?;
            Err(EventActionError::SkippableCommitted(err))
        };

        // During keysplitting, we only care about the nonce
        let Mode::Node {
            index_sync_tx: index_lookup_queue,
            slashing_protection,
            ..
        } = &self.mode
        else {
            self.db.commit_owner_nonce(owner, cursor).map_err(|e| {
                EventActionError::Fatal(ExecutionError::Database(format!(
                    "Failed to bump nonce: {e}"
                )))
            })?;
            return Ok(());
        };

        // Process data into a usable form
        let validator_pubkey = match parse_validator_pubkey(&publicKey) {
            Ok(value) => value,
            Err(err) => return skip_with_nonce(err),
        };

        // Duplicate ValidatorAdded logs still consume the owner nonce on-chain. Treat them as
        // malformed input to skip instead of letting the SQL unique constraint abort sync.
        let validator_exists = self
            .db
            .with_state(|state| state.metadata().get_by(&validator_pubkey).is_some());
        if validator_exists {
            return skip_with_nonce(ExecutionError::Duplicate(format!(
                "Validator with public key {validator_pubkey} already exists in database"
            )));
        }
        let cluster_id = compute_cluster_id(owner, &operatorIds);
        let operator_ids: Vec<_> = operatorIds.into_iter().map(OperatorId).collect();

        // Perform verification on the operator set and make sure they are all registered in the
        // network
        let operators_valid = self
            .db
            .with_state(|state| validate_operators(&operator_ids, &cluster_id, state));
        if let Err(err) = operators_valid {
            return skip_with_nonce(err);
        }

        // Parse the share byte stream into a list of valid Shares and then verify the signature
        trace!(cluster_id = ?cluster_id, "Parsing and verifying shares");
        let (signature, shares) =
            match parse_shares(&shares, &operator_ids, &cluster_id, &validator_pubkey) {
                Ok(parsed) => parsed,
                Err(e) => {
                    debug!(cluster_id = ?cluster_id, error = %e, "Failed to parse shares");
                    return skip_with_nonce(ExecutionError::InvalidEvent(format!(
                        "Failed to parse shares. {e}"
                    )));
                }
            };

        if !verify_signature(signature, nonce, &owner, &validator_pubkey) {
            debug!(cluster_id = ?cluster_id, "Signature verification failed");
            return skip_with_nonce(ExecutionError::InvalidEvent(
                "Signature verification failed".to_string(),
            ));
        }

        // Fetch the validator metadata
        let validator_metadata = match construct_validator_metadata(&validator_pubkey, &cluster_id)
        {
            Ok(metadata) => metadata,
            Err(e) => {
                debug!(validator_pubkey= ?validator_pubkey, "Failed to fetch validator metadata");
                return skip_with_nonce(ExecutionError::InvalidEvent(format!(
                    "Failed to fetch validator metadata: {e}"
                )));
            }
        };

        // `fee_recipient` is not required to persist the validator. It lives in `owners` and is
        // only needed when materializing the full cluster view after the write commits.

        // First, do the slashing protection database...
        //
        // This happens before the main DB commit on purpose: we would rather fail before
        // persisting the validator than risk later signing without slashing protection. Crash
        // safety therefore relies on `register_validator` being idempotent; on replay it must be
        // safe to call again before the main DB cursor is committed.
        slashing_protection
            .register_validator(validator_pubkey)
            .map_err(|e| {
                EventActionError::Fatal(ExecutionError::Database(format!(
                    "Failed to insert validator into slashing db: {e}"
                )))
            })?;

        // ...then the main database.
        self.db
            .commit_validator_added(cluster_id, owner, validator_metadata, shares, cursor)
            .map_err(|e| {
                debug!(cluster_id = ?cluster_id, error = %e, "Failed to insert validator into cluster");
                EventActionError::Fatal(ExecutionError::Database(format!(
                    "Failed to insert validator into cluster: {e}"
                )))
            })?;

        // Schedule validator for index lookup
        if let Err(err) = index_lookup_queue.send(validator_pubkey) {
            error!(?err, "Failed to send validator to index lookup");
        }

        trace!(
            cluster_id = ?cluster_id,
            validator_pubkey = %validator_pubkey,
            "Successfully added validator"
        );
        metrics::inc_counter_vec(&metrics::EXECUTION_EVENTS_PROCESSED, &["validator_added"]);
        Ok(())
    }

    /// Handle one `ValidatorRemoved` log.
    fn process_validator_removed(
        &self,
        log: &Log,
        cursor: ProcessedEventCursor,
    ) -> Result<(), EventActionError> {
        // Parse and destructure log
        let SSVContract::ValidatorRemoved {
            owner,
            operatorIds,
            publicKey,
            ..
        } = SSVContract::ValidatorRemoved::decode_from_log(log)
            .map_err(EventActionError::Skippable)?;
        trace!(owner = ?owner, public_key = ?publicKey, "Processing Validator Removed");

        // Parse the public key
        let validator_pubkey =
            parse_validator_pubkey(&publicKey).map_err(EventActionError::Skippable)?;

        // Compute the cluster id
        let cluster_id = compute_cluster_id(owner, &operatorIds);

        let metadata = match self
            .db
            .with_state(|state| state.metadata().get_by(&validator_pubkey).cloned())
        {
            Some(data) => data,
            None => {
                debug!(
                    cluster_id = ?cluster_id,
                    "Failed to fetch validator metadata from database"
                );
                return Err(EventActionError::Skippable(ExecutionError::InvalidEvent(
                    "Failed to fetch validator metadata from database".to_string(),
                )));
            }
        };

        // Get the cluster that this validator is in
        let cluster = match self
            .db
            .with_state(|state| state.clusters().get_by(&validator_pubkey).cloned())
        {
            Some(data) => data,
            None => {
                debug!(
                    cluster_id = ?cluster_id,
                    "Failed to fetch cluster from database"
                );
                return Err(EventActionError::Skippable(ExecutionError::InvalidEvent(
                    "Failed to fetch cluster from database".to_string(),
                )));
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
            return Err(EventActionError::Skippable(ExecutionError::InvalidEvent(
                format!(
                    "Cluster already exists with a different owner address. Expected {}. Got {}",
                    cluster.owner, owner
                ),
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
            return Err(EventActionError::Skippable(ExecutionError::InvalidEvent(
                "Validator does not match".to_string(),
            )));
        }
        // Remove the validator and all corresponding cluster data
        self.db
            .commit_validator_removed(validator_pubkey, cursor)
            .map_err(|e| {
                debug!(
                    cluster_id = ?cluster_id,
                    pubkey = ?validator_pubkey,
                    error = %e,
                    "Failed to delete validator from database"
                );
                EventActionError::Fatal(ExecutionError::Database(format!(
                    "Failed to validator cluster: {e}"
                )))
            })?;

        trace!(
            cluster_id = ?cluster_id,
            validator_pubkey = %validator_pubkey,
            "Successfully removed validator and cluster"
        );
        metrics::inc_counter_vec(&metrics::EXECUTION_EVENTS_PROCESSED, &["validator_removed"]);
        Ok(())
    }

    /// Handle one `ClusterLiquidated` log by committing the new cluster status plus cursor.
    fn process_cluster_liquidated(
        &self,
        log: &Log,
        cursor: ProcessedEventCursor,
    ) -> Result<(), EventActionError> {
        let SSVContract::ClusterLiquidated {
            owner,
            operatorIds: operator_ids,
            ..
        } = SSVContract::ClusterLiquidated::decode_from_log(log)
            .map_err(EventActionError::Skippable)?;

        let cluster_id = compute_cluster_id(owner, &operator_ids);

        trace!(cluster_id = ?cluster_id, "Processing cluster liquidation");

        // Update the status of the cluster to be liquidated
        self.db
            .commit_cluster_status(cluster_id, true, cursor)
            .map_err(|e| {
                debug!(
                    cluster_id = ?cluster_id,
                    error = %e,
                    "Failed to mark cluster as liquidated"
                );
                EventActionError::Fatal(ExecutionError::Database(format!(
                    "Failed to mark cluster as liquidated: {e}"
                )))
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

    /// Handle one `ClusterReactivated` log by committing the new cluster status plus cursor.
    fn process_cluster_reactivated(
        &self,
        log: &Log,
        cursor: ProcessedEventCursor,
    ) -> Result<(), EventActionError> {
        let SSVContract::ClusterReactivated {
            owner,
            operatorIds: operator_ids,
            ..
        } = SSVContract::ClusterReactivated::decode_from_log(log)
            .map_err(EventActionError::Skippable)?;

        let cluster_id = compute_cluster_id(owner, &operator_ids);

        trace!(cluster_id = ?cluster_id, "Processing cluster reactivation");

        // Update the status of the cluster to be active
        self.db
            .commit_cluster_status(cluster_id, false, cursor)
            .map_err(|e| {
                debug!(
                    cluster_id = ?cluster_id,
                    error = %e,
                    "Failed to mark cluster as active"
                );
                EventActionError::Fatal(ExecutionError::Database(format!(
                    "Failed to mark cluster as active: {e}"
                )))
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

    /// Handle one `FeeRecipientAddressUpdated` log.
    fn process_fee_recipient_updated(
        &self,
        log: &Log,
        cursor: ProcessedEventCursor,
    ) -> Result<(), EventActionError> {
        let SSVContract::FeeRecipientAddressUpdated {
            owner,
            recipientAddress,
        } = SSVContract::FeeRecipientAddressUpdated::decode_from_log(log)
            .map_err(EventActionError::Skippable)?;
        // update the fee recipient address in the database
        self.db
            .commit_fee_recipient_updated(owner, recipientAddress, cursor)
            .map_err(|e| {
                debug!(
                    owner = ?owner,
                    error = %e,
                    "Failed to update fee recipient"
                );
                EventActionError::Fatal(ExecutionError::Database(format!(
                    "Failed to update fee recipient: {e}"
                )))
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

    /// Handle one `ValidatorExited` log.
    ///
    /// This event does not mutate Anchor's durable validator state. It either queues exit work or
    /// intentionally ignores the event, and in both cases records cursor-only progress directly in
    /// this handler.
    fn process_validator_exited(
        &self,
        log: &Log,
        live: bool,
        cursor: ProcessedEventCursor,
    ) -> Result<(), EventActionError> {
        // In KeySplit mode, we don't need to process validator exits
        let Mode::Node { exit_tx, .. } = &self.mode else {
            self.mark_event_processed(cursor)
                .map_err(EventActionError::Fatal)?;
            return Ok(());
        };
        let SSVContract::ValidatorExited {
            owner,
            operatorIds,
            publicKey,
        } = SSVContract::ValidatorExited::decode_from_log(log)
            .map_err(EventActionError::Skippable)?;

        let validator_pubkey =
            parse_validator_pubkey(&publicKey).map_err(EventActionError::Skippable)?;
        let computed_cluster_id = compute_cluster_id(owner, &operatorIds);

        self.verify_validator_owner(&owner, &validator_pubkey, &computed_cluster_id)
            .map_err(EventActionError::Skippable)?;

        let operator_ids: Vec<OperatorId> = operatorIds.iter().map(|id| OperatorId(*id)).collect();

        // Perform verification on the operator set and make sure they are all registered in the
        // network
        self.db
            .with_state(|state| validate_operators(&operator_ids, &computed_cluster_id, state))
            .map_err(EventActionError::Skippable)?;

        let block_timestamp = log.block_timestamp.ok_or_else(|| {
            EventActionError::Skippable(ExecutionError::InvalidEvent(
                "Block timestamp not set".to_string(),
            ))
        })?;

        let validator_index = match self.get_validator_index(&validator_pubkey) {
            Ok(Some(value)) => value,
            Ok(None) => {
                self.mark_event_processed(cursor)
                    .map_err(EventActionError::Fatal)?;
                return Ok(());
            }
            Err(value) => return Err(EventActionError::Skippable(value)),
        };

        let is_our_validator = self.is_our_validator(&validator_pubkey);

        if !live {
            if is_our_validator {
                debug!(
                    %validator_index,
                    "Ignoring historic validator exit for validator assigned to us"
                );
            } else {
                trace!(%validator_index, "Ignoring historic validator exit");
            }
            self.mark_event_processed(cursor)
                .map_err(EventActionError::Fatal)?;
            return Ok(());
        }

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
                return Err(EventActionError::Fatal(ExecutionError::Misc(
                    "Failed to send validator exit request to processor".to_string(),
                )));
            }
        }

        self.mark_event_processed(cursor)
            .map_err(EventActionError::Fatal)?;

        Ok(())
    }

    /// Return `true` if the current operator holds a share for this validator.
    fn is_our_validator(&self, validator_pubkey: &PublicKeyBytes) -> bool {
        self.db
            .with_state(|state| state.shares().get_by(validator_pubkey).is_some())
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
        let validator_metadata = match self
            .db
            .with_state(|state| state.metadata().get_by(validator_pubkey).cloned())
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
    /// If the cluster is already liquidated, the function will return `Ok(())` but issue a warning.
    fn verify_validator_owner(
        &self,
        owner: &Address,
        validator_pubkey: &PublicKeyBytes,
        computed_cluster_id: &ClusterId,
    ) -> Result<(), ExecutionError> {
        // Get the cluster for this validator to access owner information
        let cluster = match self
            .db
            .with_state(|state| state.clusters().get_by(validator_pubkey).cloned())
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
            return Err(ExecutionError::InvalidEvent(
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
