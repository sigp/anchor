//! Post-consensus signing for Boole+ `AggregatorCommittee` duties.
//!
//! One QBFT decision carries both attestation aggregates and sync contributions for an SSV
//! committee, and the operator must emit exactly one committee partial-signature message per
//! `(committee, slot)` covering everything it can sign. The decided value drives both what gets
//! signed and what gets published: Anchor owns Boole+ publication of both classes outright,
//! through the metadata service's publisher, because the protocol's unit of agreement is the
//! decided value and no local Lighthouse view is part of it. Lighthouse's duty snapshots are
//! cloned before slot-start selection proofs finish (both classes share one slot-start
//! partial-signature batch), so routing publication through either callback would silently skip
//! late-installed proofs, and a callback may not fire at all (issue #1227).
//!
//! The signing set is therefore a property of the duty, not of any consumer. The slot pipeline
//! starts one execution per committee when it publishes the decided value at 2/3 slot, and the
//! publisher drains that execution's roots of both classes itself; the Lighthouse callbacks
//! return empty batches. This is the ssv-spec-normative shape, where the decided value drives
//! what gets signed and published, matching go-ssv's runner-owned post-consensus submission.

use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    future::Future,
    sync::{Arc, LazyLock},
};

use bls::Signature;
use database::{NonUniqueIndex, UniqueIndex};
use futures::{
    FutureExt, StreamExt,
    future::{BoxFuture, Either, Shared},
    stream::FuturesUnordered,
};
use qbft_manager::ConsensusDecider;
use slot_clock::SlotClock;
use ssv_types::{
    Cluster, CommitteeId, ValidatorIndex, ValidatorMetadata,
    consensus::{AggregatorCommitteeConsensusData, AssignedAggregator, QbftData},
    msgid::Role,
    partial_sig::PartialSignatureKind,
};
use tokio::time::Instant;
use tracing::{Instrument, debug, error, info, info_span, warn};
use types::{
    AggregateAndProof, Attestation, ContributionAndProof, Domain, EthSpec, ForkName, Hash256,
    SelectionProof, SignedAggregateAndProof, SignedContributionAndProof, SignedRoot, Slot,
};
use validator_metrics::IntCounterVec;

use crate::{
    AggregationAssignments, AnchorValidatorStore, CollectionMode, Error, SigningRequest,
    SpecificError, metrics,
};

/// Epochs to retain started `AggregatorCommittee` post-consensus keys.
///
/// Matches the message validator's per-signer duplicate-detection window, so a repeated
/// `(committee, slot)` inside the window finds its registered key rather than starting a second
/// execution. Each entry is one key, not a handle; the executions live in their detached tasks.
const AGGREGATOR_POST_CONSENSUS_RETAIN_EPOCHS: u64 = 2;

/// A result shared between the detached task producing it and any number of joiners.
type SharedResult<T> = Shared<BoxFuture<'static, Result<T, Arc<Error>>>>;

pub(crate) type AggregatorPostConsensusShared<E> =
    SharedResult<Arc<AggregatorPostConsensusOutcome<E>>>;

/// One decided root this operator signs, with the handle to its signature.
///
/// The underlying `collect_signature` call runs in its own task, so the local partial reaches the
/// committee batch even when no callback ever awaits the signature.
pub(crate) struct PreparedRoot<M> {
    pub(crate) request: SigningRequest<M>,
    pub(crate) signature: SharedResult<Signature>,
}

/// Everything this operator signs for one decided `AggregatorCommittee` round, keyed by signing
/// identity: validator index for aggregates, `(validator index, subcommittee index)` for
/// contributions. Both classes are read by the metadata service's publisher.
pub(crate) struct AggregatorPostConsensusOutcome<E: EthSpec> {
    /// Fork the decided value's SSZ payloads were decoded under (its `DataVersion`). The
    /// publisher derives its HTTP endpoint choice and fork header from this, so they cannot
    /// diverge from the payload variant of the decided aggregates.
    pub(crate) fork_name: ForkName,
    pub(crate) aggregates: HashMap<ValidatorIndex, PreparedRoot<AggregateAndProof<E>>>,
    pub(crate) contributions: HashMap<(ValidatorIndex, u64), PreparedRoot<ContributionAndProof<E>>>,
}

