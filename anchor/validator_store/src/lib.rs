mod aggregator_post_consensus;
mod instrumentation;
pub mod metadata_service;
mod metrics;
pub mod registration_service;
use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    future::Future,
    num::NonZeroUsize,
    str::from_utf8,
    sync::{Arc, LazyLock},
    time::Duration,
};

use bls::{PublicKeyBytes, SecretKey, Signature};
use database::{NetworkDatabase, NonUniqueIndex, UniqueIndex};
use eth2::types::{BlockContents, BlockContentsTuple, FullBlockContents, PublishBlockRequest};
use fork::{Fork, ForkSchedule};
use futures::{
    Stream, StreamExt,
    future::{Either, join_all},
    stream,
    stream::FuturesUnordered,
};
use lru::LruCache;
use openssl::{
    pkey::Private,
    rsa::{Padding, Rsa},
};
use parking_lot::Mutex;
use qbft::Completed;
use qbft_manager::{
    AggregatorCommitteeInstanceId, CommitteeInstanceId, ConsensusDecider, ProposerInstanceId,
    QbftError, QbftManager, TimeoutMode, ValidatorDutyKind,
};
use safe_arith::{ArithError, SafeArith};
use signature_collector::{
    CollectionError, SignatureCollecting, SignatureMetadata, SignatureRequester,
    SyncCommitteeBatchEntry, ValidatorSigningData,
};
use slashing_protection::{CheckSlashability, NotSafe, Safe, SlashingDatabase};
use slot_clock::SlotClock;
use ssv_types::{
    Cluster, ClusterId, CommitteeId, ENCRYPTED_KEY_LENGTH, MAX_SYNC_COMMITTEE_POSITIONS,
    ValidatorIndex, ValidatorMetadata,
    consensus::{
        AggregatorCommitteeConsensusData, AggregatorCommitteeDataValidator, BEACON_ROLE_AGGREGATOR,
        BEACON_ROLE_PROPOSER, BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION, BeaconVote,
        BeaconVoteValidator, Contribution, ContributionWrapper, Contributions,
        ProposerConsensusData, ProposerConsensusDataValidator, QbftData, SelectionProofBatchId,
        ValidatorDuty,
    },
    msgid::Role,
    partial_sig::PartialSignatureKind,
    try_to_variable_list,
    typenum::Unsigned,
};
use ssz::{Decode, DecodeError, Encode};
use task_executor::TaskExecutor;
use tokio::{
    select,
    sync::{Barrier, RwLock, watch},
    time::{Instant, sleep},
};
use tracing::{Instrument, Span, debug, error, field, info, info_span, trace, warn};
use types::{
    AbstractExecPayload, Address, AggregateAndProof, AggregateAndProofBase,
    AggregateAndProofElectra, Attestation, BeaconBlock, BeaconBlockRef, BlindedPayload, ChainSpec,
    ContributionAndProof, Domain, Epoch, EthSpec, ExecutionPayloadEnvelope, ForkName, FullPayload,
    Graffiti, Hash256, PayloadAttestationData, PayloadAttestationMessage, ProposerPreferences,
    SelectionProof, SignedAggregateAndProof, SignedBeaconBlock, SignedBlindedBeaconBlock,
    SignedContributionAndProof, SignedExecutionPayloadEnvelope, SignedProposerPreferences,
    SignedRoot, SignedValidatorRegistrationData, SignedVoluntaryExit, Slot, SlotData,
    SyncAggregatorSelectionData, SyncCommitteeContribution, SyncCommitteeMessage,
    SyncSelectionProof, SyncSubnetId, ValidatorRegistrationData, VoluntaryExit,
};
use validator_metrics::IntCounterVec;
use validator_store::{
    AggregateToSign, AttestationToSign, ContributionToSign, DoppelgangerStatus,
    Error as ValidatorStoreError, ProposalData, SignedBlock, SyncMessageToSign, UnsignedBlock,
    ValidatorStore,
};

use crate::aggregator_post_consensus::AggregatorPostConsensusShared;

/// Number of epochs of slashing protection history to keep.
///
/// This acts as a maximum safe-guard against clock drift.
const SLASHING_PROTECTION_HISTORY_EPOCHS: u64 = 512;

const MAX_VALIDATORS_PER_OPERATOR: NonZeroUsize =
    NonZeroUsize::new(3000).expect("3000 is non-zero");

const RANDAO_REVEAL_LOG_NAME: &str = "RANDAO reveal";
const BLOCK_LOG_NAME: &str = "block";
const VALIDATOR_REGISTRATION_LOG_NAME: &str = "validator registration";
const AGGREGATE_LOG_NAME: &str = "aggregate";
const SELECTION_PROOF_LOG_NAME: &str = "selection proof";
const SYNC_SELECTION_PROOF_LOG_NAME: &str = "sync selection proof";
const SYNC_COMMITTEE_CONTRIBUTION_LOG_NAME: &str = "sync committee contribution";

/// A request to collect a committee signature for a single validator.
///
/// The shared fields (`validator`, `signing_root`) drive `collect_prepared_signatures`,
/// while `duty_data` carries duty-specific context needed by the assembly step.
#[derive(Clone)]
struct SigningRequest<T> {
    validator: ValidatorMetadata,
    signing_root: Hash256,
    duty_data: T,
}

impl<T> SigningRequest<T> {
    /// Look up this request's collected signature, consuming the request.
    ///
    /// Returns `Ok((duty_data, signature))` on success, or `Err(pubkey)` if
    /// the validator index is missing or no signature was collected.
    ///
    /// Keyed by `(index, root)` rather than index alone because one validator can hold several
    /// distinct roots in a single batch (an `AggregatorCommittee` duty allows one aggregate plus
    /// one contribution per subcommittee).
    fn resolve(self, signatures: &CollectedSignatures) -> Result<(T, Signature), PublicKeyBytes> {
        let sig = self
            .validator
            .index
            .and_then(|idx| signatures.get(&(idx, self.signing_root)).cloned())
            .ok_or(self.validator.public_key)?;
        Ok((self.duty_data, sig))
    }
}

/// Signatures collected for one committee batch, keyed by the request that produced each.
type CollectedSignatures = HashMap<(ValidatorIndex, Hash256), Signature>;

/// Drive signature collection to completion, keeping each result as it resolves.
///
/// Results are taken one at a time rather than as a group so that a root which never reaches
/// quorum withholds only itself; callers surface missing roots through
/// `SigningRequest::resolve` failing for the affected request.
async fn drain_signatures<F, E>(mut pending: FuturesUnordered<F>) -> CollectedSignatures
where
    F: Future<Output = (ValidatorIndex, Hash256, Result<Signature, E>)>,
    E: Debug,
{
    let mut signatures = HashMap::with_capacity(pending.len());
    while let Some((index, signing_root, result)) = pending.next().await {
        match result {
            Ok(signature) => {
                signatures.insert((index, signing_root), signature);
            }
            Err(e) => {
                error!(?index, ?signing_root, error = ?e, "Failed to collect signature");
            }
        }
    }
    signatures
}

/// Handle committee signing errors with consistent timeout/failure metrics.
///
/// On success, returns the signed results. On timeout or failure, logs,
/// increments per-validator metrics, and returns an empty vec.
async fn run_committee_signing<T>(
    committee_id: CommitteeId,
    count: usize,
    counter: &LazyLock<validator_metrics::Result<IntCounterVec>>,
    fut: impl Future<Output = Result<Vec<T>, Error>>,
) -> Result<Vec<T>, Error> {
    match fut.await {
        Ok(signed) => Ok(signed),
        Err(Error::SpecificError(SpecificError::Timeout)) => {
            warn!(?committee_id, "Committee signing timed out");
            for _ in 0..count {
                validator_metrics::inc_counter_vec(counter, &[metrics::TIMEOUT]);
            }
            Ok(Vec::new())
        }
        Err(e) => {
            error!(?committee_id, error = ?e, "Committee signing failed");
            for _ in 0..count {
                validator_metrics::inc_counter_vec(counter, &[metrics::OTHER_ERROR]);
            }
            Ok(Vec::new())
        }
    }
}

fn determine_slot_elapsed_ms(slot_clock: &impl SlotClock) -> Option<u64> {
    slot_clock
        .millis_from_current_slot_start()
        .map(|d| d.as_millis() as u64)
}

pub struct AnchorValidatorStore<
    T: SlotClock + 'static,
    E: EthSpec,
    C: ConsensusDecider<E> = QbftManager<E, T>,
> {
    database: Arc<NetworkDatabase>,
    decrypted_keys: Mutex<LruCache<[u8; ENCRYPTED_KEY_LENGTH], SecretKey>>,
    signature_collector: Box<dyn SignatureCollecting>,
    consensus: Arc<C>,
    slashing_protection: Arc<SlashingDatabase>,
    slashing_protection_last_prune: Mutex<Epoch>,
    disable_slashing_protection: bool,
    slot_clock: T,
    spec: Arc<ChainSpec>,
    genesis_validators_root: Hash256,
    private_key: Option<Rsa<Private>>,
    fork_schedule: Arc<ForkSchedule>,
    voting_context_tx: watch::Sender<Option<Arc<VotingContext>>>,
    /// Watch channel for `VotingAssignments` (cached at slot start)
    voting_assignments_tx: watch::Sender<Option<Arc<VotingAssignments>>>,
    /// Watch channel for `AggregationAssignments` (cached at 2/3 slot)
    aggregation_assignments_tx: watch::Sender<Option<Arc<AggregationAssignments<E>>>>,
    gas_limit: u64,
    // MEV configuration is applied at the operator level and applies to all validators this
    // operator controls
    builder_boost_factor: Option<u64>,
    prefer_builder_proposals: bool,
    /// See [`await_proposer_delay`] for the semantics.
    proposer_delay: Duration,
    strict_mfp: bool,
    is_synced: watch::Receiver<bool>,
    task_executor: TaskExecutor,
    /// `(committee, slot)` keys whose Boole+ `AggregatorCommittee` post-consensus execution has
    /// already been started.
    ///
    /// Registered by the slot pipeline in [`Self::update_aggregation_assignments`] before those
    /// assignments are published, first-insert-only. That registration is the single writer, so
    /// the complete decided worklist is signed and batched exactly once no matter how often the
    /// pipeline runs. The execution handles themselves travel to their sole consumer (the
    /// metadata service's publisher) through the registration's return value; this set only
    /// provides registration idempotence across the retention window.
    aggregator_post_consensus: Mutex<HashSet<(CommitteeId, Slot)>>,
}

/// How far into `slot` the clock currently is, or `None` if the clock cannot answer.
///
/// Measured against the slot *named*, so the result is comparable to a delay target defined against
/// that same slot. [`determine_slot_elapsed_ms`] is not usable here: it is relative to the current
/// slot and wraps modulo the slot duration, so a duty that overran would read as a fresh one and
/// re-arm the delay.
fn elapsed_in_slot(slot_clock: &impl SlotClock, slot: Slot) -> Option<Duration> {
    let start = slot_clock.start_of(slot)?;
    // Both are durations since the UNIX epoch. `None` also covers a clock reporting a time before
    // the slot began, which fails open like any other unreadable clock.
    slot_clock.now_duration()?.checked_sub(start)
}

/// Outcome of evaluating the configured proposer delay for one proposer duty.
///
/// The no-wait cases are kept distinct because they are the operator's diagnostic: "off", "the
/// floor did not bite", and "no clock" otherwise all render as a flat zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProposerDelayDecision {
    Disabled,
    /// Configured, but the target offset had already passed.
    TargetPassed,
    Waited(Duration),
    /// Skipped: the clock could not say how far into the slot we are.
    ClockUnavailable,
}

/// Decides the proposer delay from timings alone, so the policy is testable without a store.
///
/// `elapsed` comes from [`elapsed_in_slot`]; `None` fails open.
fn proposer_delay_decision_at(
    proposer_delay: Duration,
    elapsed: Option<Duration>,
) -> ProposerDelayDecision {
    if proposer_delay.is_zero() {
        return ProposerDelayDecision::Disabled;
    }
    let Some(elapsed) = elapsed else {
        return ProposerDelayDecision::ClockUnavailable;
    };
    match proposer_delay.checked_sub(elapsed) {
        Some(remaining) if !remaining.is_zero() => ProposerDelayDecision::Waited(remaining),
        _ => ProposerDelayDecision::TargetPassed,
    }
}

impl ProposerDelayDecision {
    fn wait(self) -> Option<Duration> {
        match self {
            ProposerDelayDecision::Waited(duration) => Some(duration),
            ProposerDelayDecision::Disabled
            | ProposerDelayDecision::TargetPassed
            | ProposerDelayDecision::ClockUnavailable => None,
        }
    }

    /// Low-cardinality label for metrics and spans.
    fn as_str(self) -> &'static str {
        match self {
            ProposerDelayDecision::Disabled => "disabled",
            ProposerDelayDecision::TargetPassed => "target_passed",
            ProposerDelayDecision::Waited(_) => "waited",
            ProposerDelayDecision::ClockUnavailable => "clock_unavailable",
        }
    }
}

/// Holds this proposer duty until `proposer_delay` into its slot, recording the outcome either way,
/// including the no-wait cases.
///
/// The delay is a *floor* from the start of the slot, not extra latency: a duty whose RANDAO
/// pre-consensus already ran past it waits no longer. This matches go-ssv's `ProposerDelay`, so the
/// same configured value yields the same request time on either client.
///
/// `elapsed` is passed in rather than re-read so the wait and
/// [`metrics::RANDAO_REVEAL_COMPLETION_OFFSET`] share one measurement; operators are told to
/// compare them. Missing slot timing fails open. When the delay *does* apply it knowingly spends
/// proposal headroom, which is the trade the operator opted into.
async fn await_proposer_delay(proposer_delay: Duration, elapsed: Option<Duration>) {
    let decision = proposer_delay_decision_at(proposer_delay, elapsed);
    let outcome = decision.as_str();

    // Measure the real sleep, not the plan: both readings are documented as the wait applied.
    let waited = match decision.wait() {
        Some(planned) => {
            let started = Instant::now();
            sleep(planned).await;
            started.elapsed()
        }
        None => Duration::ZERO,
    };

    Span::current().record("proposer_delay_outcome", outcome);
    Span::current().record("proposer_delay_waited_ms", waited.as_millis() as u64);
    metrics::observe_timer_vec(&metrics::PROPOSER_DELAY_APPLIED, &[outcome], waited);

    if let ProposerDelayDecision::ClockUnavailable = decision {
        warn!(
            checkpoint = instrumentation::checkpoints::PROPOSER_DELAY_APPLIED,
            outcome,
            "Slot timing unreadable (clock unavailable or reported time before slot start), \
             skipping configured proposer delay"
        );
    } else {
        trace!(
            checkpoint = instrumentation::checkpoints::PROPOSER_DELAY_APPLIED,
            outcome,
            waited_ms = waited.as_millis() as u64,
            "Proposer delay evaluated"
        );
    }
}

