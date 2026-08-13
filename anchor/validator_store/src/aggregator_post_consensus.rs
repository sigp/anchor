//! Post-consensus signing for Boole+ `AggregatorCommittee` duties.
//!
//! One QBFT decision carries both attestation aggregates and sync contributions for an SSV
//! committee, and the operator must emit exactly one committee partial-signature message per
//! `(committee, slot)` covering everything it can sign. The decided value drives both what gets
//! signed and what gets published: Anchor owns Boole+ aggregate publication outright, through
//! the metadata service's publisher, because the protocol's unit of agreement is the decided
//! value and no local Lighthouse view is part of it. (Lighthouse's duty snapshot in particular
//! is cloned before slot-start selection proofs finish, so routing publication through it would
//! silently skip late-installed proofs.) Contributions are returned through a Lighthouse
//! callback that may or may not fire (issue #1227), so no callback can own the committee
//! message either.
//!
//! The signing set is therefore a property of the duty, not of any consumer. The slot pipeline
//! starts one execution per committee when it publishes the decided value at 2/3 slot; the
//! contributions callback and the aggregate publisher only look up their own results. This is
//! the ssv-spec-normative shape, where the decided value drives what gets signed and local state
//! only filters.

use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    future::Future,
    sync::Arc,
};

use bls::Signature;
use database::{NonUniqueIndex, UniqueIndex};
use futures::{
    FutureExt, StreamExt,
    future::{BoxFuture, Shared},
    stream::FuturesUnordered,
};
use qbft_manager::ConsensusDecider;
use slot_clock::SlotClock;
use ssv_types::{
    Cluster, CommitteeId, ValidatorIndex, ValidatorMetadata,
    consensus::{AggregatorCommitteeConsensusData, AssignedAggregator, DataVersion, QbftData},
    msgid::Role,
    partial_sig::PartialSignatureKind,
};
use ssz::Decode;
use tokio::time::Instant;
use tracing::{Instrument, debug, error, info, info_span, warn};
use types::{
    AggregateAndProof, Attestation, AttestationBase, AttestationElectra, ContributionAndProof,
    Domain, EthSpec, ForkName, Hash256, SelectionProof, SignedAggregateAndProof, SignedRoot, Slot,
};

use crate::{
    AggregationAssignments, AnchorValidatorStore, CollectionMode, Error, SigningRequest,
    SpecificError, drain_signatures, metrics,
};

/// Epochs to retain finished `AggregatorCommittee` post-consensus executions.
///
/// Matches the message validator's per-signer duplicate-detection window, so a very late Lighthouse
/// callback finds the execution it belongs to rather than nothing. Each entry is one small future
/// per committee-slot.
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
/// identity: validator index for aggregates (read by the aggregate publisher),
/// `(validator index, subcommittee index)` for contributions (read by the Lighthouse callback).
pub(crate) struct AggregatorPostConsensusOutcome<E: EthSpec> {
    /// Fork the decided value's SSZ payloads were decoded under (its `DataVersion`). The
    /// publisher derives its HTTP endpoint choice and fork header from this, so they cannot
    /// diverge from the payload variant actually in the batch.
    pub(crate) fork_name: ForkName,
    pub(crate) aggregates: HashMap<ValidatorIndex, PreparedRoot<AggregateAndProof<E>>>,
    pub(crate) contributions: HashMap<(ValidatorIndex, u64), PreparedRoot<ContributionAndProof<E>>>,
}