/// The signing worklist derived from one decided `AggregatorCommitteeConsensusData`.
struct DecidedWorklist<E: EthSpec> {
    aggregates: HashMap<ValidatorIndex, SigningRequest<AggregateAndProof<E>>>,
    contributions: HashMap<(ValidatorIndex, u64), SigningRequest<ContributionAndProof<E>>>,
}

fn post_consensus_aborted() -> Arc<Error> {
    Arc::new(Error::SpecificError(SpecificError::PostConsensusAborted))
}

/// Await a detached task spawned with `TaskExecutor::spawn_handle`, mapping every way it can die
/// without a result (spawn refused, executor exit, panic) onto `PostConsensusAborted`.
async fn join_detached_task<R>(
    handle: Option<tokio::task::JoinHandle<Option<R>>>,
) -> Result<R, Arc<Error>> {
    match handle {
        Some(handle) => handle.await.ok().flatten(),
        None => None,
    }
    .ok_or_else(post_consensus_aborted)
}

/// Await one decided root's threshold signature under the deadline, of either class.
///
/// Returns `None` after logging the skip and counting the failure on `signed_total` (the
/// class's Lighthouse-era signing counter): `other_error` when collection failed, `timeout` when
/// the deadline expired before quorum (matching the committee-level join arm, now that expiry is
/// distinguishable from a collection failure). `collect_signature` failures are also logged by
/// the detached signing task, but a task that died without a result (spawn refused, executor
/// exit, panic) is only visible here, so the collection-failure arm carries the error into its
/// warn.
async fn await_root_signature<M>(
    slot: Slot,
    deadline: Instant,
    root: &PreparedRoot<M>,
    signed_total: &LazyLock<validator_metrics::Result<IntCounterVec>>,
    skip_message: &'static str,
) -> Option<Signature> {
    let label = match tokio::time::timeout_at(deadline, root.signature.clone()).await {
        Ok(Ok(signature)) => return Some(signature),
        Ok(Err(e)) => {
            warn!(
                pubkey = ?root.request.validator.public_key,
                %slot,
                error = ?e,
                "{skip_message}"
            );
            metrics::OTHER_ERROR
        }
        // Deadline expired before this root's quorum; only this root is withheld.
        Err(_) => {
            warn!(
                pubkey = ?root.request.validator.public_key,
                %slot,
                "{skip_message}"
            );
            metrics::TIMEOUT
        }
    };
    validator_metrics::inc_counter_vec(signed_total, &[label]);
    None
}

/// Await one decided root's threshold signature under the deadline and publish it on quorum,
/// one publish call per aggregate.
///
/// Both publish arms reproduce the per-aggregate log Lighthouse emits on its pre-Boole path;
/// operator pipelines key on `type="aggregated"`.
async fn publish_one_root<E: EthSpec, F, Fut>(
    slot: Slot,
    fork_name: ForkName,
    deadline: Instant,
    root: &PreparedRoot<AggregateAndProof<E>>,
    publish: &F,
) where
    F: Fn(ForkName, SignedAggregateAndProof<E>) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let Some(signature) = await_root_signature(
        slot,
        deadline,
        root,
        &validator_metrics::SIGNED_AGGREGATES_TOTAL,
        "Missing signature, skipping aggregate",
    )
    .await
    else {
        // The publish-outcome counter is aggregate-only; contributions are tracked by their
        // signing counter and publish logs alone.
        metrics::inc_publish_result(metrics::NO_SIGNATURES);
        return;
    };

    let message = root.request.duty_data.clone();
    debug!(
        aggregator_index = message.aggregator_index(),
        data = ?message.aggregate().data(),
        num_set_aggregation_bits = message.aggregate().num_set_aggregation_bits(),
        "Signed AggregateAndProof (Boole+ committee consensus)"
    );
    validator_metrics::inc_counter_vec(
        &validator_metrics::SIGNED_AGGREGATES_TOTAL,
        &[validator_metrics::SUCCESS],
    );
    let signed = SignedAggregateAndProof::from_aggregate_and_proof(message, signature);

    let aggregator = signed.message().aggregator_index();
    let attestation = signed.message().aggregate();
    let signatures = attestation.num_set_aggregation_bits();
    let head_block = format!("{:?}", attestation.data().beacon_block_root);
    let committee_index = attestation.committee_index();

    let result = publish(fork_name, signed)
        .instrument(info_span!("publish_aggregate", aggregator))
        .await;
    match result {
        Ok(()) => {
            info!(
                aggregator,
                signatures,
                head_block,
                committee_index,
                slot = slot.as_u64(),
                "type" = "aggregated",
                "Successfully published attestation"
            );
            metrics::inc_publish_result(validator_metrics::SUCCESS);
        }
        Err(e) => {
            error!(
                error = %e,
                aggregator,
                committee_index,
                slot = slot.as_u64(),
                "type" = "aggregated",
                "Failed to publish attestation"
            );
            metrics::inc_publish_result(metrics::HTTP_ERROR);
        }
    }
}