impl<T: SlotClock, E: EthSpec, C: ConsensusDecider<E> + 'static> AnchorValidatorStore<T, E, C> {
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        database: Arc<NetworkDatabase>,
        signature_collector: Box<dyn SignatureCollecting>,
        consensus: Arc<C>,
        slashing_protection: Arc<SlashingDatabase>,
        disable_slashing_protection: bool,
        slot_clock: T,
        spec: Arc<ChainSpec>,
        genesis_validators_root: Hash256,
        private_key: Option<Rsa<Private>>,
        fork_schedule: Arc<ForkSchedule>,
        gas_limit: u64,
        builder_boost_factor: Option<u64>,
        prefer_builder_proposals: bool,
        proposer_delay: Duration,
        strict_mfp: bool,
        is_synced: watch::Receiver<bool>,
        task_executor: TaskExecutor,
    ) -> Arc<AnchorValidatorStore<T, E, C>> {
        Arc::new(Self {
            database,
            decrypted_keys: Mutex::new(LruCache::new(MAX_VALIDATORS_PER_OPERATOR)),
            signature_collector,
            consensus,
            slashing_protection,
            slashing_protection_last_prune: Mutex::new(Epoch::new(0)),
            disable_slashing_protection,
            slot_clock,
            spec,
            genesis_validators_root,
            private_key,
            fork_schedule,
            voting_context_tx: watch::channel(None).0,
            voting_assignments_tx: watch::channel(None).0,
            aggregation_assignments_tx: watch::channel(None).0,
            gas_limit,
            builder_boost_factor,
            prefer_builder_proposals,
            proposer_delay,
            strict_mfp,
            is_synced,
            task_executor,
            aggregator_post_consensus: Mutex::new(HashSet::new()),
        })
    }

    fn get_validator_and_cluster(
        &self,
        validator_pubkey: PublicKeyBytes,
    ) -> Result<(ValidatorMetadata, Cluster), Error> {
        let state = self.database.state();
        let validator = state
            .metadata()
            .get_by(&validator_pubkey)
            .ok_or(Error::UnknownPubkey(validator_pubkey))?
            .clone();

        // First, attempt to get the cluster normally
        if let Some(cluster) = state.clusters().get_by(&validator.cluster_id) {
            if cluster.liquidated {
                return Err(Error::SpecificError(SpecificError::ClusterLiquidated));
            }
            return Ok((validator, cluster.clone()));
        }

        // If cluster is missing, this indicates a database inconsistency
        // Log the error with context
        error!(
            validator_pubkey = %validator_pubkey,
            cluster_id = ?validator.cluster_id,
            "Database inconsistency detected: validator references non-existent cluster"
        );

        // Return specific error with context for potential recovery
        Err(Error::SpecificError(
            SpecificError::ValidatorClusterMismatch {
                validator_pubkey,
                cluster_id: validator.cluster_id,
            },
        ))
    }

    fn get_domain(&self, epoch: Epoch, domain: Domain) -> Hash256 {
        self.spec.get_domain(
            epoch,
            domain,
            &self.spec.fork_at_epoch(epoch),
            self.genesis_validators_root,
        )
    }

    /// Get the set of validator indices for validators we have shares for in a committee.
    ///
    /// This is used to filter decided consensus data to only validators we can sign for.
    fn get_committee_validator_indices(
        &self,
        committee_id: &CommitteeId,
    ) -> HashSet<ValidatorIndex> {
        let state = self.database.state();
        state
            .metadata()
            .get_all_by(committee_id)
            .filter_map(|v| v.index)
            .collect()
    }

    /// Group items by SSV committee, looking up each item's cluster.
    ///
    /// Items with unknown pubkeys or cluster lookup failures are logged and skipped.
    fn group_by_committee<I>(
        &self,
        items: Vec<I>,
        get_pubkey: impl Fn(&I) -> PublicKeyBytes,
    ) -> HashMap<CommitteeId, (Cluster, Vec<(ValidatorMetadata, I)>)> {
        let mut mapping: HashMap<CommitteeId, (Cluster, Vec<(ValidatorMetadata, I)>)> =
            HashMap::new();
        for item in items {
            let pubkey = get_pubkey(&item);
            match self.get_validator_and_cluster(pubkey) {
                Ok((validator, cluster)) => {
                    let committee_id = cluster.committee_id();
                    let (_, validators) = mapping
                        .entry(committee_id)
                        .or_insert_with(|| (cluster, Vec::new()));
                    validators.push((validator, item));
                }
                Err(Error::UnknownPubkey(pk)) => {
                    warn!(?pk, "Unknown pubkey while grouping, skipping");
                }
                Err(e) => {
                    error!(error = ?e, ?pubkey, "Failed to get cluster, skipping");
                }
            }
        }
        mapping
    }

    /// Collect committee signatures for a batch of signing requests.
    ///
    /// Shared across all `sign_committee_*` methods. It builds `CollectionMode::Committee`,
    /// runs `collect_signature` concurrently for each validator, and returns the signatures.
    /// The `validator_partial_signature_batch_size` parameter is the number of validator partial
    /// signatures this operator puts into one outgoing committee message. It is not the threshold
    /// for reconstructing a signature from other operators' shares.
    async fn collect_prepared_signatures<D>(
        &self,
        role: Role,
        slot: Slot,
        cluster: &Cluster,
        validator_partial_signature_batch_size: usize,
        data_hash: Hash256,
        prepared: &[SigningRequest<D>],
    ) -> Result<CollectedSignatures, Error> {
        let collection_mode = CollectionMode::Committee {
            validator_partial_signature_batch_size,
            base_hash: data_hash,
        };

        let pending: FuturesUnordered<_> = prepared
            .iter()
            .filter_map(|item| {
                let index = item.validator.index?;
                let collection_mode = collection_mode.clone();
                Some(async move {
                    let result = self
                        .collect_signature(
                            PartialSignatureKind::PostConsensus,
                            role,
                            collection_mode,
                            &item.validator,
                            cluster,
                            item.signing_root,
                            slot,
                        )
                        .await;
                    (index, item.signing_root, result)
                })
            })
            .collect();

        Ok(drain_signatures(pending).await)
    }

    /// Run `AggregatorCommittee` QBFT consensus for a committee at 2/3 slot.
    ///
    /// Called once per `(committee, slot)` by the post-consensus execution in
    /// [`crate::aggregator_post_consensus`], which supplies the value this operator proposes.
    pub(crate) async fn run_aggregator_committee_consensus(
        &self,
        committee_id: CommitteeId,
        slot: Slot,
        cluster: &Cluster,
        our_consensus_data: &AggregatorCommitteeConsensusData<E>,
    ) -> Result<AggregatorCommitteeConsensusData<E>, Error> {
        let timer =
            metrics::start_timer_vec(&metrics::CONSENSUS_TIMES, &[metrics::AGGREGATOR_COMMITTEE]);
        let timeout_mode = TimeoutMode::SlotTime {
            instance_start_time: self
                .get_instant_in_slot(slot, self.spec.get_slot_duration() * 2 / 3)?,
        };

        let completed = self
            .consensus
            .decide_instance(
                AggregatorCommitteeInstanceId {
                    committee: committee_id,
                    instance_height: slot.as_usize().into(),
                },
                our_consensus_data.clone(),
                Box::new(AggregatorCommitteeDataValidator::new()),
                timeout_mode,
                &cluster.cluster_members,
            )
            .await
            .map_err(SpecificError::from)?;
        drop(timer);

        match completed {
            Completed::TimedOut => Err(Error::SpecificError(SpecificError::Timeout)),
            Completed::Success(data) => Ok(data),
        }
    }

    /// Compute the signing root for a sync committee selection proof.
    ///
    /// Each subnet has a different signing root based on `SyncAggregatorSelectionData{Slot,
    /// SubcommitteeIndex}`.
    pub fn compute_sync_selection_root(&self, slot: Slot, subnet_id: u64) -> Hash256 {
        let epoch = slot.epoch(E::slots_per_epoch());
        let domain = self.get_domain(epoch, Domain::SyncCommitteeSelectionProof);
        SyncAggregatorSelectionData {
            slot,
            subcommittee_index: subnet_id,
        }
        .signing_root(domain)
    }

    fn sync_selection_proof_descriptor(
        &self,
        slot: Slot,
        callback_subnet: SyncSubnetId,
        position_counts: &HashMap<SyncSubnetId, usize>,
    ) -> Result<Vec<SyncCommitteeBatchEntry>, SyncSelectionProofAssignmentError> {
        if position_counts.is_empty() {
            return Err(SyncSelectionProofAssignmentError::Empty);
        }
        if !position_counts.contains_key(&callback_subnet) {
            return Err(SyncSelectionProofAssignmentError::MissingCallbackSubnet {
                subnet_id: callback_subnet,
            });
        }

        let subnet_count = E::SyncCommitteeSubnetCount::to_u64();
        if let Some(subnet_id) = position_counts
            .keys()
            .copied()
            .filter(|subnet_id| u64::from(*subnet_id) >= subnet_count)
            .min_by_key(|subnet_id| u64::from(*subnet_id))
        {
            return Err(SyncSelectionProofAssignmentError::OutOfRangeSubnet {
                subnet_id,
                subnet_count,
            });
        }

        // Total the whole assignment before enforcing the cap. Accumulating while constructing the
        // descriptor would report a partial sum that depends on `HashMap` iteration order.
        let expanded_positions = position_counts
            .values()
            .copied()
            .fold(0usize, usize::saturating_add);
        if expanded_positions > MAX_SYNC_COMMITTEE_POSITIONS {
            return Err(SyncSelectionProofAssignmentError::TooManyPositions {
                count: expanded_positions,
                max: MAX_SYNC_COMMITTEE_POSITIONS,
            });
        }

        let mut descriptor = Vec::with_capacity(position_counts.len());
        for (&subnet_id, &position_count) in position_counts {
            descriptor.push(SyncCommitteeBatchEntry {
                subnet_id,
                signing_root: self.compute_sync_selection_root(slot, subnet_id.into()),
                multiplicity: position_count,
            });
        }

        // Numeric subnet order makes retries byte-identical. Since
        // `subnet = position / subcommittee_size` is monotonic, ascending duty positions produce
        // the same root order, and repeated positions within one subnet share a root. Receiver
        // correctness remains multiset-based and does not depend on this order.
        descriptor.sort_unstable_by_key(|entry| u64::from(entry.subnet_id));
        Ok(descriptor)
    }

    #[expect(clippy::too_many_arguments)]
    async fn collect_signature(
        &self,
        signature_kind: PartialSignatureKind,
        role: Role,
        collection_mode: CollectionMode,
        validator: &ValidatorMetadata,
        cluster: &Cluster,
        signing_root: Hash256,
        slot: Slot,
    ) -> Result<Signature, Error> {
        let committee_id = cluster.committee_id();
        let metadata = SignatureMetadata {
            kind: signature_kind,
            role,
            threshold: cluster
                .get_f()
                .safe_mul(2)
                .and_then(|x| x.safe_add(1))
                .map_err(SpecificError::from)?,
            slot,
            committee_id,
        };

        let (requester, encrypted_private_key) = {
            let state = self.database.state();
            let requester = match collection_mode {
                CollectionMode::SingleValidator => SignatureRequester::SingleValidator {
                    pubkey: validator.public_key,
                },
                CollectionMode::SingleValidatorBatch {
                    subnet_id,
                    descriptor,
                } => SignatureRequester::SingleValidatorBatch {
                    pubkey: validator.public_key,
                    subnet_id,
                    descriptor,
                },
                CollectionMode::Committee {
                    validator_partial_signature_batch_size,
                    base_hash,
                } => SignatureRequester::Committee {
                    validator_partial_signature_batch_size,
                    base_hash,
                },
            };
            let encrypted_private_key = state
                .shares()
                .get_by(&validator.public_key)
                .ok_or(Error::UnknownPubkey(validator.public_key))?
                .encrypted_private_key;
            (requester, encrypted_private_key)
        };

        let decrypted_key_share = if let Some(operator_key) = &self.private_key {
            let key = self
                .decrypted_keys
                .lock()
                .try_get_or_insert(encrypted_private_key, || {
                    decrypt_key_share(operator_key, encrypted_private_key, validator.public_key)
                        .map_err(|_| SpecificError::KeyShareDecryptionFailed)
                })
                .cloned()?;
            Some(key)
        } else {
            // We are in imposter mode and cannot decrypt the share.
            None
        };

        let signing_data = ValidatorSigningData {
            root: signing_root,
            index: validator.index.ok_or(SpecificError::MissingIndex)?,
            validator_pubkey: validator.public_key,
            share: decrypted_key_share,
        };

        let _timer =
            validator_metrics::start_timer_vec(&validator_metrics::SIGNING_TIMES, &["ssv"]);

        let collector =
            self.signature_collector
                .sign_and_collect(metadata, requester, signing_data);
        Ok((*collector.await.map_err(SpecificError::from)?).clone())
    }

    async fn decide_abstract_block(
        &self,
        validator: &ValidatorMetadata,
        cluster: &Cluster,
        signable_block: &impl SignableBlock<E>,
    ) -> Result<UnsignedBlock<E>, Error> {
        let block = signable_block.as_block();
        let slot = block.slot();

        // first, we have to get to consensus
        let timer = metrics::start_timer_vec(&metrics::CONSENSUS_TIMES, &[metrics::BLOCK]);
        let timeout_mode = TimeoutMode::Relative {
            current_round_start_time: self.get_instant_in_slot(slot, Duration::ZERO)?,
        };

        // Define the proposer instance identity for QBFT consensus
        let instance_id = ProposerInstanceId {
            validator: validator.public_key,
            duty: ValidatorDutyKind::Proposal,
            instance_height: slot.as_usize().into(),
        };

        // Get the validator index, ensuring it exists
        let validator_index = validator.index.ok_or(SpecificError::MissingIndex)?;

        // Determine the appropriate version based on block type
        let block_version = block.fork_name_unchecked().into();

        // Create the validator duty information
        let validator_duty = ValidatorDuty {
            r#type: BEACON_ROLE_PROPOSER,
            pub_key: validator.public_key,
            slot,
            validator_index,
            committee_index: 0,
            committee_length: 0,
            committees_at_slot: 0,
            validator_committee_index: 0,
            validator_sync_committee_indices: Default::default(),
        };

        // Package the consensus data
        let consensus_data = ProposerConsensusData {
            duty: validator_duty,
            version: block_version,
            data_ssz: try_to_variable_list(signable_block.as_ssz_bytes(), |provided, max| {
                Error::SpecificError(SpecificError::DataTooLarge(format!(
                    "Block data too large for consensus: {} > {}",
                    provided, max
                )))
            })?,
        };

        let data_validator = self.create_proposer_consensus_data_validator(validator.public_key);

        // Initiate QBFT consensus for this block proposal
        let completed = self
            .consensus
            .decide_instance(
                instance_id,
                consensus_data,
                data_validator,
                timeout_mode,
                &cluster.cluster_members,
            )
            .await
            .map_err(SpecificError::from)?;
        drop(timer);

        let completed_data = match completed {
            Completed::TimedOut => return Err(Error::SpecificError(SpecificError::Timeout)),
            Completed::Success(data) => data,
        };

        completed_data
            .decode_blinded_block()
            .map(UnsignedBlock::Blinded)
            .or_else(|_| {
                completed_data
                    .decode_block_contents()
                    .map(UnsignedBlock::Full)
            })
            .map_err(|err| Error::SpecificError(SpecificError::InvalidQbftData(err)))
    }

    async fn sign_abstract_block(
        &self,
        validator: &ValidatorMetadata,
        cluster: &Cluster,
        signable_block: impl SignableBlock<E>,
        current_slot: Slot,
    ) -> Result<SignedBlock<E>, Error> {
        debug!(signable_block = ?signable_block.as_block().block_header(), "Decided on BeaconBlock to sign");

        let block = signable_block.as_block();

        // Make sure the block slot is not higher than the current slot to avoid potential attacks.
        if block.slot() > current_slot {
            warn!(
                block_slot = block.slot().as_u64(),
                current_slot = current_slot.as_u64(),
                "Not signing block with slot greater than current slot",
            );
            return Err(Error::GreaterThanCurrentSlot {
                slot: block.slot(),
                current_slot,
            });
        }

        let domain_hash = self.get_domain(block.epoch(), Domain::BeaconProposer);

        let header = block.block_header();

        if !self.disable_slashing_protection {
            convert_slashing_result(self.slashing_protection.check_and_insert_block_proposal(
                &validator.public_key,
                &header,
                domain_hash,
            ))?;
        }

        let signing_root = block.signing_root(domain_hash);
        let signature = self
            .collect_signature(
                PartialSignatureKind::PostConsensus,
                Role::Proposer,
                CollectionMode::SingleValidator,
                validator,
                cluster,
                signing_root,
                header.slot,
            )
            .await?;
        Ok(signable_block.to_signed_block(signature))
    }

    /// Get the [`VotingContext`] for the given [`Slot`], waiting for it to become available if
    /// necessary. If the requested slot has already passed, an error is returned.
    ///
    /// IMPORTANT: The voting context is computed starting at 1/3rd into the slot - so do not try
    /// to retrieve it if sleeping until then is not tolerable.
    async fn get_voting_context(&self, slot: Slot) -> Result<Arc<VotingContext>, Error> {
        let Some(metadata) = self
            .voting_context_tx
            .subscribe()
            .wait_for(|m| {
                m.as_ref()
                    .is_some_and(|metadata| metadata.voting_assignments.slot >= slot)
            })
            .await
            .ok()
            .and_then(|metadata| metadata.clone())
        else {
            error!(%slot, "Unexpected error while waiting for metadata");
            return Err(Error::SpecificError(SpecificError::Metadata));
        };

        if metadata.voting_assignments.slot == slot {
            Ok(metadata)
        } else {
            error!("Got newer metadata - performance issues?");
            Err(Error::SpecificError(SpecificError::Metadata))
        }
    }

    fn update_voting_context(&self, metadata: VotingContext) {
        self.voting_context_tx
            .send_replace(Some(Arc::new(metadata)));
    }

    /// Get validator voting assignments, waiting if not yet available for this slot.
    ///
    /// This method waits until `VotingAssignments` for the requested slot becomes available.
    /// Returns an error if the requested slot has already passed or if the watch channel is closed.
    pub async fn get_voting_assignments(
        &self,
        slot: Slot,
    ) -> Result<Arc<VotingAssignments>, Error> {
        let Some(voting_assignments) = self
            .voting_assignments_tx
            .subscribe()
            .wait_for(|d| d.as_ref().is_some_and(|info| info.slot >= slot))
            .await
            .ok()
            .and_then(|info| info.clone())
        else {
            return Err(Error::SpecificError(SpecificError::MetadataChannelClosed));
        };

        if voting_assignments.slot == slot {
            Ok(voting_assignments)
        } else {
            Err(Error::SpecificError(SpecificError::MetadataSlotPassed))
        }
    }

    /// Update validator voting assignments (called by `MetadataService` at slot start).
    ///
    /// This publishes the `VotingAssignments` to all subscribers via the watch channel.
    pub fn update_voting_assignments(&self, voting_assignments: VotingAssignments) {
        self.voting_assignments_tx
            .send_replace(Some(Arc::new(voting_assignments)));
    }

    /// Get aggregator voting assignments, waiting if not yet available for this slot.
    ///
    /// This method waits until `AggregationAssignments` for the requested slot becomes available.
    /// Called by `run_aggregator_post_consensus` (via `run_aggregator_committee_consensus`)
    /// at 2/3 slot.
    ///
    /// Returns an error if the requested slot has already passed or if the watch channel is closed.
    pub async fn get_aggregation_assignments(
        &self,
        slot: Slot,
    ) -> Result<Arc<AggregationAssignments<E>>, Error> {
        let Some(aggregator_info) = self
            .aggregation_assignments_tx
            .subscribe()
            .wait_for(|a| a.as_ref().is_some_and(|info| info.slot >= slot))
            .await
            .ok()
            .and_then(|info| info.clone())
        else {
            return Err(Error::SpecificError(
                SpecificError::AggregatorInfoChannelClosed,
            ));
        };

        if aggregator_info.slot == slot {
            Ok(aggregator_info)
        } else {
            Err(Error::SpecificError(
                SpecificError::AggregatorInfoSlotPassed,
            ))
        }
    }

    /// Update aggregator voting assignments (called by `MetadataService` Phase 3 at 2/3 slot).
    ///
    /// This publishes the `AggregationAssignments` to all subscribers via the watch channel.
    /// At 2/3 slot, selection proofs have been computed by Lighthouse, so
    /// `DutyAndProof.selection_proof.is_some()` accurately indicates `is_aggregator`.
    ///
    /// It also starts each Boole+ committee's post-consensus signing execution, before publishing,
    /// so that every consumer able to observe these assignments can also observe the execution they
    /// belong to. See [`crate::aggregator_post_consensus`].
    ///
    /// Returns the executions newly registered by this call, for the caller to hand to the
    /// aggregate publisher. Only vacant insertions are returned, so repeated calls for one slot
    /// cannot register a second publisher for the same `(committee, slot)`.
    #[must_use = "dropping the returned executions disables Boole+ aggregate publication"]
    pub(crate) fn update_aggregation_assignments(
        self: &Arc<Self>,
        info: AggregationAssignments<E>,
    ) -> Vec<(CommitteeId, AggregatorPostConsensusShared<E>)> {
        let new_executions = self.start_aggregator_post_consensus(&info);
        self.aggregation_assignments_tx
            .send_replace(Some(Arc::new(info)));
        new_executions
    }

    /// Return [`SpecificError::Timeout`] if the given future does not complete at `delay` into the
    /// given slot.
    ///
    /// In the unlikely case the `slot_clock` errors, we time out after `delay`.
    async fn timeout_within_slot<O>(
        &self,
        slot: Slot,
        delay: Duration,
        future: impl Future<Output = Result<O, impl Into<Error>>>,
    ) -> Result<O, Error> {
        let timeout_time = self
            .slot_clock
            .start_of(slot)
            .and_then(|start| {
                self.slot_clock
                    .now_duration()
                    .map(|now| (start + delay).saturating_sub(now))
            })
            .unwrap_or(delay);

        select! {
            result = future => {
                result.map_err(Into::into)
            },
            _ = sleep(timeout_time) => {
                Err(SpecificError::Timeout.into())
            }
        }
    }

    fn get_instant_in_slot(&self, slot: Slot, delay: Duration) -> Result<Instant, Error> {
        // We can calculate an instant only by adding a duration to the current instant.

        // First, we get the duration since unix epoch to the target time.
        let target_duration = self
            .slot_clock
            .start_of(slot)
            .map(|start| start + delay)
            .ok_or(SpecificError::SlotClock)?;
        // Then, we get the current time as duration since unix epoch.
        let now_duration = self
            .slot_clock
            .now_duration()
            .ok_or(SpecificError::SlotClock)?;
        // We calculate the difference and add or substract it depending on whether the target is
        // before or after the current time.
        let difference = target_duration.abs_diff(now_duration);
        let instant = if target_duration > now_duration {
            Instant::now() + difference
        } else {
            Instant::now() - difference
        };
        Ok(instant)
    }

    pub async fn collect_voluntary_exit_partial_signatures(
        &self,
        validator_pubkey: PublicKeyBytes,
        voluntary_exit: VoluntaryExit,
        slot: Slot,
    ) -> Result<SignedVoluntaryExit, Error> {
        let spec = self.spec.clone();
        let domain_hash = voluntary_exit.get_domain(self.genesis_validators_root, &spec);
        let signing_root = voluntary_exit.signing_root(domain_hash);
        let (validator, cluster) = self.get_validator_and_cluster(validator_pubkey)?;

        let signature = self
            .collect_signature(
                PartialSignatureKind::VoluntaryExit,
                Role::VoluntaryExit,
                CollectionMode::SingleValidator,
                &validator,
                &cluster,
                signing_root,
                slot,
            )
            .await?;

        // Create signed exit message
        let signed_exit = SignedVoluntaryExit {
            message: voluntary_exit,
            signature,
        };

        Ok(signed_exit)
    }

    fn create_proposer_consensus_data_validator(
        &self,
        validator_pubkey: PublicKeyBytes,
    ) -> Box<ProposerConsensusDataValidator<E>> {
        Box::new(ProposerConsensusDataValidator::new(
            Arc::clone(&self.slashing_protection),
            self.disable_slashing_protection,
            self.spec.clone(),
            validator_pubkey,
            self.genesis_validators_root,
        ))
    }

    fn create_beacon_vote_validator(
        &self,
        slot: Slot,
        validator_attestation_committees: HashMap<PublicKeyBytes, u64>,
    ) -> Box<BeaconVoteValidator<E>> {
        let slashing_protection =
            (!self.disable_slashing_protection).then(|| Arc::clone(&self.slashing_protection));

        Box::new(BeaconVoteValidator::new(
            slot,
            slashing_protection,
            self.spec.clone(),
            validator_attestation_committees,
            self.genesis_validators_root,
            self.strict_mfp,
        ))
    }

    fn get_attesting_validators_in_committee(
        &self,
        metadata: &VotingContext,
        committee_id: CommitteeId,
    ) -> HashMap<PublicKeyBytes, u64> {
        let committee_validators = self
            .database
            .state()
            .metadata()
            .get_all_by(&committee_id)
            .map(|v| v.public_key)
            .collect::<HashSet<_>>();

        metadata
            .voting_assignments
            .attesting_committees
            .iter()
            .filter_map(|(&pubkey, &index)| {
                committee_validators
                    .contains(&pubkey)
                    .then_some((pubkey, index))
            })
            .collect::<HashMap<_, _>>()
    }

    /// Sign a single aggregate and proof (pre-Boole per-validator path).
    async fn sign_single_aggregate_and_proof(
        self: &Arc<Self>,
        aggregate: AggregateToSign<E>,
    ) -> Result<SignedAggregateAndProof<E>, Error> {
        let future = async {
            let signing_epoch = aggregate.aggregate.data().target.epoch;
            let (validator, cluster) = self.get_validator_and_cluster(aggregate.pubkey)?;

            let version = match &aggregate.aggregate {
                Attestation::Base(_) => ForkName::Base.into(),
                Attestation::Electra(_) => ForkName::Electra.into(),
            };

            let message = AggregateAndProof::from_attestation(
                aggregate.aggregator_index,
                aggregate.aggregate,
                aggregate.selection_proof,
            );

            let timer = metrics::start_timer_vec(
                &metrics::CONSENSUS_TIMES,
                &[metrics::AGGREGATE_AND_PROOF],
            );
            let timeout_mode = TimeoutMode::SlotTime {
                instance_start_time: self.get_instant_in_slot(
                    message.aggregate().data().slot,
                    self.spec.get_slot_duration() * 2 / 3,
                )?,
            };

            let completed = self
                .consensus
                .decide_instance(
                    ProposerInstanceId {
                        validator: aggregate.pubkey,
                        duty: ValidatorDutyKind::Aggregator,
                        instance_height: message.aggregate().data().slot.as_usize().into(),
                    },
                    ProposerConsensusData {
                        duty: ValidatorDuty {
                            r#type: BEACON_ROLE_AGGREGATOR,
                            pub_key: aggregate.pubkey,
                            slot: message.aggregate().data().slot,
                            validator_index: validator.index.ok_or(SpecificError::MissingIndex)?,
                            committee_index: message.aggregate().data().index,
                            // TODO: it seems the below are not needed (anymore?)
                            // potentially related: https://github.com/sigp/anchor/issues/263
                            committee_length: 0,
                            committees_at_slot: 0,
                            validator_committee_index: 0,
                            validator_sync_committee_indices: Default::default(),
                        },
                        version,
                        data_ssz: try_to_variable_list(message.as_ssz_bytes(), |provided, max| {
                            Error::SpecificError(SpecificError::DataTooLarge(format!(
                                "Attestation data too large for consensus: {} > {}",
                                provided, max
                            )))
                        })?,
                    },
                    self.create_proposer_consensus_data_validator(aggregate.pubkey),
                    timeout_mode,
                    &cluster.cluster_members,
                )
                .await
                .map_err(SpecificError::from)?;
            drop(timer);

            let data = match completed {
                Completed::TimedOut => {
                    return Err(Error::SpecificError(SpecificError::Timeout));
                }
                Completed::Success(data) => data,
            };

            let message = if ForkName::from(data.version) < ForkName::Electra {
                AggregateAndProof::Base(
                    AggregateAndProofBase::from_ssz_bytes(&data.data_ssz)
                        .map_err(|e| Error::SpecificError(SpecificError::InvalidQbftData(e)))?,
                )
            } else {
                AggregateAndProof::Electra(
                    AggregateAndProofElectra::from_ssz_bytes(&data.data_ssz)
                        .map_err(|e| Error::SpecificError(SpecificError::InvalidQbftData(e)))?,
                )
            };

            debug!(
                aggregator_index = ?message.aggregator_index(),
                data = ?message.aggregate().data(),
                num_set_aggregation_bits = message.aggregate().num_set_aggregation_bits(),
                "Decided on AggregateAndProof to sign"
            );

            let domain_hash = self.get_domain(signing_epoch, Domain::AggregateAndProof);
            let signing_root = message.signing_root(domain_hash);
            let signature = self
                .collect_signature(
                    PartialSignatureKind::PostConsensus,
                    Role::Aggregator,
                    CollectionMode::SingleValidator,
                    &validator,
                    &cluster,
                    signing_root,
                    message.aggregate().get_slot(),
                )
                .await?;

            Ok(SignedAggregateAndProof::from_aggregate_and_proof(
                message, signature,
            ))
        };
        run_and_update_metrics(
            AGGREGATE_LOG_NAME,
            &validator_metrics::SIGNED_AGGREGATES_TOTAL,
            future,
        )
        .await
    }

    /// Sign a single sync committee contribution (pre-Boole per-validator path).
    async fn sign_single_sync_committee_contribution(
        self: &Arc<Self>,
        contribution: ContributionToSign<E>,
    ) -> Result<SignedContributionAndProof<E>, Error> {
        let future = async {
            let slot = contribution.contribution.slot;
            let epoch = slot.epoch(E::slots_per_epoch());
            let aggregator_index = contribution.aggregator_index;
            let aggregator_pubkey = contribution.aggregator_pubkey;
            let subcommittee_index = contribution.contribution.subcommittee_index;
            let (validator, cluster) = self.get_validator_and_cluster(aggregator_pubkey)?;

            let signing_data = ContributionAndProofSigningData {
                contribution: contribution.contribution,
                selection_proof: contribution.selection_proof,
            };

            // Get aggregator voting assignments from Phase 3 (published at 2/3 slot)
            let aggregator_info = self.get_aggregation_assignments(slot).await?;

            let signing_data = match aggregator_info
                .multi_sync_aggregators
                .get(&aggregator_pubkey)
            {
                None => vec![signing_data],
                Some(contribution_waiter) => {
                    let mut data = contribution_waiter.submit_and_wait(signing_data).await;
                    data.sort_by(|a, b| {
                        a.contribution
                            .subcommittee_index
                            .cmp(&b.contribution.subcommittee_index)
                    });
                    data
                }
            };

            let data = Contributions::new(
                signing_data
                    .iter()
                    .map(|signing_data| {
                        // Wrap contribution to match Go-SSV's encoding
                        ContributionWrapper::from(Contribution {
                            selection_proof_sig: signing_data.selection_proof.clone().into(),
                            contribution: signing_data.contribution.clone(),
                        })
                    })
                    .collect(),
            )
            .map_err(|_| SpecificError::TooManySyncSubnetsToSign)?;

            let timer = metrics::start_timer_vec(
                &metrics::CONSENSUS_TIMES,
                &[metrics::SYNC_CONTRIBUTION_AND_PROOF],
            );
            let timeout_mode = TimeoutMode::SlotTime {
                instance_start_time: self
                    .get_instant_in_slot(slot, self.spec.get_slot_duration() * 2 / 3)?,
            };

            let completed = self
                .consensus
                .decide_instance(
                    ProposerInstanceId {
                        validator: aggregator_pubkey,
                        duty: ValidatorDutyKind::SyncCommitteeAggregator,
                        instance_height: slot.as_usize().into(),
                    },
                    ProposerConsensusData {
                        duty: ValidatorDuty {
                            r#type: BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION,
                            pub_key: aggregator_pubkey,
                            slot,
                            validator_index: validator.index.ok_or(SpecificError::MissingIndex)?,
                            committee_index: 0,
                            committee_length: 0,
                            committees_at_slot: 0,
                            validator_committee_index: aggregator_index,
                            validator_sync_committee_indices: Default::default(),
                        },
                        version: ForkName::Altair.into(),
                        data_ssz: try_to_variable_list(data.as_ssz_bytes(), |provided, max| {
                            Error::SpecificError(SpecificError::DataTooLarge(format!(
                                "Sync committee data too large for consensus: {} > {}",
                                provided, max
                            )))
                        })?,
                    },
                    self.create_proposer_consensus_data_validator(aggregator_pubkey),
                    timeout_mode,
                    &cluster.cluster_members,
                )
                .await;
            drop(timer);

            let data = match completed {
                Ok(Completed::Success(data)) => data,
                Ok(Completed::TimedOut) => return Err(SpecificError::Timeout.into()),
                Err(err) => return Err(SpecificError::QbftError(err).into()),
            };

            let data = Contributions::<E>::from_ssz_bytes(&data.data_ssz)
                .map_err(|e| Error::from(SpecificError::InvalidQbftData(e)))?;

            let domain_hash = self.get_domain(epoch, Domain::ContributionAndProof);
            let PreparedSyncContributionBatch {
                callback_message: message,
                callback_signing_root: signing_root,
                descriptor,
            } = prepare_decided_sync_contributions(
                data,
                SyncSubnetId::new(subcommittee_index),
                aggregator_index,
                domain_hash,
            )?;

            debug!(
                slot = %message.contribution.slot,
                block_root = ?message.contribution.beacon_block_root,
                subcommittee_index = message.contribution.subcommittee_index,
                num_set_aggregation_bits = message.contribution.aggregation_bits.num_set_bits(),
                "Decided on Contribution to sign"
            );

            let collection_mode =
                sync_committee_collection_mode(SyncSubnetId::new(subcommittee_index), descriptor);
            self.collect_signature(
                PartialSignatureKind::PostConsensus,
                Role::SyncCommittee,
                collection_mode,
                &validator,
                &cluster,
                signing_root,
                slot,
            )
            .await
            .map(|signature| SignedContributionAndProof { message, signature })
        };
        run_and_update_metrics(
            SYNC_COMMITTEE_CONTRIBUTION_LOG_NAME,
            &validator_metrics::SIGNED_SYNC_COMMITTEE_CONTRIBUTIONS_TOTAL,
            future,
        )
        .await
    }

    /// Whether Lighthouse's callbacks own publication of aggregator-committee duties at `epoch`.
    ///
    /// Pre-Boole they do; from Boole the metadata service's publisher owns both classes
    /// (aggregates and sync contributions) and the callbacks return empty batches. This is the
    /// exact complement of the publisher's registration gate (consensus data is only built for
    /// Boole+ slots), so exactly one path publishes for any duty. Both callbacks derive `epoch`
    /// from the duty's own payload (the aggregate's target epoch, the contribution's slot),
    /// which equal the duty slot's epoch by construction.
    pub(crate) fn lighthouse_owns_publication(&self, epoch: Epoch) -> bool {
        self.fork_schedule.active_fork(epoch) < Fork::Boole
    }

    /// Sign sync committee messages for all validators in a single SSV committee.
    ///
    /// Runs QBFT consensus once for the committee, then collects signatures for each validator.
    async fn sign_committee_sync_committee_signatures(
        &self,
        committee_id: CommitteeId,
        cluster: Cluster,
        messages: Vec<(ValidatorMetadata, SyncMessageToSign)>,
    ) -> Result<Vec<SyncCommitteeMessage>, Error> {
        let Some((_, first)) = messages.first() else {
            warn!("sign_committee_sync_committee_signatures called with empty messages");
            return Ok(vec![]);
        };
        let slot = first.slot;
        let epoch = slot.epoch(E::slots_per_epoch());

        let voting_context = self.get_voting_context(slot).await?;
        let validator_attestation_committees =
            self.get_attesting_validators_in_committee(&voting_context, committee_id);

        // Run QBFT consensus once for the entire committee
        let timer = metrics::start_timer_vec(&metrics::CONSENSUS_TIMES, &[metrics::BEACON_VOTE]);
        let timeout_mode = TimeoutMode::SlotTime {
            instance_start_time: self
                .get_instant_in_slot(slot, self.spec.get_slot_duration() / 3)?,
        };

        let completed = self
            .consensus
            .decide_instance(
                CommitteeInstanceId {
                    committee: committee_id,
                    instance_height: slot.as_usize().into(),
                },
                voting_context.beacon_vote.clone(),
                self.create_beacon_vote_validator(slot, validator_attestation_committees),
                timeout_mode,
                &cluster.cluster_members,
            )
            .await
            .map_err(SpecificError::from)?;
        drop(timer);

        let data = match completed {
            Completed::TimedOut => return Err(Error::SpecificError(SpecificError::Timeout)),
            Completed::Success(data) => data,
        };

        // Prepare all validators (metadata already resolved by `group_by_committee`)
        let domain = self.get_domain(epoch, Domain::SyncCommittee);
        let signing_root = data.block_root.signing_root(domain);

        let prepared: Vec<SigningRequest<u64>> = messages
            .into_iter()
            .map(|(validator, msg)| SigningRequest {
                validator,
                signing_root,
                duty_data: msg.validator_index,
            })
            .collect();

        // Collect signatures and assemble results
        let committee_validator_indices = self.get_committee_validator_indices(&committee_id);
        let validator_partial_signature_batch_size = voting_context
            .voting_assignments
            .voting_message_count_for_committee(|idx| committee_validator_indices.contains(idx));

        let signatures = self
            .collect_prepared_signatures(
                Role::Committee,
                slot,
                &cluster,
                validator_partial_signature_batch_size,
                data.hash(),
                &prepared,
            )
            .await?;

        let mut results = Vec::with_capacity(prepared.len());
        for req in prepared {
            let (validator_index, signature) = match req.resolve(&signatures) {
                Ok(resolved) => resolved,
                Err(pubkey) => {
                    warn!(
                        ?pubkey,
                        "Missing validator index or signature, skipping sync committee message"
                    );
                    validator_metrics::inc_counter_vec(
                        &validator_metrics::SIGNED_SYNC_COMMITTEE_MESSAGES_TOTAL,
                        &[metrics::OTHER_ERROR],
                    );
                    continue;
                }
            };

            validator_metrics::inc_counter_vec(
                &validator_metrics::SIGNED_SYNC_COMMITTEE_MESSAGES_TOTAL,
                &[validator_metrics::SUCCESS],
            );
            results.push(SyncCommitteeMessage {
                slot,
                beacon_block_root: data.block_root,
                validator_index,
                signature,
            });
        }

        Ok(results)
    }

    /// Sign attestations for all validators in a single SSV committee.
    ///
    /// Runs QBFT consensus once for the committee, then collects signatures for each validator.
    async fn sign_committee_attestations(
        &self,
        committee_id: CommitteeId,
        cluster: Cluster,
        attestations: Vec<(ValidatorMetadata, AttestationToSign<E>)>,
    ) -> Result<Vec<(u64, Attestation<E>, PublicKeyBytes)>, Error> {
        // Early return and log error for empty attestations
        let Some((_, first_attestation)) = attestations.first() else {
            warn!("sign_committee_attestations called with empty attestations");
            return Ok(vec![]);
        };
        let slot = first_attestation.attestation.data().slot;
        let first_att_data = first_attestation.attestation.data();

        let voting_context_tx = self.get_voting_context(slot).await?;
        let validator_attestation_committees =
            self.get_attesting_validators_in_committee(&voting_context_tx, committee_id);

        // Run QBFT consensus once for the entire committee
        let timer = metrics::start_timer_vec(&metrics::CONSENSUS_TIMES, &[metrics::BEACON_VOTE]);
        let timeout_mode = TimeoutMode::SlotTime {
            instance_start_time: self
                .get_instant_in_slot(slot, self.spec.get_slot_duration() / 3)?,
        };

        let completed = self
            .consensus
            .decide_instance(
                CommitteeInstanceId {
                    committee: committee_id,
                    instance_height: slot.as_usize().into(),
                },
                BeaconVote {
                    block_root: first_att_data.beacon_block_root,
                    source: first_att_data.source,
                    target: first_att_data.target,
                },
                self.create_beacon_vote_validator(slot, validator_attestation_committees),
                timeout_mode,
                &cluster.cluster_members,
            )
            .await
            .map_err(SpecificError::from)?;
        drop(timer);

        let data = match completed {
            Completed::TimedOut => return Err(Error::SpecificError(SpecificError::Timeout)),
            Completed::Success(data) => data,
        };

        // Shared values for all validators in this committee
        let domain_hash = self.get_domain(data.target.epoch, Domain::BeaconAttester);

        // Prepare all validators and apply consensus results upfront
        // (metadata already resolved by `group_by_committee`)
        let prepared: Vec<SigningRequest<AttestationToSign<E>>> = attestations
            .into_iter()
            .map(|(validator, mut att)| {
                // Apply consensus result to this attestation
                att.attestation.data_mut().beacon_block_root = data.block_root;
                att.attestation.data_mut().source = data.source;
                att.attestation.data_mut().target = data.target;

                let signing_root = att.attestation.data().signing_root(domain_hash);
                SigningRequest {
                    validator,
                    signing_root,
                    duty_data: att,
                }
            })
            .collect();

        // Collect signatures and assemble results
        let committee_validator_indices = self.get_committee_validator_indices(&committee_id);
        let validator_partial_signature_batch_size = voting_context_tx
            .voting_assignments
            .voting_message_count_for_committee(|idx| committee_validator_indices.contains(idx));

        let signatures = self
            .collect_prepared_signatures(
                Role::Committee,
                slot,
                &cluster,
                validator_partial_signature_batch_size,
                data.hash(),
                &prepared,
            )
            .await?;

        let mut results = Vec::with_capacity(prepared.len());
        for req in prepared {
            let (att, signature) = match req.resolve(&signatures) {
                Ok(resolved) => resolved,
                Err(pubkey) => {
                    warn!(
                        ?pubkey,
                        "Missing validator index or signature, skipping attestation"
                    );
                    continue;
                }
            };

            let AttestationToSign {
                validator_index,
                pubkey,
                validator_committee_index,
                mut attestation,
            } = att;

            if let Err(e) = attestation.add_signature(&signature, validator_committee_index) {
                error!(error = ?e, ?pubkey, "Failed to add signature to attestation, skipping");
                continue;
            }

            results.push((validator_index, attestation, pubkey));
        }

        Ok(results)
    }

    /// Provide slashing protection for attestations, safely updating the slashing protection DB.
    ///
    /// Returns a vec of safe attestations which have passed slashing protection. Unsafe
    /// attestations will be dropped and result in warning logs.
    fn slashing_protection_attestations(
        &self,
        attestations: Vec<(u64, Attestation<E>, PublicKeyBytes)>,
    ) -> Result<Vec<(u64, Attestation<E>)>, Error> {
        let mut safe_attestations = Vec::with_capacity(attestations.len());
        let mut attestations_to_check = Vec::with_capacity(attestations.len());

        for (_, attestation, validator_pubkey) in &attestations {
            let domain_hash =
                self.get_domain(attestation.data().target.epoch, Domain::BeaconAttester);
            attestations_to_check.push((
                attestation.data(),
                validator_pubkey,
                domain_hash,
                if self.disable_slashing_protection {
                    CheckSlashability::No
                } else {
                    CheckSlashability::Yes
                },
            ))
        }

        // Batch check the attestations against the slashing protection DB while preserving the
        // order so we can zip the results against the original vec.
        //
        // If the DB transaction fails then we consider the entire batch slashable and discard it.
        let results: Vec<Result<(), Error>> = self
            .slashing_protection
            .check_and_insert_attestations(&attestations_to_check)
            .map_err(Error::Slashable)?
            .into_iter()
            .map(convert_slashing_result)
            .collect();

        for ((validator_index, attestation, validator_pubkey), slashing_status) in
            attestations.into_iter().zip(results)
        {
            match slashing_status {
                Ok(()) => {
                    safe_attestations.push((validator_index, attestation));
                    validator_metrics::inc_counter_vec(
                        &validator_metrics::SIGNED_ATTESTATIONS_TOTAL,
                        &[validator_metrics::SUCCESS],
                    );
                }
                Err(Error::SameData) => {
                    warn!("Skipping previously signed attestation");
                    validator_metrics::inc_counter_vec(
                        &validator_metrics::SIGNED_ATTESTATIONS_TOTAL,
                        &[validator_metrics::SAME_DATA],
                    );
                }
                Err(Error::Slashable(NotSafe::UnregisteredValidator(pk))) => {
                    error!(
                        ?pk,
                        "Internal error: validator was not properly registered for slashing protection",
                    );
                    validator_metrics::inc_counter_vec(
                        &validator_metrics::SIGNED_ATTESTATIONS_TOTAL,
                        &[validator_metrics::UNREGISTERED],
                    );
                }
                Err(Error::Slashable(err)) => {
                    error!(?err, "Not signing slashable attestation");
                    validator_metrics::inc_counter_vec(
                        &validator_metrics::SIGNED_ATTESTATIONS_TOTAL,
                        &[validator_metrics::SLASHABLE],
                    );
                }
                Err(e) => {
                    error!(
                        error = ?e,
                        public_key = ?validator_pubkey,
                        "Unexpected error during slashing protection check"
                    );
                    validator_metrics::inc_counter_vec(
                        &validator_metrics::SIGNED_ATTESTATIONS_TOTAL,
                        &[metrics::OTHER_ERROR],
                    );
                }
            }
        }

        Ok(safe_attestations)
    }
}