/// Outcome of resolving one committee's decided aggregates for publication.
///
/// Returned instead of a bare vec so [`AnchorValidatorStore::publish_decided_aggregates`] can
/// emit every `AGGREGATOR_COMMITTEE_PUBLISH_TOTAL` label from one `match`, keeping the metric's
/// partition over processed committees checkable in one place.
pub(crate) enum ResolvedAggregates<E: EthSpec> {
    /// Consensus failed, timed out, or the deadline could not be computed (logged at the site).
    ConsensusFailed,
    /// The decided worklist legitimately holds contributions only.
    NoAggregates,
    /// Aggregates were decided, but no root reached signature quorum before the deadline.
    NoSignatures,
    /// At least one decided aggregate reached quorum. `fork_name` is the fork the decided
    /// value's payloads were decoded under, carried with the batch so the publish endpoint
    /// always matches the payload variant.
    Batch {
        fork_name: ForkName,
        aggregates: Vec<SignedAggregateAndProof<E>>,
    },
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

impl<T: SlotClock + 'static, E: EthSpec, C: ConsensusDecider<E> + 'static>
    AnchorValidatorStore<T, E, C>
{
    /// Start one post-consensus execution per committee in freshly built assignments, returning
    /// the executions newly registered by this call.
    ///
    /// This is the only place executions are created. Callbacks look results up and never start
    /// work, so exactly one committee message per `(committee, slot)` holds by construction rather
    /// than by locking. Called from `update_aggregation_assignments` before the watch channel is
    /// published, so any consumer that can observe the assignments can also observe the execution.
    ///
    /// The returned handles feed the metadata service's aggregate publisher. Returning only the
    /// vacant insertions gives the publisher the same exactly-once property as the executions
    /// themselves: a repeated call for one `(committee, slot)` registers nothing and therefore
    /// publishes nothing twice. Both properties are scoped to the process lifetime and to the
    /// retention window below: after an entry is pruned, only a slot clock stepping backwards
    /// past the window could present its `(committee, slot)` again, and that would re-register.
    ///
    /// Pre-Boole there is no consensus data and this is a no-op returning no executions.
    #[must_use = "dropping the returned executions disables Boole+ aggregate publication"]
    pub(crate) fn start_aggregator_post_consensus(
        self: &Arc<Self>,
        assignments: &AggregationAssignments<E>,
    ) -> Vec<(CommitteeId, AggregatorPostConsensusShared<E>)> {
        let slot = assignments.slot;
        let mut executions = self.aggregator_post_consensus.lock();

        let cutoff =
            slot.saturating_sub(AGGREGATOR_POST_CONSENSUS_RETAIN_EPOCHS * E::slots_per_epoch());
        executions.retain(|(_, execution_slot), _| *execution_slot >= cutoff);

        let mut new_executions = Vec::new();
        for (&committee_id, decided_data) in &assignments.consensus_data_by_ssv_committee {
            // Vacant-only, so registration is idempotent. Overwriting would spawn a second QBFT
            // round and a second set of detached signing tasks while the first set kept running,
            // putting two committee messages on the wire for one slot, which peers reject with a
            // gossip penalty. The slot pipeline publishes once per slot today, but that is a
            // property of another module's timing loop; keeping this vacant-only makes
            // exactly-once hold here regardless of how often it is called.
            if let Entry::Vacant(vacant) = executions.entry((committee_id, slot)) {
                let store = Arc::clone(self);
                let decided_data = Arc::clone(decided_data);
                let execution = self.spawn_shared("aggregator_post_consensus", async move {
                    store
                        .run_aggregator_post_consensus(committee_id, slot, decided_data)
                        .await
                });
                vacant.insert(execution.clone());
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

    /// Join one committee's post-consensus execution and assemble every decided aggregate this
    /// operator signed into publishable [`SignedAggregateAndProof`]s.
    ///
    /// This backs [`Self::publish_decided_aggregates`], which owns Boole+ aggregate publication:
    /// Lighthouse's aggregate callback returns an empty batch at Boole+, because its duty
    /// snapshot is cloned before slot-start selection proofs finish, silently skipping any proof
    /// installed after the clone. The publisher works from the decided value instead, so
    /// publication does not depend on Lighthouse's snapshot timing.
    ///
    /// Takes the execution handle directly rather than re-entering the assignments watch channel,
    /// so a late-polled publisher cannot observe `AggregatorInfoSlotPassed` for an execution that
    /// still exists in the retention map.
    pub(crate) async fn resolve_decided_aggregates(
        &self,
        committee_id: CommitteeId,
        slot: Slot,
        execution: AggregatorPostConsensusShared<E>,
    ) -> ResolvedAggregates<E> {
        let (deadline, outcome) = match self.join_execution(slot, execution).await {
            Ok(joined) => joined,
            Err(e) => {
                warn!(
                    ?committee_id,
                    %slot,
                    error = ?e,
                    "Aggregator post-consensus failed, no aggregates to publish"
                );
                // Keeps the Lighthouse-era signing counter alive for dashboards keyed on it. The
                // per-aggregate count is unknowable before the outcome resolves, so failures
                // count once per committee, an undercount but not silence.
                let label = match e {
                    Error::SpecificError(SpecificError::Timeout) => metrics::TIMEOUT,
                    _ => metrics::OTHER_ERROR,
                };
                validator_metrics::inc_counter_vec(
                    &validator_metrics::SIGNED_AGGREGATES_TOTAL,
                    &[label],
                );
                return ResolvedAggregates::ConsensusFailed;
            }
        };

        // A decided value can legitimately hold contributions only; nothing to publish then.
        if outcome.aggregates.is_empty() {
            debug!(?committee_id, %slot, "Decided worklist holds no aggregates");
            return ResolvedAggregates::NoAggregates;
        }

        // Drain every aggregate root's signature concurrently under the deadline, so a root that
        // never reaches quorum withholds only itself.
        let pending = FuturesUnordered::new();
        for (&index, root) in &outcome.aggregates {
            let signing_root = root.request.signing_root;
            let signature = root.signature.clone();
            pending.push(async move { (index, signing_root, signature.await) });
        }
        let signatures = drain_signatures(pending, Some(deadline)).await;

        let mut results = Vec::with_capacity(outcome.aggregates.len());
        for (index, root) in &outcome.aggregates {
            // Clone the decided message only on a signature hit; misses need just the pubkey.
            let Some(signature) = signatures.get(&(*index, root.request.signing_root)) else {
                warn!(
                    pubkey = ?root.request.validator.public_key,
                    %slot,
                    "Missing signature, skipping aggregate"
                );
                validator_metrics::inc_counter_vec(
                    &validator_metrics::SIGNED_AGGREGATES_TOTAL,
                    &[metrics::OTHER_ERROR],
                );
                continue;
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
            results.push(SignedAggregateAndProof::from_aggregate_and_proof(
                message,
                signature.clone(),
            ));
        }

        if results.is_empty() {
            return ResolvedAggregates::NoSignatures;
        }

        ResolvedAggregates::Batch {
            fork_name: outcome.fork_name,
            aggregates: results,
        }
    }

    /// Resolve each committee's decided aggregates and hand every non-empty batch to `publish`.
    ///
    /// This is the authoritative Boole+ aggregate publication driver, spawned per slot by the
    /// metadata service with the executions its assignment update newly registered. Generic over
    /// the publish operation so tests (in `testing/aggregator_post_consensus.rs`) can inject a
    /// recorder instead of an HTTP client. Each committee runs resolve-and-publish as one
    /// `FuturesUnordered` entry, so a contended QBFT round or a slow POST in one committee does
    /// not delay another committee's publication.
    ///
    /// Owns every `AGGREGATOR_COMMITTEE_PUBLISH_TOTAL` increment: the labels partition the
    /// processed committees by outcome, and that partition is checkable in the single `match`
    /// below.
    ///
    /// A batch always holds exactly one committee's aggregates, all decoded from one decided
    /// `DataVersion`; the fork that version names travels with the batch, so the publish
    /// closure's endpoint choice always matches the payload variant.
    pub(crate) async fn publish_decided_aggregates<F, Fut>(
        &self,
        slot: Slot,
        executions: Vec<(CommitteeId, AggregatorPostConsensusShared<E>)>,
        publish: F,
    ) where
        F: Fn(ForkName, Arc<Vec<SignedAggregateAndProof<E>>>) -> Fut,
        Fut: Future<Output = Result<(), String>>,
    {
        let publish = &publish;
        let mut pending: FuturesUnordered<_> = executions
            .into_iter()
            .map(|(committee_id, execution)| async move {
                match self
                    .resolve_decided_aggregates(committee_id, slot, execution)
                    .await
                {
                    ResolvedAggregates::ConsensusFailed => {
                        metrics::inc_publish_result(metrics::CONSENSUS_ERROR);
                    }
                    ResolvedAggregates::NoAggregates => {
                        metrics::inc_publish_result(metrics::NO_AGGREGATES);
                    }
                    ResolvedAggregates::NoSignatures => {
                        warn!(
                            ?committee_id,
                            %slot,
                            "No decided aggregate reached signature quorum, nothing to publish"
                        );
                        metrics::inc_publish_result(metrics::NO_SIGNATURES);
                    }
                    ResolvedAggregates::Batch {
                        fork_name,
                        aggregates,
                    } => {
                        let signed = Arc::new(aggregates);
                        let result = publish(fork_name, Arc::clone(&signed))
                            .instrument(info_span!("publish_aggregates", count = signed.len()))
                            .await;
                        // Both arms reproduce the per-aggregate log Lighthouse emits on its
                        // pre-Boole path; operator pipelines key on `type="aggregated"`.
                        match result {
                            Ok(()) => {
                                for signed in signed.iter() {
                                    let attestation = signed.message().aggregate();
                                    info!(
                                        aggregator = signed.message().aggregator_index(),
                                        signatures = attestation.num_set_aggregation_bits(),
                                        head_block =
                                            format!("{:?}", attestation.data().beacon_block_root),
                                        committee_index = attestation.committee_index(),
                                        slot = slot.as_u64(),
                                        "type" = "aggregated",
                                        "Successfully published attestation"
                                    );
                                }
                                metrics::inc_publish_result(validator_metrics::SUCCESS);
                            }
                            Err(e) => {
                                for signed in signed.iter() {
                                    let attestation = signed.message().aggregate();
                                    error!(
                                        error = %e,
                                        aggregator = signed.message().aggregator_index(),
                                        committee_index = attestation.committee_index(),
                                        slot = slot.as_u64(),
                                        "type" = "aggregated",
                                        "Failed to publish attestation"
                                    );
                                }
                                metrics::inc_publish_result(metrics::HTTP_ERROR);
                            }
                        }
                    }
                }
            })
            .collect();

        while pending.next().await.is_some() {}
    }

    /// Join the post-consensus execution for `(committee, slot)`, returning the outcome and the
    /// deadline that bounded the join, for the caller to reuse when draining its own roots.
    pub(crate) async fn post_consensus_outcome(
        &self,
        committee_id: CommitteeId,
        slot: Slot,
    ) -> Result<(Instant, Arc<AggregatorPostConsensusOutcome<E>>), Error> {
        // Waiting on the assignments is what makes the lookup below race-free: the execution was
        // registered before this slot's assignments were published.
        self.get_aggregation_assignments(slot).await?;

        let execution = self
            .aggregator_post_consensus
            .lock()
            .get(&(committee_id, slot))
            .cloned()
            .ok_or(Error::SpecificError(SpecificError::ConsensusDataNotFound))?;

        self.join_execution(slot, execution).await
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

        // Decode based on fork version
        let decided_aggregate = if decided_data.version < DataVersion::from(ForkName::Electra) {
            AttestationBase::from_ssz_bytes(decided_aggregate_bytes)
                .map(Attestation::Base)
                .map_err(|e| format!("Failed to decode decided aggregate: {e:?}"))?
        } else {
            AttestationElectra::from_ssz_bytes(decided_aggregate_bytes)
                .map(Attestation::Electra)
                .map_err(|e| format!("Failed to decode decided aggregate: {e:?}"))?
        };

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