/// Await one decided contribution's threshold signature under the deadline and publish it on
/// quorum, one publish call per contribution.
///
/// Signing outcomes keep the Lighthouse-era `SIGNED_SYNC_COMMITTEE_CONTRIBUTIONS_TOTAL`
/// accounting that used to live in the contribution callback, and the publish arms reproduce the
/// per-publish logs Lighthouse emits on its pre-Boole path, so operator pipelines keyed on
/// either keep working. `AGGREGATOR_COMMITTEE_PUBLISH_TOTAL` stays aggregate-only.
async fn publish_one_contribution<E: EthSpec, F, Fut>(
    slot: Slot,
    deadline: Instant,
    root: &PreparedRoot<ContributionAndProof<E>>,
    publish: &F,
) where
    F: Fn(SignedContributionAndProof<E>) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let Some(signature) = await_root_signature(
        slot,
        deadline,
        root,
        &validator_metrics::SIGNED_SYNC_COMMITTEE_CONTRIBUTIONS_TOTAL,
        "Missing signature, skipping sync committee contribution",
    )
    .await
    else {
        return;
    };

    let message = root.request.duty_data.clone();
    debug!(
        aggregator_index = message.aggregator_index,
        slot = %message.contribution.slot,
        block_root = ?message.contribution.beacon_block_root,
        subcommittee_index = message.contribution.subcommittee_index,
        num_set_aggregation_bits = message.contribution.aggregation_bits.num_set_bits(),
        "Signed ContributionAndProof (Boole+ committee consensus)"
    );
    validator_metrics::inc_counter_vec(
        &validator_metrics::SIGNED_SYNC_COMMITTEE_CONTRIBUTIONS_TOTAL,
        &[validator_metrics::SUCCESS],
    );

    let aggregator_index = message.aggregator_index;
    let subnet = message.contribution.subcommittee_index;
    let beacon_block_root = message.contribution.beacon_block_root;
    let num_signers = message.contribution.aggregation_bits.num_set_bits();
    let signed = SignedContributionAndProof { message, signature };

    let result = publish(signed)
        .instrument(info_span!("publish_contribution", aggregator_index))
        .await;
    match result {
        Ok(()) => {
            info!(
                aggregator_index,
                subnet,
                beacon_block_root = ?beacon_block_root,
                num_signers,
                slot = slot.as_u64(),
                "Successfully published sync contributions"
            );
        }
        Err(e) => {
            error!(
                error = %e,
                aggregator_index,
                subnet,
                slot = slot.as_u64(),
                "Unable to publish signed contributions and proofs"
            );
        }
    }
}