/// # Arguments
/// - `log_name`: The name for the object being signed, used in error logging.
/// - `metric`: The metric updated according to the result.
/// - `action`: The future performing the necessary actions and returning the result to check.
async fn run_and_update_metrics<T>(
    log_name: &'static str,
    metric: &LazyLock<validator_metrics::Result<IntCounterVec>>,
    action: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    let result = action.await;
    match &result {
        Ok(_) => {
            validator_metrics::inc_counter_vec(metric, &[validator_metrics::SUCCESS]);
        }
        Err(Error::SameData) => {
            warn!("Skipping signing of previously signed {log_name}",);
            validator_metrics::inc_counter_vec(metric, &[validator_metrics::SAME_DATA]);
        }
        Err(Error::Slashable(NotSafe::UnregisteredValidator(pk))) => {
            error!(
                ?pk,
                "Internal error: validator was not properly registered for slashing protection",
            );
            validator_metrics::inc_counter_vec(metric, &[validator_metrics::UNREGISTERED]);
        }
        Err(Error::Slashable(err)) => {
            error!(?err, "Not signing slashable {log_name}",);
            validator_metrics::inc_counter_vec(metric, &[validator_metrics::SLASHABLE]);
        }
        Err(Error::SpecificError(SpecificError::Timeout)) => {
            warn!("Signing {log_name} timed out - other operators might be offline");
            validator_metrics::inc_counter_vec(metric, &[metrics::TIMEOUT]);
        }
        Err(err) => {
            error!(?err, "Unexpected error while signing {log_name}");
            validator_metrics::inc_counter_vec(metric, &[metrics::OTHER_ERROR]);
        }
    }
    result
}

