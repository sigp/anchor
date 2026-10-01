//! Pre-consensus selection proofs for Boole+ `AggregatorCommittee` duties.
//!
//! Each operator sends one partial-signature message per `(committee, slot)` carrying a selection
//! proof for every attester duty and every distinct sync subnet of the validators it signs for.
//! Peers reject a second pre-consensus message from the same operator in a slot, so that one
//! message has to be complete when it is sent.
//!
//! The slot pipeline therefore starts one execution per committee when it publishes the slot's
//! [`VotingAssignments`], first insert wins. The execution signs the committee's complete worklist
//! once and retries only the send, until the selection deadline. What is sent does not depend on
//! which Lighthouse callbacks fire: the callbacks contribute their own share locally and wait for
//! reconstruction, and never send. This matches go-ssv's runner, which signs every selection proof
//! when the duty starts.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use database::UniqueIndex;
use fork::Fork;
use futures::{StreamExt, stream::FuturesUnordered};
use qbft_manager::ConsensusDecider;
use signature_collector::{
    CommitteeSelectionBatch, MAX_SELECTION_ROOTS_PER_VALIDATOR, ValidatorSigningData,
};
use slot_clock::SlotClock;
use ssv_types::{CommitteeId, ValidatorMetadata, typenum::Unsigned};
use tokio::time::{Instant, sleep_until};
use tracing::{debug, error, trace, warn};
use types::{Domain, EthSpec, Hash256, SignedRoot, Slot, SyncAggregatorSelectionData};

use crate::{AnchorValidatorStore, VotingAssignments, elapsed_in_slot};

/// Epochs to retain started `(committee, slot)` keys.
///
/// Matches the post-consensus registry and the message validator's per-signer duplicate window,
/// so republished assignments for a recent slot find their key instead of sending twice. After a
/// key is pruned, only a slot clock stepping backwards past the window could present its
/// `(committee, slot)` again before that slot's deadline.
const COMMITTEE_SELECTION_RETAIN_EPOCHS: u64 = 2;

/// Pause between attempts to sign or send. Sends fail while the processor queue is full at a slot
/// boundary, or while the node is not synced; each attempt reuses the same signed message.
const RETRY_INTERVAL: Duration = Duration::from_millis(250);

/// A committee's eligible validators with their selection roots, before signing keys are loaded.
type SelectionPlan = Vec<(ValidatorMetadata, Vec<Hash256>)>;