impl<T: SlotClock + 'static, E: EthSpec, C: ConsensusDecider<E> + 'static>
    AnchorValidatorStore<T, E, C>
{
    /// Start one post-consensus execution per committee in freshly built assignments, returning
    /// the executions newly registered by this call.
    ///
    /// This is the only place executions are created, so exactly one committee message per
    /// `(committee, slot)` holds by construction rather than by locking. Called from
    /// `update_aggregation_assignments` before the watch channel is published.
    ///
    /// The returned handles feed the metadata service's publisher, which is their sole consumer.
    /// Returning only first-insertions gives the publisher the same exactly-once property as the
    /// executions themselves: a repeated call for one `(committee, slot)` registers nothing and
    /// therefore publishes nothing twice. Both properties are scoped to the process lifetime and
    /// to the retention window below: after a key is pruned, only a slot clock stepping backwards
    /// past the window could present its `(committee, slot)` again, and that would re-register.
    ///
    /// Pre-Boole there is no consensus data and this is a no-op returning no executions.
    #[must_use = "dropping the returned executions disables Boole+ aggregate and contribution \
                  publication"]
    pub(crate) fn start_aggregator_post_consensus(
        self: &Arc<Self>,
        assignments: &AggregationAssignments<E>,
    ) -> Vec<(CommitteeId, AggregatorPostConsensusShared<E>)> {
        let slot = assignments.slot;
        let mut executions = self.aggregator_post_consensus.lock();

        let cutoff =
            slot.saturating_sub(AGGREGATOR_POST_CONSENSUS_RETAIN_EPOCHS * E::slots_per_epoch());
        executions.retain(|&(_, execution_slot)| execution_slot >= cutoff);

        let mut new_executions = Vec::new();
        for (&committee_id, decided_data) in &assignments.consensus_data_by_ssv_committee {
            // First-insert-only, so registration is idempotent. A second execution for one key
            // would spawn a second QBFT round and a second set of detached signing tasks while
            // the first set kept running, putting two committee messages on the wire for one
            // slot, which peers reject with a gossip penalty. The slot pipeline publishes once
            // per slot today, but that is a property of another module's timing loop; keeping
            // this first-insert-only makes exactly-once hold here regardless of how often it is
            // called.
            if executions.insert((committee_id, slot)) {
                let store = Arc::clone(self);
                let decided_data = Arc::clone(decided_data);
                let execution = self.spawn_shared("aggregator_post_consensus", async move {
                    store
                        .run_aggregator_post_consensus(committee_id, slot, decided_data)
                        .await
                });
                new_executions.push((committee_id, execution));
            }
        }
        new_executions
    }

    /// Join one committee's execution under the shared deadline, returning the outcome and the
    /// deadline for the caller to reuse when draining its own roots.
    ///
    /// The single deadline (one slot past the duty slot's end) is an anti-hang backstop, not a
    /// duty-freshness bound: QBFT `SlotTime` rounds legitimately decide after the slot ends and
    /// reconstruction needs a network round trip after that, so bounding at slot end would discard
    /// results decided in contended rounds.
    async fn join_execution(
        &self,
        slot: Slot,
        execution: AggregatorPostConsensusShared<E>,
    ) -> Result<(Instant, Arc<AggregatorPostConsensusOutcome<E>>), Error> {
        let deadline = self.get_instant_in_slot(slot, self.spec.get_slot_duration() * 2)?;
        let outcome = tokio::time::timeout_at(deadline, execution)
            .await
            .map_err(|_| Error::SpecificError(SpecificError::Timeout))?
            .map_err(|e| (*e).clone())?;

        Ok((deadline, outcome))
    }

    /// Join one committee's post-consensus execution and publish each decided root this operator
    /// signed, of both classes, as soon as its threshold signature reconstructs.
    ///
    /// This backs [`Self::publish_decided_aggregates`], which owns Boole+ publication of
    /// aggregates and sync contributions: Lighthouse's callbacks return empty batches at Boole+,
    /// because their duty snapshots are cloned before slot-start selection proofs finish,
    /// silently skipping any proof installed after the clone. The publisher works from the
    /// decided value instead, so publication does not depend on Lighthouse's snapshot timing.
    ///
    /// Takes the execution handle directly rather than re-entering the assignments watch channel,
    /// so a late-polled publisher cannot observe `AggregatorInfoSlotPassed` for an execution that
    /// still exists in the retention map.
    async fn publish_committee_aggregates<FA, FutA, FC, FutC>(
        &self,
        committee_id: CommitteeId,
        slot: Slot,
        execution: AggregatorPostConsensusShared<E>,
        publish_aggregate: &FA,
        publish_contribution: &FC,
    ) where
        FA: Fn(ForkName, SignedAggregateAndProof<E>) -> FutA,
        FutA: Future<Output = Result<(), String>>,
        FC: Fn(SignedContributionAndProof<E>) -> FutC,
        FutC: Future<Output = Result<(), String>>,
    {
        let (deadline, outcome) = match self.join_execution(slot, execution).await {
            Ok(joined) => joined,
            Err(e) => {
                warn!(
                    ?committee_id,
                    %slot,
                    error = ?e,
                    "Aggregator post-consensus failed, nothing to publish"
                );
                // Keeps the Lighthouse-era signing counters alive for dashboards keyed on them.
                // The per-root counts and class composition are unknowable before the outcome
                // resolves, so failures count once per committee on both class counters: an
                // undercount against multiple lost roots, an overcount for a class the committee
                // had no duties in, but never silence.
                let label = match e {
                    Error::SpecificError(SpecificError::Timeout) => metrics::TIMEOUT,
                    _ => metrics::OTHER_ERROR,
                };
                validator_metrics::inc_counter_vec(
                    &validator_metrics::SIGNED_AGGREGATES_TOTAL,
                    &[label],
                );
                validator_metrics::inc_counter_vec(
                    &validator_metrics::SIGNED_SYNC_COMMITTEE_CONTRIBUTIONS_TOTAL,
                    &[label],
                );
                metrics::inc_publish_result(metrics::CONSENSUS_ERROR);
                return;
            }
        };

        // A decided value can legitimately hold either class alone; each root publishes
        // independently, and an empty drain below is a no-op. The aggregate-only publish counter
        // keeps its committee-level `no_aggregates` outcome.
        if outcome.aggregates.is_empty() {
            debug!(?committee_id, %slot, "Decided worklist holds no aggregates");
            metrics::inc_publish_result(metrics::NO_AGGREGATES);
        }

        // Beacon nodes accept a contribution only for its own slot (no lookback window, unlike
        // aggregates' one-epoch tolerance), so a contribution quorum arriving after the slot ends
        // is unpublishable. Bound that wait at slot end instead of POSTing a guaranteed
        // rejection; aggregates keep the full deadline.
        let contribution_deadline = self
            .get_instant_in_slot(slot, self.spec.get_slot_duration())
            .unwrap_or(deadline);

        // Each root of either class publishes independently under its class's deadline, so a
        // root that never reaches quorum withholds only itself and never delays a sibling's
        // POST, in its own class or the other.
        let mut roots: FuturesUnordered<_> = outcome
            .aggregates
            .values()
            .map(|root| {
                Either::Left(publish_one_root(
                    slot,
                    outcome.fork_name,
                    deadline,
                    root,
                    publish_aggregate,
                ))
            })
            .chain(outcome.contributions.values().map(|root| {
                Either::Right(publish_one_contribution(
                    slot,
                    contribution_deadline,
                    root,
                    publish_contribution,
                ))
            }))
            .collect();
        while roots.next().await.is_some() {}
    }

    /// Resolve each committee's decided roots and hand each signed aggregate or contribution to
    /// its publish operation the moment its threshold signature reconstructs, one publish call
    /// per root. This is go-ssv's publication shape at the reference pin: a bad root cannot fail
    /// a sibling's POST, and a no-quorum root cannot delay its committee's other roots.
    ///
    /// This is the authoritative Boole+ publication driver for both classes, spawned per slot by
    /// the metadata service with the executions its assignment update newly registered. Generic
    /// over the publish operations so tests (in `testing/aggregator_post_consensus.rs`) can
    /// inject recorders instead of an HTTP client. Each committee runs as one `FuturesUnordered`
    /// entry and each of its roots as another inside it, so committees and roots are all mutually
    /// independent.
    ///
    /// Owns every `AGGREGATOR_COMMITTEE_PUBLISH_TOTAL` increment, together with
    /// [`publish_one_root`]: `consensus_error` and `no_aggregates` count committees (those
    /// failures occur before per-root work exists), while `success`, `http_error`, and
    /// `no_signatures` count individual aggregates. Contribution outcomes are tracked by
    /// `SIGNED_SYNC_COMMITTEE_CONTRIBUTIONS_TOTAL` and per-publish logs instead (see
    /// [`publish_one_contribution`]).
    ///
    /// The fork handed to `publish_aggregate` is the one the decided value's payloads were
    /// decoded under, so the closure's endpoint choice always matches the payload variant. The
    /// contribution endpoint is fork-independent at the pin, so `publish_contribution` takes
    /// none.
    pub(crate) async fn publish_decided_aggregates<FA, FutA, FC, FutC>(
        &self,
        slot: Slot,
        executions: Vec<(CommitteeId, AggregatorPostConsensusShared<E>)>,
        publish_aggregate: FA,
        publish_contribution: FC,
    ) where
        FA: Fn(ForkName, SignedAggregateAndProof<E>) -> FutA,
        FutA: Future<Output = Result<(), String>>,
        FC: Fn(SignedContributionAndProof<E>) -> FutC,
        FutC: Future<Output = Result<(), String>>,
    {
        let publish_aggregate = &publish_aggregate;
        let publish_contribution = &publish_contribution;
        let mut pending: FuturesUnordered<_> = executions
            .into_iter()
            .map(|(committee_id, execution)| {
                self.publish_committee_aggregates(
                    committee_id,
                    slot,
                    execution,
                    publish_aggregate,
                    publish_contribution,
                )
            })
            .collect();

        while pending.next().await.is_some() {}
    }

    /// Spawn `fut` detached and hand out a `Shared` view of its result.
    fn spawn_shared<R: Clone + Send + Sync + 'static>(
        &self,
        task_name: &'static str,
        fut: impl Future<Output = Result<R, Arc<Error>>> + Send + 'static,
    ) -> SharedResult<R> {
        let handle = self.task_executor.spawn_handle(fut, task_name);
        async move { join_detached_task(handle).await.and_then(|result| result) }
            .boxed()
            .shared()
    }

    /// Decide the committee's value, then submit every root this operator can sign from it.
    ///
    /// Which Lighthouse callbacks happen to fire has no influence on what is signed, only on what
    /// is returned to Lighthouse.
    async fn run_aggregator_post_consensus(
        self: Arc<Self>,
        committee_id: CommitteeId,
        slot: Slot,
        our_consensus_data: Arc<AggregatorCommitteeConsensusData<E>>,
    ) -> Result<Arc<AggregatorPostConsensusOutcome<E>>, Arc<Error>> {
        let Some(cluster) = self.committee_cluster(&committee_id) else {
            warn!(
                ?committee_id,
                %slot,
                "No active cluster for committee, skipping aggregator post-consensus"
            );
            return Ok(Arc::new(AggregatorPostConsensusOutcome {
                fork_name: ForkName::from(our_consensus_data.version),
                aggregates: HashMap::new(),
                contributions: HashMap::new(),
            }));
        };
        let cluster = Arc::new(cluster);

        let decided_data = self
            .run_aggregator_committee_consensus(committee_id, slot, &cluster, &our_consensus_data)
            .await
            .map_err(Arc::new)?;

        let worklist = self.build_decided_worklist(&decided_data, &committee_id, slot);

        // The batch size is the number of partials this operator actually submits. Peers do not
        // validate completeness for `AggregatorCommittee` (committee views may differ), and a count
        // local filtering cannot reach stalls the batch forever.
        let batch_size = worklist.aggregates.len() + worklist.contributions.len();
        if batch_size == 0 {
            debug!(
                ?committee_id,
                %slot,
                "No locally signable entries in decided aggregator committee data"
            );
        }
        let collection_mode = CollectionMode::Committee {
            validator_partial_signature_batch_size: batch_size,
            base_hash: decided_data.hash(),
        };

        Ok(Arc::new(AggregatorPostConsensusOutcome {
            fork_name: ForkName::from(decided_data.version),
            aggregates: self.prepare_roots(worklist.aggregates, &collection_mode, &cluster, slot),
            contributions: self.prepare_roots(
                worklist.contributions,
                &collection_mode,
                &cluster,
                slot,
            ),
        }))
    }

    /// Any active cluster in the committee.
    ///
    /// `Cluster::committee_id` is derived from `cluster_members`, so every cluster sharing a
    /// committee has the same operator set and the same `f`, which is all consensus and the
    /// signature threshold need. Per-validator liquidation is still filtered in the worklist.
    fn committee_cluster(&self, committee_id: &CommitteeId) -> Option<Cluster> {
        let state = self.database.state();
        state
            .metadata()
            .get_all_by(committee_id)
            .find_map(|validator| {
                let cluster = state.clusters().get_by(&validator.cluster_id)?;
                (!cluster.liquidated).then(|| cluster.clone())
            })
    }

    /// Spawn one detached signing task per worklist entry.
    fn prepare_roots<K: std::hash::Hash + Eq, M>(
        self: &Arc<Self>,
        requests: HashMap<K, SigningRequest<M>>,
        collection_mode: &CollectionMode,
        cluster: &Arc<Cluster>,
        slot: Slot,
    ) -> HashMap<K, PreparedRoot<M>> {
        requests
            .into_iter()
            .map(|(key, request)| {
                let signature = self.spawn_root_collection(
                    collection_mode.clone(),
                    request.validator.clone(),
                    Arc::clone(cluster),
                    request.signing_root,
                    slot,
                );
                (key, PreparedRoot { request, signature })
            })
            .collect()
    }

    /// Spawn one root's signature collection as its own detached task.
    ///
    /// The task hands the partial to the committee batch even if no callback ever awaits the
    /// returned handle, so an absent or dropped Lighthouse callback cannot strand the batch below
    /// its expected size.
    fn spawn_root_collection(
        self: &Arc<Self>,
        collection_mode: CollectionMode,
        validator: ValidatorMetadata,
        cluster: Arc<Cluster>,
        signing_root: Hash256,
        slot: Slot,
    ) -> SharedResult<Signature> {
        let store = Arc::clone(self);
        self.spawn_shared("aggregator_post_consensus_root", async move {
            let result = store
                .collect_signature(
                    PartialSignatureKind::PostConsensus,
                    Role::AggregatorCommittee,
                    collection_mode,
                    &validator,
                    &cluster,
                    signing_root,
                    slot,
                )
                .await;
            if let Err(e) = &result {
                // Callbacks only see failures for roots they requested; log here so a failed
                // unrequested root (which leaves the committee batch under-filled and unsent) is
                // still visible to operators.
                error!(
                    ?signing_root,
                    pubkey = ?validator.public_key,
                    error = ?e,
                    "Post-consensus root signing failed; the committee batch for this duty \
                     may not be sent"
                );
            }
            result.map_err(Arc::new)
        })
    }

    /// Build the signing worklist from a decided value.
    ///
    /// Resolves metadata, cluster status and share availability for every decided entry under one
    /// database snapshot, then keys the result by signing identity: validator index for aggregates,
    /// `(validator, subcommittee)` for contributions. Keying by identity collapses repeated decided
    /// entries (a root submitted twice would double-count the batch) and holds every validator
    /// inside the receivers' five-roots-per-validator cap; a decided value carrying conflicting
    /// roots for one identity signs the first and logs the rest.
    fn build_decided_worklist(
        &self,
        decided_data: &AggregatorCommitteeConsensusData<E>,
        committee_id: &CommitteeId,
        slot: Slot,
    ) -> DecidedWorklist<E> {
        let decided_indices: HashSet<ValidatorIndex> = decided_data
            .aggregators
            .iter()
            .chain(decided_data.contributors.iter())
            .map(|entry| entry.validator_index)
            .collect();

        // One snapshot for metadata, liquidation and share rows, so entries cannot mix database
        // states. Released before any signing.
        let locals: HashMap<ValidatorIndex, ValidatorMetadata> = {
            let state = self.database.state();
            state
                .metadata()
                .get_all_by(committee_id)
                .filter_map(|validator| {
                    let index = validator.index.filter(|i| decided_indices.contains(i))?;
                    let cluster = state.clusters().get_by(&validator.cluster_id)?;
                    if cluster.liquidated {
                        return None;
                    }
                    // Without a share row this operator cannot sign for the validator, and counting
                    // it would leave the committee batch permanently short.
                    state.shares().get_by(&validator.public_key)?;
                    Some((index, validator.clone()))
                })
                .collect()
        };

        let epoch = slot.epoch(E::slots_per_epoch());
        let aggregate_domain = self.get_domain(epoch, Domain::AggregateAndProof);
        let contribution_domain = self.get_domain(epoch, Domain::ContributionAndProof);

        let mut aggregates = HashMap::new();
        for decided_aggregator in decided_data.aggregators.iter() {
            let Some(validator) = locals.get(&decided_aggregator.validator_index) else {
                continue;
            };
            let request = match Self::resolve_decided_aggregate(
                decided_data,
                decided_aggregator,
                validator,
                slot,
                aggregate_domain,
            ) {
                Ok(request) => request,
                Err(e) => {
                    debug!(
                        validator_index = ?decided_aggregator.validator_index,
                        error = e,
                        "Skipping decided aggregate"
                    );
                    continue;
                }
            };
            match aggregates.entry(decided_aggregator.validator_index) {
                Entry::Vacant(vacant) => {
                    vacant.insert(request);
                }
                Entry::Occupied(kept) if kept.get().signing_root != request.signing_root => warn!(
                    validator_index = ?decided_aggregator.validator_index,
                    "Decided value contains conflicting aggregate roots for one validator, \
                     signing the first"
                ),
                Entry::Occupied(_) => {}
            }
        }

        let mut contributions = HashMap::new();
        for decided_contributor in decided_data.contributors.iter() {
            let Some(validator) = locals.get(&decided_contributor.validator_index) else {
                continue;
            };
            let request = match Self::resolve_decided_contribution(
                decided_data,
                decided_contributor,
                validator,
                slot,
                contribution_domain,
            ) {
                Ok(request) => request,
                Err(e) => {
                    debug!(
                        validator_index = ?decided_contributor.validator_index,
                        subcommittee_index = decided_contributor.committee_index,
                        error = e,
                        "Skipping decided contribution"
                    );
                    continue;
                }
            };
            match contributions.entry((
                decided_contributor.validator_index,
                decided_contributor.committee_index,
            )) {
                Entry::Vacant(vacant) => {
                    vacant.insert(request);
                }
                Entry::Occupied(kept) if kept.get().signing_root != request.signing_root => warn!(
                    validator_index = ?decided_contributor.validator_index,
                    subcommittee_index = decided_contributor.committee_index,
                    "Decided value contains conflicting contribution roots for one identity, \
                     signing the first"
                ),
                Entry::Occupied(_) => {}
            }
        }

        DecidedWorklist {
            aggregates,
            contributions,
        }
    }

    /// Resolve one decided aggregator entry into a signable `AggregateAndProof`.
    ///
    /// Finds the matching committee index, decodes the aggregate attestation from SSZ, and binds
    /// the decoded slot to the duty slot. The signing domain comes from the duty slot's epoch,
    /// matching the beacon spec (domain at the epoch of `aggregate.data.slot`) and go-ssv's
    /// expected roots.
    fn resolve_decided_aggregate(
        decided_data: &AggregatorCommitteeConsensusData<E>,
        decided_aggregator: &AssignedAggregator,
        validator: &ValidatorMetadata,
        slot: Slot,
        domain_hash: Hash256,
    ) -> Result<SigningRequest<AggregateAndProof<E>>, String> {
        let aggregator_index = decided_aggregator.validator_index.0 as u64;

        let committee_index = decided_aggregator.committee_index;

        // Find the position of this committee index in decided data
        let decided_aggregate_idx = decided_data
            .aggregator_committee_indexes
            .iter()
            .position(|&idx| idx == committee_index)
            .ok_or("Committee index not found in decided data")?;

        let decided_aggregate_bytes = decided_data
            .aggregated_attestations
            .get(decided_aggregate_idx)
            .ok_or("Aggregate attestation bytes not found in decided data")?;

        // Decode with the shape the decided version selects (Gloas merkleizes progressively,
        // so the shape drives the signing root computed below).
        let decided_aggregate: Attestation<E> = decided_data
            .version
            .decode_attestation(decided_aggregate_bytes)
            .map_err(|e| format!("Failed to decode decided aggregate: {e}"))?;

        let aggregate_slot = decided_aggregate.data().slot;
        if aggregate_slot != slot {
            return Err(format!(
                "Decided aggregate slot {aggregate_slot} does not match duty slot {slot}"
            ));
        }

        let decided_selection_proof =
            SelectionProof::from(decided_aggregator.selection_proof.clone());

        let message = AggregateAndProof::from_attestation(
            aggregator_index,
            decided_aggregate,
            decided_selection_proof,
        );

        Ok(SigningRequest {
            validator: validator.clone(),
            signing_root: message.signing_root(domain_hash),
            duty_data: message,
        })
    }

    /// Resolve one decided contributor entry into a signable `ContributionAndProof`.
    fn resolve_decided_contribution(
        decided_data: &AggregatorCommitteeConsensusData<E>,
        decided_contributor: &AssignedAggregator,
        validator: &ValidatorMetadata,
        slot: Slot,
        domain_hash: Hash256,
    ) -> Result<SigningRequest<ContributionAndProof<E>>, String> {
        let subcommittee_index = decided_contributor.committee_index;

        // Find the contribution for this subcommittee
        let decided_contribution = decided_data
            .sync_committee_contributions
            .iter()
            .find(|c| c.subcommittee_index == subcommittee_index)
            .ok_or(
                "Contribution not in consensus data - likely filtered due to beacon API failure",
            )?;
        if decided_contribution.slot != slot {
            return Err(format!(
                "Decided contribution slot {} does not match duty slot {slot}",
                decided_contribution.slot
            ));
        }

        let message = ContributionAndProof {
            aggregator_index: decided_contributor.validator_index.0 as u64,
            contribution: decided_contribution.clone(),
            selection_proof: decided_contributor.selection_proof.clone(),
        };

        Ok(SigningRequest {
            validator: validator.clone(),
            signing_root: message.signing_root(domain_hash),
            duty_data: message,
        })
    }
}