fn decrypt_key_share(
    operator_key: &Rsa<Private>,
    encrypted_private_key: [u8; ENCRYPTED_KEY_LENGTH],
    pubkey_bytes: PublicKeyBytes,
) -> Result<SecretKey, ()> {
    // the buffer size must be larger than or equal the modulus size
    let mut key_hex = [0; 2048 / 8];
    let length = operator_key
        .private_decrypt(&encrypted_private_key, &mut key_hex, Padding::PKCS1)
        .map_err(|e| error!(?e, validator = %pubkey_bytes, "Share decryption failed"))?;

    let key_hex = from_utf8(&key_hex[..length]).map_err(|err| {
        error!(
            ?err,
            validator = %pubkey_bytes,
            "Share decryption yielded non-utf8 data"
        )
    })?;

    let mut secret_key = [0; 32];
    hex::decode_to_slice(
        key_hex.strip_prefix("0x").unwrap_or(key_hex),
        &mut secret_key,
    )
    .map_err(|err| {
        error!(
            ?err,
            validator = %pubkey_bytes,
            "Decrypted share is not a hex string of size 64"
        )
    })?;

    SecretKey::deserialize(&secret_key)
        .map_err(|err| error!(?err, validator = %pubkey_bytes, "Invalid secret key decrypted"))
}

struct VotingContext {
    /// Cached voting assignments (computed at slot start, reused here)
    voting_assignments: Arc<VotingAssignments>,
    /// The `BeaconVote` (only available at 1/3 slot from beacon node)
    beacon_vote: BeaconVote,
}

/// Cached validator voting assignments for a slot.
///
/// This struct caches voting assignments computed at slot start and reuses it at 1/3 slot,
/// eliminating redundant computation. It supports two different counting patterns:
///
/// 1. **Committee messages** (attestation + sync): `+1` per sync validator
/// 2. **Selection proofs** (aggregator committee): `+N` per sync validator (N = subnets)
#[derive(Debug, Clone)]
pub struct VotingAssignments {
    /// The slot this voting assignments is about.
    pub slot: Slot,
    /// The indices of validators that are attesting in this slot.
    pub attesting_validators: Vec<ValidatorIndex>,
    /// The pubkeys of attesting validators mapped to their attestation committee index.
    pub attesting_committees: HashMap<PublicKeyBytes, u64>,
    /// Sync committee validators mapped to the number of original positions in each subnet.
    /// A validator may participate in multiple subnets or occupy several positions in one subnet.
    pub sync_validators_by_subnet: HashMap<ValidatorIndex, HashMap<SyncSubnetId, usize>>,
}

impl VotingAssignments {
    /// Returns a flat list of all sync validator indices.
    ///
    /// Derives this from the keys of `sync_validators_by_subnet`.
    pub fn sync_validators(&self) -> Vec<ValidatorIndex> {
        self.sync_validators_by_subnet.keys().copied().collect()
    }

    /// Counts expected signatures for selection proof collection.
    ///
    /// For each validator in the committee:
    /// - `+1` if the validator is attesting
    /// - `+N` if the validator is in sync committee (N = number of subnets)
    ///
    /// This counting pattern is used for aggregator committee pre-consensus where
    /// each sync validator produces one selection proof per subnet they participate in.
    pub fn selection_proof_count_for_committee<F>(&self, is_in_committee: F) -> usize
    where
        F: Fn(&ValidatorIndex) -> bool,
    {
        let mut count = 0;

        // Count attesting validators: +1 each
        for validator_idx in &self.attesting_validators {
            if is_in_committee(validator_idx) {
                count += 1;
            }
        }

        // Count sync validators: +N each (N = number of subnets)
        for (validator_idx, subnets) in &self.sync_validators_by_subnet {
            if is_in_committee(validator_idx) {
                count += subnets.len();
            }
        }

        count
    }

    /// Counts expected signatures for voting message collection.
    ///
    /// For each validator in the committee:
    /// - `+1` if the validator is attesting
    /// - `+1` if the validator is in sync committee (regardless of subnet count)
    ///
    /// This counting pattern is used for post-consensus attestation and sync committee
    /// voting messages where each validator produces one message regardless of
    /// how many subnets they participate in.
    pub fn voting_message_count_for_committee<F>(&self, is_in_committee: F) -> usize
    where
        F: Fn(&ValidatorIndex) -> bool,
    {
        let mut count = 0;

        // Count attesting validators: +1 each
        for validator_idx in &self.attesting_validators {
            if is_in_committee(validator_idx) {
                count += 1;
            }
        }

        // Count sync validators: +1 each (flat, regardless of subnet count)
        for validator_idx in self.sync_validators_by_subnet.keys() {
            if is_in_committee(validator_idx) {
                count += 1;
            }
        }

        count
    }
}