impl<T: SlotClock + 'static, E: EthSpec, C: ConsensusDecider<E> + 'static>
    AnchorValidatorStore<T, E, C>
{
    /// Starts the selection execution of every committee with work in `assignments` that has not
    /// already started for this slot.
    ///
    /// Called by the slot pipeline when it publishes the assignments. A committee only appears
    /// once per slot, so a repeated publication keeps the first worklist and cannot send twice,
    /// while a committee that first appears in a later publication still starts.
    pub(crate) fn start_committee_selections(self: &Arc<Self>, assignments: &VotingAssignments) {
        let slot = assignments.slot;
        if self
            .fork_schedule
            .active_fork(slot.epoch(E::slots_per_epoch()))
            < Fork::Boole
        {
            return;
        }
        let deadline = match self.get_instant_in_slot(slot, self.selection_proof_due()) {
            Ok(deadline) if deadline > Instant::now() => deadline,
            Ok(_) => {
                warn!(
                    %slot,
                    elapsed = ?elapsed_in_slot(&self.slot_clock, slot),
                    "Voting assignments published after the selection deadline, not sending \
                     committee selection batches"
                );
                return;
            }
            Err(err) => {
                error!(%slot, ?err, "Failed to compute the committee selection deadline");
                return;
            }
        };

        let plans = self.committee_selection_plans(assignments);
        let plans = {
            let mut started = self.committee_selections.lock();
            let cutoff =
                slot.saturating_sub(COMMITTEE_SELECTION_RETAIN_EPOCHS * E::slots_per_epoch());
            started.retain(|&(_, started_slot)| started_slot >= cutoff);
            plans
                .into_iter()
                .filter(|(committee_id, _)| started.insert((*committee_id, slot)))
                .collect::<Vec<_>>()
        };
        if plans.is_empty() {
            return;
        }

        let store = Arc::clone(self);
        self.task_executor.spawn(
            async move { store.run_committee_selections(slot, deadline, plans).await },
            "committee_selection",
        );
    }

    /// Groups this operator's validators that have a selection duty in `assignments` by
    /// committee, with one root per attester duty and per distinct sync subnet.
    ///
    /// An ineligible validator is left out entirely, never its whole committee: receivers accept a
    /// batch that omits entries, and one bad entry must not withhold every other validator's
    /// proofs.
    fn committee_selection_plans(
        &self,
        assignments: &VotingAssignments,
    ) -> HashMap<CommitteeId, SelectionPlan> {
        let slot = assignments.slot;
        let epoch = slot.epoch(E::slots_per_epoch());
        let attestation_root = slot.signing_root(self.get_domain(epoch, Domain::SelectionProof));
        let sync_domain = self.get_domain(epoch, Domain::SyncCommitteeSelectionProof);
        let attesting_indices = assignments
            .attesting_validators
            .iter()
            .copied()
            .collect::<HashSet<_>>();

        let state = self.database.state();
        let mut plans = HashMap::<CommitteeId, SelectionPlan>::new();
        for share in state.shares().values() {
            // Shares and metadata are inserted and removed together.
            let Some(validator) = state.metadata().get_by(&share.validator_pubkey) else {
                continue;
            };
            let attesting = assignments
                .attesting_committees
                .contains_key(&validator.public_key);
            let listed_attester = validator
                .index
                .is_some_and(|index| attesting_indices.contains(&index));
            let sync_subnets = validator
                .index
                .and_then(|index| assignments.sync_validators_by_subnet.get(&index));
            if !attesting && !listed_attester && sync_subnets.is_none() {
                continue;
            }

            let skip = |reason: &str| {
                warn!(
                    %slot,
                    validator_pubkey = %validator.public_key,
                    validator_index = ?validator.index,
                    reason,
                    "Skipping validator in committee selection batch"
                )
            };
            // A newly registered validator may hold an attester duty before its index is known.
            if validator.index.is_none() {
                skip("missing validator index");
                continue;
            }
            if attesting != listed_attester {
                skip("inconsistent attester assignment");
                continue;
            }
            let mut roots = Vec::with_capacity(MAX_SELECTION_ROOTS_PER_VALIDATOR);
            if attesting {
                roots.push(attestation_root);
            }
            if let Some(subnets) = sync_subnets {
                if subnets.is_empty()
                    || subnets.iter().any(|(subnet, count)| {
                        u64::from(*subnet) >= E::SyncCommitteeSubnetCount::to_u64() || *count == 0
                    })
                {
                    skip("invalid sync subnet assignment");
                    continue;
                }
                roots.extend(subnets.keys().map(|subnet| {
                    SyncAggregatorSelectionData {
                        slot,
                        subcommittee_index: (*subnet).into(),
                    }
                    .signing_root(sync_domain)
                }));
            }

            let Some(cluster) = state.clusters().get_by(&validator.cluster_id) else {
                skip("unknown cluster");
                continue;
            };
            if cluster.liquidated {
                // Lighthouse keeps a liquidated validator's sync duty for the rest of the period,
                // so this recurs every slot and is expected.
                debug!(
                    %slot,
                    validator_pubkey = %validator.public_key,
                    validator_index = ?validator.index,
                    reason = "cluster liquidated",
                    "Skipping validator in committee selection batch"
                );
                continue;
            }
            plans
                .entry(cluster.committee_id())
                .or_default()
                .push((validator.clone(), roots));
        }
        plans
    }

    /// Loads the planned signing keys, then signs and sends each committee's batch.
    async fn run_committee_selections(
        self: Arc<Self>,
        slot: Slot,
        deadline: Instant,
        plans: Vec<(CommitteeId, SelectionPlan)>,
    ) {
        // Decrypting key shares can be slow on a cold cache, so load them off the async runtime.
        // The blocking work cannot be cancelled, so it is awaited and the deadline checked after.
        let committees = plans.len();
        let store = Arc::clone(&self);
        let Some(loading) = self.task_executor.spawn_blocking_handle(
            move || store.load_committee_selection_batches(slot, plans),
            "committee_selection_keys",
        ) else {
            return;
        };
        let batches = match loading.await {
            Ok(batches) => batches,
            Err(err) => {
                error!(%slot, committees, ?err, "Committee selection key loading task failed");
                return;
            }
        };
        if Instant::now() >= deadline {
            error!(
                %slot,
                committees,
                "Committee selection signing keys loaded after the selection deadline, not sending"
            );
            return;
        }

        let mut sends = batches
            .into_iter()
            .map(|batch| self.send_committee_selection(batch, deadline))
            .collect::<FuturesUnordered<_>>();
        while sends.next().await.is_some() {}
    }

    /// Signs one committee's batch once, then offers that same message until it is admitted or
    /// the selection deadline passes.
    async fn send_committee_selection(
        &self,
        batch: Arc<CommitteeSelectionBatch>,
        deadline: Instant,
    ) {
        let slot = batch.slot();
        let committee_id = batch.committee_id();
        let entries = batch.signing_data().len();

        let message = loop {
            match self
                .signature_collector
                .sign_committee_selection(Arc::clone(&batch))
                .await
            {
                Ok(message) => break message,
                Err(err) => {
                    let retry_at = Instant::now() + RETRY_INTERVAL;
                    if retry_at >= deadline {
                        error!(
                            %slot,
                            ?committee_id,
                            entries,
                            ?err,
                            "Committee selection batch not signed before the selection deadline"
                        );
                        return;
                    }
                    debug!(%slot, ?committee_id, ?err, "Failed to sign committee selection batch, retrying");
                    sleep_until(retry_at).await;
                }
            }
        };

        // Signing may finish, and a retry may wake, after the deadline under load; nothing is sent
        // from then on.
        let mut attempts = 0_u32;
        let mut last_err = None;
        while Instant::now() < deadline {
            attempts += 1;
            match self
                .signature_collector
                .send_committee_message(message.clone(), committee_id)
            {
                Ok(()) => {
                    // Admission queues the message; signing and publication happen later.
                    if attempts > 1 {
                        debug!(
                            %slot,
                            ?committee_id,
                            entries,
                            attempts,
                            "Admitted committee selection batch after retries"
                        );
                    } else {
                        trace!(%slot, ?committee_id, entries, "Admitted committee selection batch");
                    }
                    return;
                }
                Err(err) => {
                    trace!(
                        %slot,
                        ?committee_id,
                        attempts,
                        ?err,
                        "Failed to admit committee selection batch, retrying"
                    );
                    last_err = Some(err);
                    sleep_until(deadline.min(Instant::now() + RETRY_INTERVAL)).await;
                }
            }
        }
        match last_err {
            None => error!(
                %slot,
                ?committee_id,
                entries,
                "Committee selection batch signed after the selection deadline, not sending"
            ),
            Some(err) => error!(
                %slot,
                ?committee_id,
                entries,
                attempts,
                ?err,
                "Committee selection batch not admitted before the selection deadline"
            ),
        }
    }

    /// Loads each planned validator's signing key and builds one validated batch per committee.
    ///
    /// A validator whose key cannot be loaded is left out of its committee's batch.
    fn load_committee_selection_batches(
        &self,
        slot: Slot,
        plans: Vec<(CommitteeId, SelectionPlan)>,
    ) -> Vec<Arc<CommitteeSelectionBatch>> {
        plans
            .into_iter()
            .filter_map(|(committee_id, validators)| {
                let mut entries = Vec::new();
                for (validator, roots) in validators {
                    match self.validator_signing_data(&validator, roots[0]) {
                        Ok(data) => {
                            entries.extend(roots.into_iter().map(|root| ValidatorSigningData {
                                root,
                                ..data.clone()
                            }))
                        }
                        Err(err) => warn!(
                            %slot,
                            validator_pubkey = %validator.public_key,
                            validator_index = ?validator.index,
                            reason = "signing key unavailable",
                            ?err,
                            "Skipping validator in committee selection batch"
                        ),
                    }
                }
                if entries.is_empty() {
                    return None;
                }
                let entry_count = entries.len();
                CommitteeSelectionBatch::new(slot, committee_id, entries)
                    .inspect_err(|err| {
                        error!(
                            %slot,
                            ?committee_id,
                            entries = entry_count,
                            ?err,
                            "Invalid committee selection batch"
                        )
                    })
                    .ok()
                    .map(Arc::new)
            })
            .collect()
    }
}