/// Aggregator-specific voting assignments, cached at 2/3 slot when selection proofs are known.
///
/// This struct is separate from `VotingAssignments` because:
/// - `VotingAssignments` is cached at slot start, before selection proofs are computed
/// - `AggregationAssignments` is cached at 2/3 slot, after Lighthouse fills in selection proofs
///
/// At 2/3 slot, `DutyAndProof.selection_proof.is_some()` indicates `is_aggregator = true`.
///
/// Also tracks multi-subnet sync aggregators. When a validator aggregates for multiple
/// sync subnets, `produce_signed_contribution_and_proof` is called multiple times (once
/// per subnet). The waiter ensures all contributions are collected before starting QBFT.
pub struct AggregationAssignments<E: EthSpec> {
    /// The slot this info is for
    pub slot: Slot,

    /// `Pubkey` -> `committee_index` for aggregating validators
    pub aggregator_committees: HashMap<PublicKeyBytes, u64>,

    /// Multi-subnet sync aggregators (validators aggregating > 1 subnet)
    multi_sync_aggregators: HashMap<PublicKeyBytes, ContributionWaiter<E>>,

    /// Pre-built consensus data per SSV committee (for Boole+)
    /// Maps `CommitteeId` -> `AggregatorCommitteeConsensusData`
    consensus_data_by_ssv_committee: HashMap<CommitteeId, Arc<AggregatorCommitteeConsensusData<E>>>,
}

impl<E: EthSpec> AggregationAssignments<E> {
    /// Get the pre-built consensus data for an SSV committee.
    /// Returns None if fork < Boole or no aggregators in committee.
    pub fn get_consensus_data(
        &self,
        ssv_committee_id: &CommitteeId,
    ) -> Option<Arc<AggregatorCommitteeConsensusData<E>>> {
        self.consensus_data_by_ssv_committee
            .get(ssv_committee_id)
            .cloned()
    }
}

pub struct ContributionWaiter<E: EthSpec> {
    data: RwLock<Vec<ContributionAndProofSigningData<E>>>,
    barrier: Barrier,
}

impl<E: EthSpec> ContributionWaiter<E> {
    fn new(count: usize) -> Self {
        Self {
            data: Default::default(),
            barrier: Barrier::new(count),
        }
    }

    async fn submit_and_wait(
        &self,
        data: ContributionAndProofSigningData<E>,
    ) -> Vec<ContributionAndProofSigningData<E>> {
        self.data.write().await.push(data);
        select! {
            _ = self.barrier.wait() => {}
            _ = sleep(Duration::from_secs(1)) => {
                warn!("Contribution waiter timed out");
            }
        }
        (*self.data.read().await).clone()
    }
}

#[derive(Clone)]
pub struct ContributionAndProofSigningData<E: EthSpec> {
    contribution: SyncCommitteeContribution<E>,
    selection_proof: SyncSelectionProof,
}

struct PreparedSyncContributionBatch<E: EthSpec> {
    callback_message: ContributionAndProof<E>,
    callback_signing_root: Hash256,
    descriptor: Vec<SyncCommitteeBatchEntry>,
}

/// Prepares the exact decided sync contribution root multiset for every Lighthouse callback.
fn prepare_decided_sync_contributions<E: EthSpec>(
    contributions: Contributions<E>,
    callback_subnet: SyncSubnetId,
    aggregator_index: u64,
    domain_hash: Hash256,
) -> Result<PreparedSyncContributionBatch<E>, SpecificError> {
    let mut callback = None;
    let mut descriptor = Vec::with_capacity(contributions.len());

    for contribution in contributions.into_iter().map(Contribution::from) {
        let subnet_id = SyncSubnetId::new(contribution.contribution.subcommittee_index);
        let message = ContributionAndProof {
            aggregator_index,
            contribution: contribution.contribution,
            selection_proof: contribution.selection_proof_sig,
        };
        let signing_root = message.signing_root(domain_hash);

        if callback.is_none() && subnet_id == callback_subnet {
            callback = Some((message, signing_root));
        }
        descriptor.push(SyncCommitteeBatchEntry {
            subnet_id,
            signing_root,
            multiplicity: 1,
        });
    }

    let (callback_message, callback_signing_root) = callback.ok_or(SpecificError::NoDataAgreed)?;

    descriptor.sort_unstable_by_key(|entry| (u64::from(entry.subnet_id), entry.signing_root));

    let mut collapsed: Vec<SyncCommitteeBatchEntry> = Vec::with_capacity(descriptor.len());
    for entry in descriptor {
        match collapsed.last_mut() {
            Some(previous)
                if previous.subnet_id == entry.subnet_id
                    && previous.signing_root == entry.signing_root =>
            {
                previous.multiplicity += entry.multiplicity;
            }
            _ => collapsed.push(entry),
        }
    }

    Ok(PreparedSyncContributionBatch {
        callback_message,
        callback_signing_root,
        descriptor: collapsed,
    })
}

#[derive(Clone)]
enum CollectionMode {
    SingleValidator,
    SingleValidatorBatch {
        /// Subnet requested by the current Lighthouse callback.
        subnet_id: SyncSubnetId,
        /// Canonical descriptor for every unique subnet and signing root pair in this batch.
        descriptor: Vec<SyncCommitteeBatchEntry>,
    },
    Committee {
        /// The number of validator partial signatures this operator batches locally into the
        /// outgoing committee message for the round.
        validator_partial_signature_batch_size: usize,
        /// Identifies which validator partial signatures belong in the same outgoing committee
        /// message.
        base_hash: Hash256,
    },
}

fn sync_committee_collection_mode(
    callback_subnet: SyncSubnetId,
    descriptor: Vec<SyncCommitteeBatchEntry>,
) -> CollectionMode {
    if descriptor
        .iter()
        .map(|entry| entry.multiplicity)
        .sum::<usize>()
        == 1
    {
        CollectionMode::SingleValidator
    } else {
        CollectionMode::SingleValidatorBatch {
            subnet_id: callback_subnet,
            descriptor,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncSelectionProofAssignmentError {
    Empty,
    MissingCallbackSubnet {
        subnet_id: SyncSubnetId,
    },
    OutOfRangeSubnet {
        subnet_id: SyncSubnetId,
        subnet_count: u64,
    },
    TooManyPositions {
        count: usize,
        max: usize,
    },
}

#[derive(Debug, Clone)]
pub enum SpecificError {
    Unsupported,
    SignatureCollectionFailed(CollectionError),
    ArithError(ArithError),
    QbftError(QbftError),
    Timeout,
    InvalidQbftData(DecodeError),
    TooManySyncSubnetsToSign,
    NoDataAgreed,
    Metadata,
    MissingIndex,
    SlotClock,
    NotSynced,
    InconsistentDatabase,
    /// Database inconsistency: validator references a cluster that doesn't exist
    ValidatorClusterMismatch {
        validator_pubkey: PublicKeyBytes,
        cluster_id: ClusterId,
    },
    KeyShareDecryptionFailed,
    DataTooLarge(String),
    ClusterLiquidated,
    /// Requested slot has already passed the current cached slot in `VotingAssignments`
    MetadataSlotPassed,
    /// Watch channel for `VotingAssignments` has been closed
    MetadataChannelClosed,
    /// Requested slot has already passed the current cached slot in `AggregationAssignments`
    AggregatorInfoSlotPassed,
    /// Watch channel for `AggregationAssignments` has been closed
    AggregatorInfoChannelClosed,
    /// `produce_selection_proof` called for validator not in
    /// `VotingAssignments.attesting_committees`
    ValidatorNotAttesting {
        validator_pubkey: PublicKeyBytes,
        slot: Slot,
    },
    /// `produce_sync_selection_proof` called for validator not in
    /// `VotingAssignments.sync_validators_by_subnet`
    ValidatorNotInSyncCommittee {
        validator_pubkey: PublicKeyBytes,
        slot: Slot,
    },
    InvalidSyncSelectionProofAssignment {
        validator_pubkey: PublicKeyBytes,
        slot: Slot,
        reason: SyncSelectionProofAssignmentError,
    },
    /// This committee's aggregate not found in consensus data (Boole+)
    AggregateNotInConsensus(u64),
    /// This subcommittee's contribution not found in consensus data (Boole+)
    ContributionNotInConsensus(u64),
    /// This validator not found in consensus data (Boole+)
    ValidatorNotInConsensus(ValidatorIndex),
    /// The detached `AggregatorCommittee` post-consensus execution died before producing a
    /// result (executor shutdown, spawn refusal, or task panic)
    PostConsensusAborted,
}

impl From<CollectionError> for SpecificError {
    fn from(err: CollectionError) -> SpecificError {
        SpecificError::SignatureCollectionFailed(err)
    }
}

impl From<ArithError> for SpecificError {
    fn from(err: ArithError) -> SpecificError {
        SpecificError::ArithError(err)
    }
}

impl From<QbftError> for SpecificError {
    fn from(err: QbftError) -> SpecificError {
        SpecificError::QbftError(err)
    }
}

fn convert_slashing_result(value: Result<Safe, NotSafe>) -> Result<(), Error> {
    match value {
        Ok(Safe::Valid) => Ok(()),
        Ok(Safe::SameData) => Err(Error::SameData),
        Err(not_safe) => Err(Error::Slashable(not_safe)),
    }
}

pub type Error = ValidatorStoreError<SpecificError>;

impl<T: SlotClock, E: EthSpec, C: ConsensusDecider<E> + 'static> ValidatorStore
    for AnchorValidatorStore<T, E, C>
{
    type Error = SpecificError;
    type E = E;

    fn validator_index(&self, pubkey: &PublicKeyBytes) -> Option<u64> {
        self.database
            .state()
            .metadata()
            .get_by(pubkey)
            .and_then(|v| v.index.map(|idx| *idx as u64))
    }

    fn voting_pubkeys<I, F>(&self, filter_func: F) -> I
    where
        I: FromIterator<PublicKeyBytes>,
        F: Fn(DoppelgangerStatus) -> Option<PublicKeyBytes>,
    {
        let state = self.database.state();

        // Treat all shares as `SigningEnabled`
        state
            .shares()
            .values()
            .filter_map(|v| filter_func(DoppelgangerStatus::SigningEnabled(v.validator_pubkey)))
            .filter(|public_key| {
                state
                    .clusters()
                    .get_by(public_key)
                    .is_some_and(|cluster| !cluster.liquidated)
            })
            .collect()
    }

    fn doppelganger_protection_allows_signing(&self, _validator_pubkey: PublicKeyBytes) -> bool {
        // we don't care about doppelgangers
        true
    }

    fn num_voting_validators(&self) -> usize {
        self.database.state().shares().length()
    }

    fn graffiti(&self, validator_pubkey: &PublicKeyBytes) -> Option<Graffiti> {
        self.database
            .state()
            .metadata()
            .get_by(validator_pubkey)
            .map(|metadata| metadata.graffiti)
    }

    fn get_fee_recipient(&self, validator_pubkey: &PublicKeyBytes) -> Option<Address> {
        let state = self.database.state();
        state.metadata().get_by(validator_pubkey).and_then(|v| {
            state
                .clusters()
                .get_by(&v.cluster_id)
                .map(|cluster| cluster.fee_recipient)
        })
    }

    fn determine_builder_boost_factor(&self, _validator_pubkey: &PublicKeyBytes) -> Option<u64> {
        if self.prefer_builder_proposals {
            return Some(u64::MAX);
        }

        self.builder_boost_factor
    }

    /// Runs RANDAO pre-consensus and reconstructs the reveal for this proposer duty.
    ///
    /// Also holds until `--proposer-delay-ms` into the slot before returning, so it can block for
    /// as long as that setting allows. The reveal is a required parameter of the block request,
    /// so Lighthouse cannot ask earlier and this is the last point Anchor owns before it does.
    /// See `await_proposer_delay`.
    async fn randao_reveal(
        &self,
        validator_pubkey: PublicKeyBytes,
        signing_epoch: Epoch,
    ) -> Result<Signature, Error> {
        let span = info_span!(
            "proposer_randao_reveal",
            cluster_size = field::Empty,
            // Includes any proposer delay, and wraps like every other `slot_elapsed_ms` here.
            // `randao_completed_ms` is pre-consensus alone, and does not wrap.
            slot_elapsed_ms = field::Empty,
            clock_slot = field::Empty,
            randao_completed_ms = field::Empty,
            proposer_delay_outcome = field::Empty,
            proposer_delay_waited_ms = field::Empty,
            signing_epoch = signing_epoch.as_u64(),
            failure_reason = field::Empty,
            outcome = field::Empty,
            validator_pubkey = %validator_pubkey,
            validator_index = field::Empty,
        );
        let future = async {
            trace!(
                checkpoint = instrumentation::checkpoints::RANDAO_REVEAL_ENTERED,
                "Proposer randao reveal entered"
            );
            let result = async {
                let clock_slot = self.slot_clock.now().ok_or(SpecificError::SlotClock)?;
                Span::current().record("clock_slot", clock_slot.as_u64());

                let domain_hash = self.get_domain(signing_epoch, Domain::Randao);
                let signing_root = signing_epoch.signing_root(domain_hash);

                let (validator, cluster) = self.get_validator_and_cluster(validator_pubkey)?;

                if let Some(validator_idx) = validator.index {
                    Span::current().record("validator_index", *validator_idx);
                }

                let cluster_size = cluster.cluster_members.len();
                Span::current().record("cluster_size", cluster_size);

                let signature = self
                    .collect_signature(
                        PartialSignatureKind::RandaoPartialSig,
                        Role::Proposer,
                        CollectionMode::SingleValidator,
                        &validator,
                        &cluster,
                        signing_root,
                        clock_slot,
                    )
                    .await?;

                // Taken before any wait, so it is not polluted by our own delay. This is what tells
                // an operator whether a configured delay can ever bite.
                let randao_completed = elapsed_in_slot(&self.slot_clock, clock_slot);
                Span::current().record(
                    "randao_completed_ms",
                    randao_completed.map(|elapsed| elapsed.as_millis() as u64),
                );
                if let Some(elapsed) = randao_completed {
                    metrics::observe_duration(&metrics::RANDAO_REVEAL_COMPLETION_OFFSET, elapsed);
                }
                trace!(
                    checkpoint = instrumentation::checkpoints::RANDAO_REVEAL_RECONSTRUCTED,
                    randao_completed_ms = randao_completed.map(|e| e.as_millis() as u64),
                    "Proposer randao reveal reconstructed"
                );

                await_proposer_delay(self.proposer_delay, randao_completed).await;

                Ok(signature)
            }
            .await;

            Span::current().record(
                "slot_elapsed_ms",
                determine_slot_elapsed_ms(&self.slot_clock),
            );
            let outcome = instrumentation::outcome_from_result(&result);
            Span::current().record("outcome", outcome);
            match &result {
                Ok(_) => trace!(
                    checkpoint = instrumentation::checkpoints::RANDAO_REVEAL_COMPLETED,
                    outcome = &outcome
                ),
                Err(err) => {
                    let failure_reason = instrumentation::failure_reason(err);
                    warn!(
                        checkpoint = instrumentation::checkpoints::RANDAO_REVEAL_FAILED,
                        outcome = &outcome,
                        failure_reason = &failure_reason
                    );
                    Span::current().record("failure_reason", failure_reason);
                }
            }

            result
        }
        .instrument(span);

        run_and_update_metrics(
            RANDAO_REVEAL_LOG_NAME,
            &metrics::SIGNED_RANDAO_REVEALS_TOTAL,
            future,
        )
        .await
    }

    fn set_validator_index(&self, validator_pubkey: &PublicKeyBytes, index: u64) {
        let Some(maybe_old_idx) = self
            .database
            .state()
            .metadata()
            .get_by(validator_pubkey)
            .map(|v| v.index)
        else {
            warn!(
                validator = validator_pubkey.as_hex_string(),
                "Trying to set index for unknown validator"
            );
            return;
        };

        let index = ValidatorIndex(index as usize);
        if let Some(old_idx) = maybe_old_idx {
            if old_idx != index {
                error!(
                    ?validator_pubkey,
                    db=?old_idx,
                    got=?index,
                    "Inconsistent validator index - database corrupt?"
                );
            }
        } else {
            let result = self
                .database
                .set_validator_indices(HashMap::from([(*validator_pubkey, index)]));
            if let Err(err) = result {
                error!(?err, "Failed to set validator index");
            }
        }
    }

    async fn sign_block(
        &self,
        validator_pubkey: PublicKeyBytes,
        block: UnsignedBlock<E>,
        current_slot: Slot,
    ) -> Result<SignedBlock<E>, Error> {
        let (block_type, block_slot) = match block {
            UnsignedBlock::Full(FullBlockContents::BlockContents(ref contents)) => {
                ("full", contents.block.slot())
            }
            UnsignedBlock::Full(FullBlockContents::Block(ref block)) => ("full", block.slot()),
            UnsignedBlock::Blinded(ref block) => ("blinded", block.slot()),
        };

        let span = info_span!(
            "proposer_sign_block",
            block_type,
            cluster_size = field::Empty,
            clock_slot = current_slot.as_u64(),
            failure_reason = field::Empty,
            proposal_matched = field::Empty,
            outcome = field::Empty,
            slot_elapsed_ms = field::Empty,
            block_slot = block_slot.as_u64(),
            validator_pubkey = %validator_pubkey,
            validator_index = field::Empty,
        );

        let future = async {
            trace!(
                checkpoint = instrumentation::checkpoints::DUTY_ENTRY,
                slot_elapsed_ms = determine_slot_elapsed_ms(&self.slot_clock),
                "Proposer block signing duty entered"
            );

            let result = async {
                if !*self.is_synced.borrow() {
                    return Err(Error::SpecificError(SpecificError::NotSynced));
                }
                let (validator, cluster) = self.get_validator_and_cluster(validator_pubkey)?;

                let cluster_size = cluster.cluster_members.len();
                Span::current().record("cluster_size", cluster_size);

                if let Some(validator_idx) = validator.index {
                    Span::current().record("validator_index", *validator_idx);
                }

                let (blinded_block, local_full_block) = match block {
                    UnsignedBlock::Full(FullBlockContents::BlockContents(contents)) => (
                        contents.block.to_ref().into(),
                        Some((contents.block, Some((contents.kzg_proofs, contents.blobs)))),
                    ),
                    UnsignedBlock::Full(FullBlockContents::Block(block)) => {
                        (block.to_ref().into(), Some((block, None)))
                    }
                    UnsignedBlock::Blinded(block) => (block, None),
                };

                trace!(
                    checkpoint = instrumentation::checkpoints::PRE_CONSENSUS_HANDOFF,
                    slot_elapsed_ms = determine_slot_elapsed_ms(&self.slot_clock),
                    "Handing block to consensus process"
                );

                let decided_block = self
                    .decide_abstract_block(&validator, &cluster, &blinded_block)
                    .await?;

                trace!(
                    checkpoint = instrumentation::checkpoints::CONSENSUS_DECIDED,
                    "Block consensus completed successfully"
                );

                // Sign the decided block
                let signed_block = match decided_block {
                    UnsignedBlock::Blinded(block) => {
                        self.sign_abstract_block(&validator, &cluster, block, current_slot)
                            .await
                    }
                    UnsignedBlock::Full(block) => {
                        self.sign_abstract_block(
                            &validator,
                            &cluster,
                            BeaconBlock::from(block),
                            current_slot,
                        )
                        .await
                    }
                }?;

                trace!(
                    checkpoint = instrumentation::checkpoints::BLOCK_SIGNED,
                    "Block threshold signature completed"
                );

                let publish_decision =
                    select_publish_block(signed_block, &blinded_block, local_full_block);
                Span::current().record("proposal_matched", publish_decision.proposal_matched);
                trace!(
                    checkpoint = instrumentation::checkpoints::PUBLISH_BLOCK,
                    publish_path = publish_decision.publish_path,
                    "Publish path selected"
                );

                Ok(publish_decision)
            }
            .await;

            Span::current().record(
                "slot_elapsed_ms",
                determine_slot_elapsed_ms(&self.slot_clock),
            );
            let outcome = instrumentation::outcome_from_result(&result);
            Span::current().record("outcome", outcome);
            match &result {
                Ok(_) => trace!(
                    checkpoint = instrumentation::checkpoints::DUTY_COMPLETED,
                    outcome = &outcome
                ),
                Err(err) => {
                    let failure_reason = instrumentation::failure_reason(err);
                    warn!(
                        checkpoint = instrumentation::checkpoints::DUTY_FAILED,
                        outcome = &outcome,
                        failure_reason = &failure_reason
                    );
                    Span::current().record("failure_reason", failure_reason);
                }
            }
            result
        }
        .instrument(span);

        let publish_decision = run_and_update_metrics(
            BLOCK_LOG_NAME,
            &validator_metrics::SIGNED_BLOCKS_TOTAL,
            future,
        )
        .await?;

        // Lighthouse's `ValidatorStore` API has no explicit "signed, do not publish" outcome.
        // Its block service treats `UnknownPubkey` as a benign cancellation and returns without
        // publishing, whereas every other error is surfaced as a block-production failure. Keep
        // this integration signal outside the signing instrumentation so the completed threshold
        // signature and the selected non-leader path are recorded as successful.
        publish_decision
            .signed_block
            .ok_or(Error::UnknownPubkey(validator_pubkey))
    }

    async fn sign_validator_registration_data(
        &self,
        validator_registration_data: ValidatorRegistrationData,
    ) -> Result<SignedValidatorRegistrationData, Error> {
        let future = async {
            let domain_hash = self.spec.get_builder_application_domain();

            let (validator, cluster) =
                self.get_validator_and_cluster(validator_registration_data.pubkey)?;

            // Go-SSV always uses the start of the current epoch for the timestamp in
            // `ValidatorRegistrationData`, so we need to convert to that. However, it uses the duty
            // slot (which is passed in) for the signature message, so we need to pass that to
            // `collect_signature`.
            let duty_slot = self
                .slot_clock
                .slot_of(Duration::from_secs(validator_registration_data.timestamp))
                .ok_or(SpecificError::SlotClock)?;
            let epoch_start_slot = duty_slot
                .epoch(E::slots_per_epoch())
                .start_slot(E::slots_per_epoch());
            let duration = self
                .slot_clock
                .start_of(epoch_start_slot)
                .ok_or(SpecificError::SlotClock)?;
            let validator_registration_data = ValidatorRegistrationData {
                timestamp: duration.as_secs(),
                ..validator_registration_data
            };

            let signing_root = validator_registration_data.signing_root(domain_hash);

            let signature = self
                .collect_signature(
                    PartialSignatureKind::ValidatorRegistration,
                    Role::ValidatorRegistration,
                    CollectionMode::SingleValidator,
                    &validator,
                    &cluster,
                    signing_root,
                    duty_slot,
                )
                .await?;

            Ok(SignedValidatorRegistrationData {
                message: validator_registration_data,
                signature,
            })
        };

        run_and_update_metrics(
            VALIDATOR_REGISTRATION_LOG_NAME,
            &validator_metrics::SIGNED_VALIDATOR_REGISTRATIONS_TOTAL,
            future,
        )
        .await
    }

    fn sign_aggregate_and_proofs(
        self: &Arc<Self>,
        aggregates: Vec<AggregateToSign<E>>,
    ) -> impl Stream<Item = Result<Vec<SignedAggregateAndProof<E>>, Error>> + Send {
        let this = Arc::clone(self);
        stream::once(async move {
            // Empty input: nothing to sign and no fork to determine.
            let Some(first) = aggregates.first() else {
                return Ok(Vec::new());
            };

            if !this.lighthouse_owns_publication(first.aggregate.data().target.epoch) {
                // Boole+: hand Lighthouse an empty batch, which its publish loop drops silently.
                // Publication ownership lives with the metadata service's publisher, which signs
                // and publishes the decided worklist regardless of whether Lighthouse's duty
                // snapshot saw the selection proofs in time. See
                // [`crate::aggregator_post_consensus`].
                //
                // Lighthouse's request set is its snapshot's view of who aggregates; the
                // publisher only ever sees the decided view. Logging the request set here keeps
                // divergent operator views diagnosable by comparing the two.
                debug!(
                    slot = %first.aggregate.data().slot,
                    requested = aggregates.len(),
                    aggregators = ?aggregates
                        .iter()
                        .map(|agg| agg.aggregator_index)
                        .collect::<Vec<_>>(),
                    "Deferring Lighthouse-requested aggregates to the decided-value publisher"
                );
                return Ok(Vec::new());
            }

            // Pre-Boole: per-validator processing, Lighthouse publishes the results
            let futures = aggregates.into_iter().map(|agg| {
                let this = Arc::clone(&this);
                async move { this.sign_single_aggregate_and_proof(agg).await }
            });
            let results = join_all(futures).await;
            Ok(results.into_iter().filter_map(|r| r.ok()).collect())
        })
    }

    async fn produce_selection_proof(
        &self,
        validator_pubkey: PublicKeyBytes,
        slot: Slot,
    ) -> Result<SelectionProof, Error> {
        let future = async {
            let epoch = slot.epoch(E::slots_per_epoch());
            let domain_hash = self.get_domain(epoch, Domain::SelectionProof);
            let signing_root = slot.signing_root(domain_hash);
            let (validator, cluster) = self.get_validator_and_cluster(validator_pubkey)?;

            // Stop at two thirds of the slot. If the selection proof is not ready by then, we
            // will not produce an aggregation anyway.
            let delay = self.spec.get_slot_duration() * 2 / 3;

            let signature = if self.fork_schedule.active_fork(epoch) >= Fork::Boole {
                let committee_id = cluster.committee_id();
                let voting_assignments = self.get_voting_assignments(slot).await?;

                // Defensive check: the validator should be present in `VotingAssignments` because
                // both this call and `VotingAssignments` come from `DutiesService`. If not, we
                // have an inconsistency, for example a stale cache after a poll timeout, and
                // should not participate with the wrong batch size.
                if !voting_assignments
                    .attesting_committees
                    .contains_key(&validator_pubkey)
                {
                    return Err(SpecificError::ValidatorNotAttesting {
                        validator_pubkey,
                        slot,
                    }
                    .into());
                }

                // Build a set of validator indices in this committee.
                // This handles divergent operator views, since we only count validators we have
                // shares for.
                let committee_validator_indices =
                    self.get_committee_validator_indices(&committee_id);

                // Count how many selection-proof partial signatures belong in this batch.
                let validator_partial_signature_batch_size = voting_assignments
                    .selection_proof_count_for_committee(|idx| {
                        committee_validator_indices.contains(idx)
                    });

                // Compute a deterministic batch ID. Every operator in the committee must derive
                // the same `base_hash`.
                let batch_id = SelectionProofBatchId::new(slot, committee_id);
                let base_hash = batch_id.hash();

                trace!(
                    %slot,
                    validator_index = ?validator.index,
                    "Producing committee selection proof"
                );

                let collection_mode = CollectionMode::Committee {
                    validator_partial_signature_batch_size,
                    base_hash,
                };

                self.timeout_within_slot(
                    slot,
                    delay,
                    self.collect_signature(
                        PartialSignatureKind::AggregatorCommitteePartialSig,
                        Role::AggregatorCommittee,
                        collection_mode,
                        &validator,
                        &cluster,
                        signing_root,
                        slot,
                    ),
                )
                .await?
            } else {
                // Use the original single-validator path.
                self.timeout_within_slot(
                    slot,
                    delay,
                    self.collect_signature(
                        PartialSignatureKind::SelectionProofPartialSig,
                        Role::Aggregator,
                        CollectionMode::SingleValidator,
                        &validator,
                        &cluster,
                        signing_root,
                        slot,
                    ),
                )
                .await?
            };

            Ok(signature.into())
        };

        run_and_update_metrics(
            SELECTION_PROOF_LOG_NAME,
            &validator_metrics::SIGNED_SELECTION_PROOFS_TOTAL,
            future,
        )
        .await
    }

    async fn produce_sync_selection_proof(
        &self,
        validator_pubkey: &PublicKeyBytes,
        slot: Slot,
        subnet_id: SyncSubnetId,
    ) -> Result<SyncSelectionProof, Error> {
        let future = async {
            let epoch = slot.epoch(E::slots_per_epoch());
            let domain_hash = self.get_domain(epoch, Domain::SyncCommitteeSelectionProof);
            let signing_root = SyncAggregatorSelectionData {
                slot,
                subcommittee_index: subnet_id.into(),
            }
            .signing_root(domain_hash);
            let (validator, cluster) = self.get_validator_and_cluster(*validator_pubkey)?;

            // Stop at two thirds of the slot. If the selection proof is not ready by then, we
            // will not produce an aggregation anyway.
            let delay = self.spec.get_slot_duration() * 2 / 3;

            let signature = if self.fork_schedule.active_fork(epoch) >= Fork::Boole {
                // Under Boole, sync selection proofs use the same committee path as attestation
                // selection proofs.
                let committee_id = cluster.committee_id();
                let voting_assignments = self.get_voting_assignments(slot).await?;

                // Defensive check: the validator should be present in `VotingAssignments` because
                // both this call and `VotingAssignments` come from `DutiesService`. If not, we
                // have an inconsistency, for example a stale cache after a poll timeout, and
                // should not participate with the wrong batch size.
                let validator_index = validator.index.ok_or(SpecificError::MissingIndex)?;
                if !voting_assignments
                    .sync_validators_by_subnet
                    .contains_key(&validator_index)
                {
                    return Err(SpecificError::ValidatorNotInSyncCommittee {
                        validator_pubkey: *validator_pubkey,
                        slot,
                    }
                    .into());
                }

                // Build a set of validator indices in this committee.
                // This handles divergent operator views, since we only count validators we have
                // shares for.
                let committee_validator_indices =
                    self.get_committee_validator_indices(&committee_id);

                // Count how many selection-proof partial signatures belong in this batch.
                let validator_partial_signature_batch_size = voting_assignments
                    .selection_proof_count_for_committee(|idx| {
                        committee_validator_indices.contains(idx)
                    });

                // Use the same deterministic batch ID as attestation selection proofs so both
                // attestation and sync selection proofs are sent in one committee message.
                let batch_id = SelectionProofBatchId::new(slot, committee_id);
                let base_hash = batch_id.hash();

                trace!(
                    %slot,
                    ?validator_index,
                    ?subnet_id,
                    "Producing committee sync selection proof"
                );

                let collection_mode = CollectionMode::Committee {
                    validator_partial_signature_batch_size,
                    base_hash,
                };

                self.timeout_within_slot(
                    slot,
                    delay,
                    self.collect_signature(
                        PartialSignatureKind::AggregatorCommitteePartialSig,
                        Role::AggregatorCommittee,
                        collection_mode,
                        &validator,
                        &cluster,
                        signing_root,
                        slot,
                    ),
                )
                .await?
            } else {
                self.timeout_within_slot(slot, delay, async {
                    let validator_index = validator.index.ok_or(SpecificError::MissingIndex)?;
                    // Anchor configures Lighthouse's sync selection-proof lookahead to one slot, so
                    // this proof slot is current and its assignments are published at slot start.
                    // A larger lookahead would make future proof slots wait for their assignments.
                    let voting_assignments = self.get_voting_assignments(slot).await?;
                    let position_counts = voting_assignments
                        .sync_validators_by_subnet
                        .get(&validator_index)
                        .ok_or(SpecificError::ValidatorNotInSyncCommittee {
                            validator_pubkey: *validator_pubkey,
                            slot,
                        })?;
                    let descriptor = self
                        .sync_selection_proof_descriptor(slot, subnet_id, position_counts)
                        .map_err(
                            |reason| SpecificError::InvalidSyncSelectionProofAssignment {
                                validator_pubkey: *validator_pubkey,
                                slot,
                                reason,
                            },
                        )?;

                    let collection_mode = sync_committee_collection_mode(subnet_id, descriptor);
                    self.collect_signature(
                        PartialSignatureKind::ContributionProofs,
                        Role::SyncCommittee,
                        collection_mode,
                        &validator,
                        &cluster,
                        signing_root,
                        slot,
                    )
                    .await
                })
                .await?
            };

            Ok(signature.into())
        };

        run_and_update_metrics(
            SYNC_SELECTION_PROOF_LOG_NAME,
            &validator_metrics::SIGNED_SYNC_SELECTION_PROOFS_TOTAL,
            future,
        )
        .await
    }

    fn sign_sync_committee_signatures(
        self: &Arc<Self>,
        messages: Vec<SyncMessageToSign>,
    ) -> impl Stream<Item = Result<Vec<SyncCommitteeMessage>, Error>> + Send {
        let _span = info_span!("sign_sync_committee_signatures").entered();
        let committee_mapping = self.group_by_committee(messages, |m| m.pubkey);

        // Process each committee concurrently, streaming results as each completes
        let committee_futures: FuturesUnordered<_> = committee_mapping
            .into_iter()
            .map(|(committee_id, (cluster, messages))| {
                let this = Arc::clone(self);
                async move {
                    run_committee_signing(
                        committee_id,
                        messages.len(),
                        &validator_metrics::SIGNED_SYNC_COMMITTEE_MESSAGES_TOTAL,
                        this.sign_committee_sync_committee_signatures(
                            committee_id,
                            cluster,
                            messages,
                        ),
                    )
                    .await
                }
            })
            .collect();

        committee_futures
    }

    fn sign_sync_committee_contributions(
        self: &Arc<Self>,
        contributions: Vec<ContributionToSign<E>>,
    ) -> impl Stream<Item = Result<Vec<SignedContributionAndProof<E>>, Error>> + Send {
        let this = Arc::clone(self);
        stream::once(async move {
            // Empty input: nothing to sign and no fork to determine.
            let Some(first) = contributions.first() else {
                return Ok(Vec::new());
            };

            if !this
                .lighthouse_owns_publication(first.contribution.slot.epoch(E::slots_per_epoch()))
            {
                // Boole+: hand Lighthouse an empty batch, which its publish loop drops silently.
                // Publication ownership lives with the metadata service's publisher, which signs
                // and publishes the decided worklist regardless of whether Lighthouse's duty
                // snapshot saw the sync selection proofs in time. See
                // [`crate::aggregator_post_consensus`].
                //
                // Lighthouse's request set is its snapshot's view of who aggregates; the
                // publisher only ever sees the decided view. Logging the request set here keeps
                // divergent operator views diagnosable by comparing the two.
                debug!(
                    slot = %first.contribution.slot,
                    // Lighthouse requests contributions per subnet, so one call is one subnet;
                    // the publisher's logs key on subcommittee index, and this field is the join.
                    subnet = first.contribution.subcommittee_index,
                    requested = contributions.len(),
                    aggregators = ?contributions
                        .iter()
                        .map(|contrib| contrib.aggregator_index)
                        .collect::<Vec<_>>(),
                    "Deferring Lighthouse-requested contributions to the decided-value publisher"
                );
                return Ok(Vec::new());
            }

            // Pre-Boole: per-validator processing, Lighthouse publishes the results
            let futures = contributions.into_iter().map(|contrib| {
                let this = Arc::clone(&this);
                async move { this.sign_single_sync_committee_contribution(contrib).await }
            });
            let results = join_all(futures).await;
            Ok(results.into_iter().filter_map(|r| r.ok()).collect())
        })
    }

    // stolen from lighthouse
    /// Prune the slashing protection database so that it remains performant.
    ///
    /// This function will only do actual pruning periodically, so it should usually be
    /// cheap to call. The `first_run` flag can be used to print a more verbose message when pruning
    /// runs.
    fn prune_slashing_protection_db(&self, current_epoch: Epoch, first_run: bool) {
        // Attempt to prune every SLASHING_PROTECTION_HISTORY_EPOCHs, with a tolerance for
        // missing the epoch that aligns exactly.
        let mut last_prune = self.slashing_protection_last_prune.lock();
        if current_epoch / SLASHING_PROTECTION_HISTORY_EPOCHS
            <= *last_prune / SLASHING_PROTECTION_HISTORY_EPOCHS
        {
            return;
        }

        if first_run {
            info!(
                "epoch" = %current_epoch,
                "msg" = "pruning may take several minutes the first time it runs",
                "Pruning slashing protection DB",
            );
        } else {
            info!(
                "epoch" = %current_epoch,
                "Pruning slashing protection DB",
            );
        }

        let _timer =
            validator_metrics::start_timer(&validator_metrics::SLASHING_PROTECTION_PRUNE_TIMES);

        let new_min_target_epoch = current_epoch.saturating_sub(SLASHING_PROTECTION_HISTORY_EPOCHS);
        let new_min_slot = new_min_target_epoch.start_slot(E::slots_per_epoch());

        let all_pubkeys: Vec<_> = self.voting_pubkeys(DoppelgangerStatus::ignored);

        if let Err(e) = self
            .slashing_protection
            .prune_all_signed_attestations(all_pubkeys.iter(), new_min_target_epoch)
        {
            error!(
                "error" = ?e,
                "Error during pruning of signed attestations",
            );
            return;
        }

        if let Err(e) = self
            .slashing_protection
            .prune_all_signed_blocks(all_pubkeys.iter(), new_min_slot)
        {
            error!(
                "error" = ?e,
                "Error during pruning of signed blocks",
            );
            return;
        }

        *last_prune = current_epoch;

        info!("Completed pruning of slashing protection DB");
    }

    fn proposal_data(&self, pubkey: &PublicKeyBytes) -> Option<ProposalData> {
        let state = self.database.state();
        let validator = state.metadata().get_by(pubkey)?;

        let validator_index = validator.index.map(|idx| *idx as u64);
        let cluster = state.clusters().get_by(&validator.cluster_id);
        let fee_recipient = cluster.map(|c| c.fee_recipient);

        Some(ProposalData {
            validator_index,
            fee_recipient,
            gas_limit: self.gas_limit,
            builder_proposals: true,
        })
    }

    fn sign_attestations(
        self: &Arc<Self>,
        attestations: Vec<AttestationToSign<E>>,
    ) -> impl Stream<Item = Result<Vec<(u64, Attestation<Self::E>)>, Error>> + Send {
        if !*self.is_synced.borrow() {
            return Either::Left(stream::once(futures::future::ready(Err(
                Error::SpecificError(SpecificError::NotSynced),
            ))));
        }

        let _span = info_span!("sign_attestations").entered();
        let committee_mapping = self.group_by_committee(attestations, |a| a.pubkey);

        // Process each committee concurrently, streaming results as each completes.
        // Each committee runs consensus + batch signing + slashing protection independently.
        let committee_futures: FuturesUnordered<_> = committee_mapping
            .into_iter()
            .map(|(committee_id, (cluster, attestations))| {
                let this = Arc::clone(self);
                async move {
                    let signed = match this
                        .sign_committee_attestations(committee_id, cluster, attestations)
                        .await
                    {
                        Ok(signed) if signed.is_empty() => return Ok(Vec::new()),
                        Ok(signed) => signed,
                        Err(e) => {
                            error!(?committee_id, error = ?e, "Failed to sign committee attestations");
                            return Ok(Vec::new());
                        }
                    };

                    // Check slashing protection on a blocking thread
                    let vs = Arc::clone(&this);
                    this.task_executor
                        .spawn_blocking_handle(
                            move || vs.slashing_protection_attestations(signed),
                            "slashing_protect_attestations",
                        )
                        .ok_or(Error::ExecutorError)?
                        .await
                        .map_err(|_| Error::ExecutorError)?
                }
            })
            .collect();

        Either::Right(committee_futures)
    }

    async fn sign_execution_payload_envelope(
        &self,
        _validator_pubkey: PublicKeyBytes,
        _envelope: ExecutionPayloadEnvelope<E>,
    ) -> Result<SignedExecutionPayloadEnvelope<E>, Error> {
        Err(Error::SpecificError(SpecificError::Unsupported))
    }

    async fn sign_payload_attestation(
        &self,
        _validator_pubkey: PublicKeyBytes,
        _data: PayloadAttestationData,
    ) -> Result<PayloadAttestationMessage, Error> {
        Err(Error::SpecificError(SpecificError::Unsupported))
    }

    async fn sign_proposer_preferences(
        &self,
        _validator_pubkey: PublicKeyBytes,
        _preferences: ProposerPreferences,
    ) -> Result<SignedProposerPreferences, Error> {
        Err(Error::SpecificError(SpecificError::Unsupported))
    }
}

struct PublishDecision<E: EthSpec> {
    signed_block: Option<SignedBlock<E>>,
    proposal_matched: bool,
    publish_path: &'static str,
}

const PUBLISH_PATH_RECONSTRUCTED_FULL_BLOCK_AS_LEADER: &str = "reconstructed_full_block_as_leader";
const PUBLISH_PATH_BLINDED_BLOCK_AS_LEADER: &str = "blinded_block_as_leader";
const PUBLISH_PATH_BLINDED_BLOCK_NOT_LEADER: &str = "blinded_block_not_leader";
const PUBLISH_PATH_SKIPPED_BLINDED_BLOCK_NOT_LEADER_PRE_GLOAS: &str =
    "skipped_blinded_block_not_leader_pre_gloas";
const PUBLISH_PATH_FULL_BLOCK_DIRECTLY: &str = "full_block_directly";

fn select_publish_block<E: EthSpec>(
    signed_block: SignedBlock<E>,
    original_blinded_block: &BeaconBlock<E, BlindedPayload<E>>,
    local_full_block: Option<BlockContentsTuple<E>>,
) -> PublishDecision<E> {
    match signed_block {
        SignedBlock::Blinded(signed_blinded_block) => {
            let proposal_matched = signed_blinded_block.signed_block_header().message
                == original_blinded_block.block_header();

            // An unmatched local full block cannot safely reconstruct the decided proposal, and a
            // separate pre-Gloas beacon node cannot unblind a payload built by another operator.
            // Use the locally requested fork so peer-decided data cannot change this boundary.
            if !proposal_matched
                && !original_blinded_block
                    .to_ref()
                    .fork_name_unchecked()
                    .gloas_enabled()
            {
                return PublishDecision {
                    signed_block: None,
                    proposal_matched,
                    publish_path: PUBLISH_PATH_SKIPPED_BLINDED_BLOCK_NOT_LEADER_PRE_GLOAS,
                };
            }

            if !proposal_matched {
                return PublishDecision {
                    signed_block: Some(SignedBlock::Blinded(signed_blinded_block)),
                    proposal_matched,
                    publish_path: PUBLISH_PATH_BLINDED_BLOCK_NOT_LEADER,
                };
            }

            match local_full_block {
                Some((full_block, proofs_and_blobs)) => {
                    let signed_full_block = SignedBeaconBlock::from_block(
                        full_block,
                        signed_blinded_block.signature().clone(),
                    );

                    PublishDecision {
                        signed_block: Some(SignedBlock::Full(PublishBlockRequest::new(
                            Arc::new(signed_full_block),
                            proofs_and_blobs,
                        ))),
                        proposal_matched,
                        publish_path: PUBLISH_PATH_RECONSTRUCTED_FULL_BLOCK_AS_LEADER,
                    }
                }
                None => PublishDecision {
                    signed_block: Some(SignedBlock::Blinded(signed_blinded_block)),
                    proposal_matched,
                    publish_path: PUBLISH_PATH_BLINDED_BLOCK_AS_LEADER,
                },
            }
        }
        SignedBlock::Full(signed_block) => PublishDecision {
            signed_block: Some(SignedBlock::Full(signed_block)),
            proposal_matched: false,
            publish_path: PUBLISH_PATH_FULL_BLOCK_DIRECTLY,
        },
    }
}

trait SignableBlock<E: EthSpec>: Debug + Encode {
    type Payload: AbstractExecPayload<E>;

    fn as_block(&self) -> BeaconBlockRef<'_, E, Self::Payload>;
    fn to_signed_block(self, signature: Signature) -> SignedBlock<E>;
}

impl<E: EthSpec> SignableBlock<E> for BlockContents<E> {
    type Payload = FullPayload<E>;

    fn as_block(&self) -> BeaconBlockRef<'_, E, Self::Payload> {
        self.block.to_ref()
    }

    fn to_signed_block(self, signature: Signature) -> SignedBlock<E> {
        SignedBlock::Full(PublishBlockRequest::new(
            Arc::new(SignedBeaconBlock::from_block(self.block, signature)),
            Some((self.kzg_proofs, self.blobs)),
        ))
    }
}

impl<E: EthSpec> SignableBlock<E> for BeaconBlock<E, FullPayload<E>> {
    type Payload = FullPayload<E>;

    fn as_block(&self) -> BeaconBlockRef<'_, E, Self::Payload> {
        self.to_ref()
    }

    fn to_signed_block(self, signature: Signature) -> SignedBlock<E> {
        SignedBlock::Full(PublishBlockRequest::new(
            Arc::new(SignedBeaconBlock::from_block(self, signature)),
            None,
        ))
    }
}

impl<E: EthSpec> SignableBlock<E> for BeaconBlock<E, BlindedPayload<E>> {
    type Payload = BlindedPayload<E>;

    fn as_block(&self) -> BeaconBlockRef<'_, E, Self::Payload> {
        self.to_ref()
    }

    fn to_signed_block(self, signature: Signature) -> SignedBlock<E> {
        SignedBlock::Blinded(Arc::new(SignedBlindedBeaconBlock::from_block(
            self, signature,
        )))
    }
}

#[cfg(test)]
mod testing;

#[cfg(test)]
mod tests {
    use types::{BeaconBlockFulu, BeaconBlockGloas, EmptyBlock, MainnetEthSpec};

    use super::*;

    type TestFullBlock = BeaconBlock<MainnetEthSpec, FullPayload<MainnetEthSpec>>;
    type TestBlindedBlock = BeaconBlock<MainnetEthSpec, BlindedPayload<MainnetEthSpec>>;

    fn fulu_block(state_root: Hash256) -> TestFullBlock {
        let spec = ForkName::Fulu.make_genesis_spec(MainnetEthSpec::default_spec());
        let mut block = BeaconBlockFulu::empty(&spec);
        block.state_root = state_root;
        BeaconBlock::Fulu(block)
    }

    fn gloas_block(state_root: Hash256) -> TestFullBlock {
        let spec = ForkName::Gloas.make_genesis_spec(MainnetEthSpec::default_spec());
        let mut block = BeaconBlockGloas::empty(&spec);
        block.state_root = state_root;
        BeaconBlock::Gloas(block)
    }

    fn blinded(block: &TestFullBlock) -> TestBlindedBlock {
        block.to_ref().into()
    }

    fn signed_blinded(
        block: TestBlindedBlock,
        signature: Signature,
    ) -> SignedBlock<MainnetEthSpec> {
        SignedBlock::Blinded(Arc::new(SignedBlindedBeaconBlock::from_block(
            block, signature,
        )))
    }

    #[test]
    fn select_publish_block_reconstructs_matching_pre_gloas_leader_block() {
        // Arrange
        let full_block = fulu_block(Hash256::repeat_byte(1));
        let original_blinded_block = blinded(&full_block);
        let signature = Signature::empty();
        let signed_block = signed_blinded(original_blinded_block.clone(), signature.clone());

        // Act
        let decision = select_publish_block(
            signed_block,
            &original_blinded_block,
            Some((full_block, None)),
        );

        // Assert
        assert!(decision.proposal_matched);
        assert_eq!(
            decision.publish_path,
            PUBLISH_PATH_RECONSTRUCTED_FULL_BLOCK_AS_LEADER
        );
        let Some(SignedBlock::Full(publish_request)) = decision.signed_block else {
            panic!("matching pre-Gloas leader block should be reconstructed in full");
        };
        assert_eq!(
            publish_request.signed_block().message().block_header(),
            original_blinded_block.block_header()
        );
        assert_eq!(publish_request.signed_block().signature(), &signature);
    }

    #[test]
    fn select_publish_block_skips_unmatched_pre_gloas_non_leader_block() {
        // Arrange
        let local_full_block = fulu_block(Hash256::repeat_byte(1));
        let original_blinded_block = blinded(&local_full_block);
        let decided_blinded_block = blinded(&fulu_block(Hash256::repeat_byte(2)));
        let signed_block = signed_blinded(decided_blinded_block, Signature::empty());

        // Act
        let decision = select_publish_block(
            signed_block,
            &original_blinded_block,
            Some((local_full_block, None)),
        );

        // Assert
        assert!(!decision.proposal_matched);
        assert_eq!(
            decision.publish_path,
            PUBLISH_PATH_SKIPPED_BLINDED_BLOCK_NOT_LEADER_PRE_GLOAS
        );
        assert!(decision.signed_block.is_none());
    }

    #[test]
    fn select_publish_block_uses_local_pre_gloas_fork_when_decided_block_is_gloas() {
        // Arrange
        let local_full_block = fulu_block(Hash256::repeat_byte(1));
        let original_blinded_block = blinded(&local_full_block);
        let decided_blinded_block = blinded(&gloas_block(Hash256::repeat_byte(2)));
        let signed_block = signed_blinded(decided_blinded_block, Signature::empty());

        // Act
        let decision = select_publish_block(
            signed_block,
            &original_blinded_block,
            Some((local_full_block, None)),
        );

        // Assert
        assert!(!decision.proposal_matched);
        assert_eq!(
            decision.publish_path,
            PUBLISH_PATH_SKIPPED_BLINDED_BLOCK_NOT_LEADER_PRE_GLOAS
        );
        assert!(decision.signed_block.is_none());
    }

    #[test]
    fn select_publish_block_keeps_unmatched_gloas_non_leader_block_publishable() {
        // Arrange
        let local_full_block = gloas_block(Hash256::repeat_byte(1));
        let original_blinded_block = blinded(&local_full_block);
        let decided_blinded_block = blinded(&gloas_block(Hash256::repeat_byte(2)));
        let signed_block = signed_blinded(decided_blinded_block, Signature::empty());

        // Act
        let decision = select_publish_block(
            signed_block,
            &original_blinded_block,
            Some((local_full_block, None)),
        );

        // Assert
        assert!(!decision.proposal_matched);
        assert_eq!(decision.publish_path, PUBLISH_PATH_BLINDED_BLOCK_NOT_LEADER);
        let Some(SignedBlock::Blinded(signed_blinded_block)) = decision.signed_block else {
            panic!("unmatched Gloas block should retain the existing blinded publish path");
        };
        assert!(
            signed_blinded_block
                .as_ref()
                .clone()
                .try_into_full_block(None)
                .is_some(),
            "Gloas blocks must remain reconstructable without an execution payload"
        );
    }

    #[test]
    fn select_publish_block_uses_local_gloas_fork_when_decided_block_is_pre_gloas() {
        // Arrange
        let local_full_block = gloas_block(Hash256::repeat_byte(1));
        let original_blinded_block = blinded(&local_full_block);
        let decided_blinded_block = blinded(&fulu_block(Hash256::repeat_byte(2)));
        let signed_block = signed_blinded(decided_blinded_block, Signature::empty());

        // Act
        let decision = select_publish_block(
            signed_block,
            &original_blinded_block,
            Some((local_full_block, None)),
        );

        // Assert
        assert!(!decision.proposal_matched);
        assert_eq!(decision.publish_path, PUBLISH_PATH_BLINDED_BLOCK_NOT_LEADER);
        assert!(matches!(
            decision.signed_block,
            Some(SignedBlock::Blinded(_))
        ));
    }

    #[test]
    fn select_publish_block_keeps_full_gloas_block_direct() {
        // Arrange
        let original_blinded_block = blinded(&gloas_block(Hash256::repeat_byte(1)));
        let full_block = gloas_block(Hash256::repeat_byte(2));
        let expected_header = full_block.block_header();
        let signature = Signature::empty();
        let signed_block = SignedBlock::Full(PublishBlockRequest::new(
            Arc::new(SignedBeaconBlock::from_block(full_block, signature.clone())),
            None,
        ));

        // Act
        let decision = select_publish_block(signed_block, &original_blinded_block, None);

        // Assert
        assert!(!decision.proposal_matched);
        assert_eq!(decision.publish_path, PUBLISH_PATH_FULL_BLOCK_DIRECTLY);
        let Some(SignedBlock::Full(publish_request)) = decision.signed_block else {
            panic!("full Gloas block should remain on the direct publish path");
        };
        assert_eq!(
            publish_request.signed_block().message().block_header(),
            expected_header
        );
        assert_eq!(publish_request.signed_block().signature(), &signature);
    }

    // ═══════════════════════════════════════════════════════════════════════════════
    // Proposer delay
    // ═══════════════════════════════════════════════════════════════════════════════

    /// Decision for `delay_ms` when pre-consensus finished `elapsed_ms` into the slot.
    fn decision_at(delay_ms: u64, elapsed_ms: u64) -> ProposerDelayDecision {
        proposer_delay_decision_at(
            Duration::from_millis(delay_ms),
            Some(Duration::from_millis(elapsed_ms)),
        )
    }

    #[test]
    fn elapsed_in_slot_does_not_wrap_when_a_duty_overruns() {
        const SLOT_DURATION: Duration = Duration::from_secs(12);
        let clock =
            slot_clock::ManualSlotClock::new(Slot::new(0), Duration::from_secs(0), SLOT_DURATION);
        let duty_slot = Slot::new(100);
        clock.set_slot(duty_slot.as_u64());

        assert_eq!(elapsed_in_slot(&clock, duty_slot), Some(Duration::ZERO));
        clock.advance_time(Duration::from_millis(300));
        assert_eq!(
            elapsed_in_slot(&clock, duty_slot),
            Some(Duration::from_millis(300))
        );

        // Past the end of the duty's slot the offset must keep growing; a shrinking one would
        // re-arm the floor. `determine_slot_elapsed_ms` reports this same instant as a fresh 300ms.
        clock.advance_time(SLOT_DURATION);
        assert_eq!(
            elapsed_in_slot(&clock, duty_slot),
            Some(SLOT_DURATION + Duration::from_millis(300))
        );
        assert_eq!(determine_slot_elapsed_ms(&clock), Some(300));
    }

    #[test]
    fn proposer_delay_zero_is_disabled() {
        // Must stay distinguishable from "configured but did not bite".
        assert_eq!(decision_at(0, 0), ProposerDelayDecision::Disabled);
        assert_eq!(decision_at(0, 5_000), ProposerDelayDecision::Disabled);
    }

    #[test]
    fn proposer_delay_waits_out_the_remainder_only() {
        // Finished 100ms in with a 300ms floor leaves 200ms: an offset, not 300ms of added latency.
        assert_eq!(
            decision_at(300, 100),
            ProposerDelayDecision::Waited(Duration::from_millis(200))
        );
        assert_eq!(
            decision_at(300, 0),
            ProposerDelayDecision::Waited(Duration::from_millis(300))
        );
    }

    #[test]
    fn proposer_delay_does_not_wait_once_the_target_has_passed() {
        // A late duty must not be delayed further.
        assert_eq!(decision_at(300, 301), ProposerDelayDecision::TargetPassed);
        assert_eq!(decision_at(300, 4_000), ProposerDelayDecision::TargetPassed);
    }

    #[test]
    fn proposer_delay_target_boundary_is_not_a_wait() {
        // Guards against a zero-length sleep being reported as a wait.
        assert_eq!(decision_at(300, 300), ProposerDelayDecision::TargetPassed);
    }

    #[test]
    fn proposer_delay_fails_open_without_a_clock() {
        // An unavailable clock must never block a proposal.
        assert_eq!(
            proposer_delay_decision_at(Duration::from_millis(300), None),
            ProposerDelayDecision::ClockUnavailable
        );
    }

    #[test]
    fn proposer_delay_handles_a_duty_that_overran_its_slot() {
        // A duty longer than a full slot arrives as a large elapsed value, and must read as
        // "already past the target", never as a fresh duty with time to spare.
        assert_eq!(
            decision_at(300, 12_100),
            ProposerDelayDecision::TargetPassed
        );
        assert_eq!(
            proposer_delay_decision_at(Duration::from_millis(300), Some(Duration::MAX)),
            ProposerDelayDecision::TargetPassed
        );
    }

    /// Time is paused, so `sleep` advances the clock without spending any, and
    /// [`tokio::time::Instant`] observes that same virtual clock. These assert the wait is actually
    /// applied, which the decision tests above cannot: they only prove what was decided.
    #[tokio::test(start_paused = true)]
    async fn await_proposer_delay_sleeps_only_the_remainder() {
        let started = Instant::now();
        await_proposer_delay(Duration::from_millis(300), Some(Duration::from_millis(100))).await;
        assert_eq!(started.elapsed(), Duration::from_millis(200));
    }

    #[tokio::test(start_paused = true)]
    async fn await_proposer_delay_never_sleeps_without_a_wait() {
        // Every no-wait decision must return promptly: an overdue duty, a missing clock, and the
        // disabled default. A regression here would delay a proposal that has no time to spare.
        for (delay_ms, elapsed) in [
            (300, Some(Duration::from_millis(400))),
            (300, None),
            (0, Some(Duration::ZERO)),
        ] {
            let started = Instant::now();
            await_proposer_delay(Duration::from_millis(delay_ms), elapsed).await;
            assert_eq!(
                started.elapsed(),
                Duration::ZERO,
                "delay={delay_ms}ms elapsed={elapsed:?} must not sleep"
            );
        }
    }

    #[test]
    fn proposer_delay_outcome_labels_are_stable() {
        // These label values are a dashboard contract, and they are documented by name in
        // docs/docs/pages/mev_configuration.mdx. Renaming one silently breaks operator queries,
        // and duplicating one silently merges two metric series.
        assert_eq!(ProposerDelayDecision::Disabled.as_str(), "disabled");
        assert_eq!(
            ProposerDelayDecision::TargetPassed.as_str(),
            "target_passed"
        );
        assert_eq!(
            ProposerDelayDecision::Waited(Duration::from_millis(1)).as_str(),
            "waited"
        );
        assert_eq!(
            ProposerDelayDecision::ClockUnavailable.as_str(),
            "clock_unavailable"
        );
    }

    #[test]
    fn proposer_delay_only_waited_yields_a_wait() {
        assert_eq!(
            ProposerDelayDecision::Waited(Duration::from_millis(200)).wait(),
            Some(Duration::from_millis(200))
        );
        assert!(ProposerDelayDecision::Disabled.wait().is_none());
        assert!(ProposerDelayDecision::TargetPassed.wait().is_none());
        assert!(ProposerDelayDecision::ClockUnavailable.wait().is_none());
    }

    /// Creates a test `VotingAssignments` with the given parameters.
    fn create_test_voting_assignments(
        attesting_validators: Vec<usize>,
        sync_validators_by_subnet: Vec<(usize, Vec<u64>)>,
    ) -> VotingAssignments {
        VotingAssignments {
            slot: Slot::new(100),
            attesting_validators: attesting_validators
                .into_iter()
                .map(ValidatorIndex)
                .collect(),
            attesting_committees: HashMap::new(),
            sync_validators_by_subnet: sync_validators_by_subnet
                .into_iter()
                .map(|(idx, subnets)| {
                    let position_counts =
                        subnets
                            .into_iter()
                            .fold(HashMap::new(), |mut counts, subnet_id| {
                                *counts.entry(SyncSubnetId::new(subnet_id)).or_insert(0) += 1;
                                counts
                            });
                    (ValidatorIndex(idx), position_counts)
                })
                .collect(),
        }
    }

    #[test]
    fn test_selection_proof_count_with_multi_subnet_validators() {
        // Create voting assignments with:
        // - Validators 1, 2 attesting
        // - Validator 3 in 1 subnet (contributes 1)
        // - Validator 4 in 3 subnets (contributes 3)
        // - Validator 5 in 2 subnets (contributes 2)
        let voting_assignments = create_test_voting_assignments(
            vec![1, 2],
            vec![(3, vec![0]), (4, vec![0, 1, 2]), (5, vec![0, 1])],
        );

        // All validators in committee
        let all_in_committee = |_: &ValidatorIndex| true;
        let count = voting_assignments.selection_proof_count_for_committee(all_in_committee);
        // 2 attesting + 1 + 3 + 2 sync = 8
        assert_eq!(count, 8);
    }

    #[test]
    fn test_selection_proof_count_with_filter() {
        let voting_assignments = create_test_voting_assignments(
            vec![1, 2, 3],
            vec![
                (4, vec![0, 1]),    // 2 subnets
                (5, vec![0, 1, 2]), // 3 subnets
            ],
        );

        // Only validators 1, 2, 4 are in the committee
        let in_committee = |idx: &ValidatorIndex| matches!(idx.0, 1 | 2 | 4);
        let count = voting_assignments.selection_proof_count_for_committee(in_committee);
        // 2 attesting (1, 2) + 2 sync subnets (validator 4) = 4
        assert_eq!(count, 4);
    }

    #[test]
    fn test_committee_message_count_with_multi_subnet_validators() {
        // Same setup as selection proof test, but counting should be flat
        let voting_assignments = create_test_voting_assignments(
            vec![1, 2],
            vec![
                (3, vec![0]),
                (4, vec![0, 1, 2]), // 3 subnets but counts as 1
                (5, vec![0, 1]),    // 2 subnets but counts as 1
            ],
        );

        let all_in_committee = |_: &ValidatorIndex| true;
        let count = voting_assignments.voting_message_count_for_committee(all_in_committee);
        // 2 attesting + 3 sync validators (flat) = 5
        assert_eq!(count, 5);
    }

    #[test]
    fn test_committee_message_count_with_filter() {
        let voting_assignments = create_test_voting_assignments(
            vec![1, 2, 3],
            vec![
                (4, vec![0, 1]),    // 2 subnets
                (5, vec![0, 1, 2]), // 3 subnets
            ],
        );

        // Only validators 1, 2, 4 are in the committee
        let in_committee = |idx: &ValidatorIndex| matches!(idx.0, 1 | 2 | 4);
        let count = voting_assignments.voting_message_count_for_committee(in_committee);
        // 2 attesting (1, 2) + 1 sync (validator 4) = 3
        assert_eq!(count, 3);
    }

    #[test]
    fn test_counting_difference_between_methods() {
        // Demonstrate the key difference between the two counting methods
        let voting_assignments = create_test_voting_assignments(
            vec![1], // 1 attesting validator
            vec![
                (2, vec![0, 1, 2, 3]), // Validator in 4 subnets
            ],
        );

        let all_in_committee = |_: &ValidatorIndex| true;

        // Selection proof: 1 + 4 = 5
        let selection_count =
            voting_assignments.selection_proof_count_for_committee(all_in_committee);
        assert_eq!(selection_count, 5);

        // Voting message: 1 + 1 = 2
        let message_count = voting_assignments.voting_message_count_for_committee(all_in_committee);
        assert_eq!(message_count, 2);

        // The difference highlights the counting patterns:
        // - Selection proofs need one proof per subnet per validator
        // - Voting messages need one message per validator regardless of subnets
    }

    #[test]
    fn test_position_multiplicity_does_not_inflate_boole_counts() {
        let voting_assignments = create_test_voting_assignments(vec![], vec![(1, vec![0, 0, 1])]);
        let all_in_committee = |_: &ValidatorIndex| true;

        assert_eq!(
            voting_assignments.selection_proof_count_for_committee(all_in_committee),
            2,
            "Boole selection-proof batching counts unique subnet keys"
        );
        assert_eq!(
            voting_assignments.voting_message_count_for_committee(all_in_committee),
            1,
            "voting-message batching counts the validator key once"
        );
        assert_eq!(
            voting_assignments.sync_validators_by_subnet[&ValidatorIndex(1)],
            HashMap::from([(SyncSubnetId::new(0), 2), (SyncSubnetId::new(1), 1)])
        );
    }

    #[test]
    fn test_overlapping_attesting_and_sync_validators() {
        // Validator can be both attesting and in sync committee
        let mut voting_assignments = create_test_voting_assignments(
            vec![1, 2], // Validators 1 and 2 attesting
            vec![
                (1, vec![0]),    // Validator 1 also in sync (1 subnet)
                (2, vec![0, 1]), // Validator 2 also in sync (2 subnets)
            ],
        );
        voting_assignments.attesting_validators = vec![ValidatorIndex(1), ValidatorIndex(2)];

        let all_in_committee = |_: &ValidatorIndex| true;

        // Selection proof: 2 attesting + (1 + 2) sync = 5
        let selection_count =
            voting_assignments.selection_proof_count_for_committee(all_in_committee);
        assert_eq!(selection_count, 5);

        // Voting message: 2 attesting + 2 sync = 4
        let message_count = voting_assignments.voting_message_count_for_committee(all_in_committee);
        assert_eq!(message_count, 4);
    }

    #[tokio::test]
    async fn test_validator_voting_assignments_watch_channel_waits_for_update() {
        // Test the watch channel behavior directly without creating a full AnchorValidatorStore
        let (tx, mut rx) = watch::channel::<Option<Arc<VotingAssignments>>>(None);

        // Start a task to wait for slot 5
        let wait_task = tokio::spawn(async move {
            loop {
                let current = rx.borrow().clone();
                if let Some(voting_assignments) = current
                    && voting_assignments.slot == Slot::new(5)
                {
                    return Ok::<_, ()>(voting_assignments);
                }
                // Wait for update
                if rx.changed().await.is_err() {
                    return Err(());
                }
            }
        });

        // Give the task time to start waiting
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Update with voting assignments for slot 5
        let voting_assignments = VotingAssignments {
            slot: Slot::new(5),
            attesting_validators: vec![ValidatorIndex(1)],
            attesting_committees: HashMap::new(),
            sync_validators_by_subnet: HashMap::new(),
        };
        tx.send_replace(Some(Arc::new(voting_assignments)));

        // The wait task should now complete successfully
        let result = wait_task.await.unwrap();
        assert!(result.is_ok());
        let received = result.unwrap();
        assert_eq!(received.slot, Slot::new(5));
        assert_eq!(received.attesting_validators, vec![ValidatorIndex(1)]);
    }

    #[tokio::test]
    async fn test_validator_voting_assignments_errors_if_slot_passed() {
        // Test the watch channel behavior directly
        let (tx, rx) = watch::channel::<Option<Arc<VotingAssignments>>>(None);

        // Update with voting assignments for slot 10 (newer than what we'll request)
        let voting_assignments = VotingAssignments {
            slot: Slot::new(10),
            attesting_validators: vec![],
            attesting_committees: HashMap::new(),
            sync_validators_by_subnet: HashMap::new(),
        };
        tx.send_replace(Some(Arc::new(voting_assignments)));

        // Simulate requesting slot 5 (older than cached slot 10)
        let current = rx.borrow().clone();
        if let Some(voting_assignments) = current {
            if voting_assignments.slot > Slot::new(5) {
                // This simulates the error condition we'd return
                assert_eq!(voting_assignments.slot, Slot::new(10));
            } else {
                panic!("Should have newer slot cached");
            }
        } else {
            panic!("Should have voting assignments cached");
        }
    }
}
