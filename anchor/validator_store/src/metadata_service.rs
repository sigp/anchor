use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::Arc,
    time::Duration,
};

use beacon_node_fallback::{BeaconNodeFallback, beacon_head_monitor::HeadEvent};
use bls::PublicKeyBytes;
use eth2::{
    BeaconNodeHttpClient,
    types::{BlockId, SyncContributionData},
};
use futures::{
    FutureExt,
    stream::{FuturesUnordered, StreamExt},
};
use slot_clock::SlotClock;
use ssv_types::{
    CommitteeId, IndexSet, ValidatorIndex, VariableList,
    consensus::{
        AggregatorCommitteeConsensusData, AssignedAggregator, BeaconVote, DataVersion,
        GloasBeaconVote,
    },
    typenum::Unsigned,
};
use ssz::Encode;
use task_executor::TaskExecutor;
use tokio::{
    sync::mpsc,
    time::{Instant, sleep, sleep_until},
};
use tracing::{Instrument, debug, error, info, info_span, trace, warn};
use tree_hash::TreeHash;
use types::{
    Attestation, AttestationData, ChainSpec, EthSpec, ForkName, Hash256, SignedAggregateAndProof,
    SignedContributionAndProof, Slot, SyncCommitteeContribution, SyncSelectionProof, SyncSubnetId,
};
use validator_services::duties_service::{DutiesService, DutyAndProof};

use crate::{
    AggregationAssignments, AnchorValidatorStore, ContributionWaiter, SlotVote, VotingAssignments,
    VotingContext, aggregator_post_consensus::AggregatorPostConsensusShared, metrics,
};

/// Data for sync committee aggregators.
struct SyncAggregatorData {
    validator_index: u64,
    pubkey: PublicKeyBytes,
    selection_proof: SyncSelectionProof,
}

/// Map from SSV committee to its sync aggregators grouped by subnet.
type SyncByCommitteeMap = HashMap<CommitteeId, Vec<(SyncSubnetId, SyncAggregatorData)>>;
type SyncSubnetPositionCounts = HashMap<SyncSubnetId, usize>;

/// Identifies one aggregate-attestation Beacon API request within a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct AggregateFetchKey {
    attestation_data_root: Hash256,
    committee_index: u64,
}

/// Identifies one sync-contribution Beacon API request within a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SyncContributionFetchKey {
    block_root: Hash256,
    subnet_id: SyncSubnetId,
}

/// Resolved committee votes and their deduplicated Beacon API request keys.
struct ResolvedCommitteeRequests {
    votes: HashMap<CommitteeId, SlotVote>,
    aggregate_attestation_keys: HashSet<AggregateFetchKey>,
    sync_contribution_keys: HashSet<SyncContributionFetchKey>,
}

/// Deduplicated Beacon API results shared by all SSV committees in one slot.
struct AggregationFetchResults<E: EthSpec> {
    aggregated_attestations: HashMap<AggregateFetchKey, Attestation<E>>,
    sync_contributions: HashMap<SyncContributionFetchKey, SyncCommitteeContribution<E>>,
}

/// Maximum time to wait for beacon node API calls to fetch aggregated attestations
/// and sync contributions. After this timeout, we return whatever partial results
/// have been collected. This is shorter than the standard 3-second Lighthouse timeout
/// because SSV has additional latency for QBFT consensus and P2P propagation.
const BEACON_API_FETCH_TIMEOUT: Duration = Duration::from_secs(2);

// Weighted Attestation Data (WAD) timeouts.
//
// SSV-Go uses 5s hard / 2s soft / 1s block-lookup (derived from `CommonTimeout`).
// We halve these because our Rust implementation has lower per-request overhead
// and we want to minimise the delay before QBFT consensus begins.
const WAD_SOFT_TIMEOUT: Duration = Duration::from_secs(1);
const WAD_HARD_TIMEOUT: Duration = Duration::from_secs(3);
const BLOCK_SLOT_LOOKUP_TIMEOUT: Duration = Duration::from_millis(500);

/// Publish `VotingAssignments` this long after the slot boundary.
///
/// The boundary sleep runs on the monotonic timer but the slot is read from the
/// wall clock, and the two drift apart by a few milliseconds over a full-slot
/// sleep. A wake landing marginally before the boundary would read the previous
/// slot, republish it, and skip the new one (issue #1223). This margin absorbs
/// that drift. 50 ms mirrors the message validator's `CLOCK_ERROR_TOLERANCE`,
/// the clock error the SSV network already budgets for between nodes. Nothing
/// consumes the assignments this early: the soonest consumers are the
/// selection-proof flows (deadline 2/3 slot) and the voting-context build
/// (triggered no earlier than a head event).
const VOTING_ASSIGNMENTS_PUBLISH_DELAY: Duration = Duration::from_millis(50);

/// Builds each validator's per-subnet position counts from raw sync committee positions.
fn build_sync_validator_assignments<E, I, P>(
    duties: I,
) -> HashMap<ValidatorIndex, SyncSubnetPositionCounts>
where
    E: EthSpec,
    I: IntoIterator<Item = (ValidatorIndex, P)>,
    P: IntoIterator<Item = u64>,
{
    let mut positions_by_validator = HashMap::<ValidatorIndex, HashSet<u64>>::new();
    for (validator_index, positions) in duties {
        // Sync committee positions are unique indices. Unioning them prevents malformed repeated
        // JSON duty records, including exact duplicate positions, from inflating multiplicity.
        positions_by_validator
            .entry(validator_index)
            .or_default()
            .extend(positions);
    }

    let subcommittee_size = E::SyncSubcommitteeSize::to_u64();
    positions_by_validator
        .into_iter()
        .map(|(validator_index, positions)| {
            let mut position_counts = SyncSubnetPositionCounts::new();
            for position in positions {
                let subnet_id = SyncSubnetId::new(position / subcommittee_size);
                *position_counts.entry(subnet_id).or_default() += 1;
            }
            (validator_index, position_counts)
        })
        .collect()
}

#[derive(Debug)]
struct AttestationScore {
    score: f64,
    base_score: f64,
    distance: Option<u64>,
    bonus: Option<f64>,
}

/// Score attestation data for weighted selection across multiple beacon nodes.
///
/// Implements the same formula as SSV-Go's `scoreAttestationData` for cross-client
/// compatibility (see `beacon/goclient/attest.go`). Inspired by Vouch (Attestant).
///
/// **Base score** = `source.epoch + target.epoch`. Reflects the expected attestation
/// reward: higher checkpoint epochs indicate a more up-to-date view of the chain.
///
/// **Proximity bonus** = `1 / (1 + attestation_slot - head_slot)`. Rewards beacon
/// nodes whose head block is closer to the attestation slot. The bonus is always in
/// (0, 1], so it only acts as a tie-breaker between responses with identical
/// checkpoint epochs — it cannot override a higher base score.
///
/// When the block header lookup fails or times out, only the base score is used.
fn calculate_attestation_score(
    attestation_data: &AttestationData,
    head_slot: Option<Slot>,
) -> AttestationScore {
    let base_score =
        (attestation_data.source.epoch.as_u64() + attestation_data.target.epoch.as_u64()) as f64;

    match head_slot {
        Some(head_slot) => {
            let attestation_slot_u64 = attestation_data.slot.as_u64();
            let head_slot_u64 = head_slot.as_u64();

            if head_slot_u64 <= attestation_slot_u64 {
                let distance = attestation_slot_u64 - head_slot_u64;
                let bonus = 1.0 / (1 + distance) as f64;
                AttestationScore {
                    score: base_score + bonus,
                    base_score,
                    distance: Some(distance),
                    bonus: Some(bonus),
                }
            } else {
                AttestationScore {
                    score: base_score,
                    base_score,
                    distance: None,
                    bonus: None,
                }
            }
        }
        None => AttestationScore {
            score: base_score,
            base_score,
            distance: None,
            bonus: None,
        },
    }
}

/// Build the fork-typed committee vote from one coherent beacon-node response.
fn slot_vote_from_attestation_data<E: EthSpec>(
    spec: &ChainSpec,
    slot: Slot,
    attestation_data: AttestationData,
) -> SlotVote {
    if spec.fork_name_at_slot::<E>(slot).gloas_enabled() {
        SlotVote::Gloas(GloasBeaconVote {
            block_root: attestation_data.beacon_block_root,
            source: attestation_data.source,
            target: attestation_data.target,
            attestation_data_index: attestation_data.index,
        })
    } else {
        SlotVote::Base(BeaconVote {
            block_root: attestation_data.beacon_block_root,
            source: attestation_data.source,
            target: attestation_data.target,
        })
    }
}

/// The head root a same-slot head event fixed for `slot`, consumed by the SIP-94 same-slot
/// index check. `wait_for_head_event` matched the event against an earlier clock read, so the
/// slot is checked again here: an event for the previous slot must not be attached to this
/// slot's context.
fn same_slot_head_root(slot: Slot, head_event: Option<&HeadEvent>) -> Option<Hash256> {
    head_event
        .filter(|event| event.slot == slot)
        .map(|event| event.beacon_block_root)
}

/// Reconstruct the attestation data whose tree root is used for aggregate fetching.
fn aggregate_fetch_attestation_data<E: EthSpec>(
    spec: &ChainSpec,
    slot: Slot,
    vote: &SlotVote,
    committee_index: u64,
) -> AttestationData {
    let fork_name = spec.fork_name_at_slot::<E>(slot);
    AttestationData {
        slot,
        index: if fork_name < ForkName::Electra {
            committee_index
        } else {
            vote.index()
        },
        beacon_block_root: vote.block_root(),
        source: vote.source(),
        target: vote.target(),
    }
}

/// Identify one aggregate-attestation request from its complete Beacon API inputs.
fn aggregate_fetch_key<E: EthSpec>(
    spec: &ChainSpec,
    slot: Slot,
    vote: &SlotVote,
    committee_index: u64,
) -> AggregateFetchKey {
    let attestation_data = aggregate_fetch_attestation_data::<E>(spec, slot, vote, committee_index);
    AggregateFetchKey {
        attestation_data_root: attestation_data.tree_hash_root(),
        committee_index,
    }
}

/// Identify one sync-contribution request from its complete Beacon API inputs.
fn sync_contribution_fetch_key(
    vote: &SlotVote,
    subnet_id: SyncSubnetId,
) -> SyncContributionFetchKey {
    SyncContributionFetchKey {
        block_root: vote.block_root(),
        subnet_id,
    }
}

#[derive(Clone)]
pub struct MetadataService<E: EthSpec, T: SlotClock + 'static> {
    duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>,
    validator_store: Arc<AnchorValidatorStore<T, E>>,
    slot_clock: T,
    beacon_nodes: Arc<BeaconNodeFallback<T>>,
    executor: TaskExecutor,
    spec: Arc<ChainSpec>,
    weighted_attestation_data: bool,
}

/// Wait for a head event matching the current slot reported by `slot_clock`.
///
/// Drops events whose slot doesn't match the current slot and keeps waiting.
/// Resolves to `None` if the channel is closed or `slot_clock.now()` fails.
async fn wait_for_head_event<T: SlotClock>(
    receiver: &mut mpsc::Receiver<HeadEvent>,
    slot_clock: &T,
) -> Option<HeadEvent> {
    loop {
        match receiver.recv().await {
            Some(head_event) => {
                let current_slot = slot_clock.now()?;
                if head_event.slot == current_slot {
                    return Some(head_event);
                }
                // Head event slot doesn't match current; drop and keep waiting.
            }
            None => {
                warn!("Head monitor channel closed unexpectedly");
                return None;
            }
        }
    }
}

/// Drive `publish` once per slot, shortly after each slot boundary.
///
/// The delay past the boundary is what makes this correct: sleeping exactly to
/// the boundary lets a marginally early timer wake read the previous slot,
/// republish it, and skip the new one (issue #1223). See
/// [`VOTING_ASSIGNMENTS_PUBLISH_DELAY`].
async fn run_slot_start_publisher<T: SlotClock>(slot_clock: T, mut publish: impl FnMut()) {
    loop {
        if let Some(duration_to_next_slot) = slot_clock.duration_to_next_slot() {
            sleep(duration_to_next_slot + VOTING_ASSIGNMENTS_PUBLISH_DELAY).await;
            publish();
        } else {
            error!("Failed to read slot clock");
            sleep(slot_clock.slot_duration()).await;
        }
    }
}

/// Drive aggregation publication at the upcoming slot's fork-specific deadline.
pub(super) async fn run_aggregation_publisher<E: EthSpec, T: SlotClock, F: Future<Output = ()>>(
    slot_clock: T,
    spec: Arc<ChainSpec>,
    mut publish: impl FnMut() -> F,
) {
    loop {
        // Sample once so the target slot and remaining delay agree across a slot boundary.
        let delay = slot_clock.now_duration().and_then(|now| {
            let next_slot = slot_clock
                .slot_of(now)
                .map_or_else(|| slot_clock.genesis_slot(), |slot| slot + 1);
            slot_clock
                .start_of(next_slot)?
                .checked_add(spec.get_aggregate_attestation_due::<E>(next_slot))?
                .checked_sub(now)
        });

        if let Some(delay) = delay {
            sleep(delay).await;
            publish().await;
        } else {
            error!("Failed to read slot clock");
            sleep(slot_clock.slot_duration()).await;
        }
    }
}

impl<E: EthSpec, T: SlotClock + 'static> MetadataService<E, T> {
    pub fn new(
        duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>,
        validator_store: Arc<AnchorValidatorStore<T, E>>,
        slot_clock: T,
        beacon_nodes: Arc<BeaconNodeFallback<T>>,
        executor: TaskExecutor,
        spec: Arc<ChainSpec>,
        weighted_attestation_data: bool,
    ) -> Self {
        Self {
            duties_service,
            validator_store,
            slot_clock,
            beacon_nodes,
            executor,
            spec,
            weighted_attestation_data,
        }
    }

    pub fn start_update_service(
        self,
        mut head_monitor_rx: Option<mpsc::Receiver<HeadEvent>>,
    ) -> Result<(), String> {
        let slot_duration = self.spec.get_slot_duration();
        let duration_to_next_slot = self
            .slot_clock
            .duration_to_next_slot()
            .ok_or("Unable to determine duration to next slot")?;

        info!(
            next_update_millis = duration_to_next_slot.as_millis(),
            weighted_attestation_data = self.weighted_attestation_data,
            "Metadata service started"
        );

        let executor = self.executor.clone();

        // ═══════════════════════════════════════════════════════════════════════
        // PHASE 1: VotingAssignments (slot start)
        // Caches voting assignments for use by both selection proofs AND voting context.
        // Reads directly from DutiesService cache which is populated on startup and
        // refreshed each slot.
        // ═══════════════════════════════════════════════════════════════════════
        let self_clone_phase1 = self.clone();
        executor.spawn(
            async move {
                let slot_clock = self_clone_phase1.slot_clock.clone();
                run_slot_start_publisher(slot_clock, move || {
                    if let Err(err) = self_clone_phase1.update_voting_assignments() {
                        error!(err, "Failed to update validator voting assignments");
                    }
                })
                .await
            },
            "voting_assignments_service",
        );

        // ═══════════════════════════════════════════════════════════════════════
        // PHASE 2: VotingContext (head event or spec-derived fallback, first to fire)
        // Gets cached voting assignments, fetches beacon_vote, builds VotingContext.
        // ═══════════════════════════════════════════════════════════════════════
        let self_clone_phase2 = self.clone();
        executor.spawn(
            async move {
                loop {
                    let Some(duration_to_next_slot) =
                        self_clone_phase2.slot_clock.duration_to_next_slot()
                    else {
                        error!("Failed to read slot clock");
                        sleep(slot_duration).await;
                        continue;
                    };

                    // We sleep into the next slot, so look up the deadline for that slot, not
                    // the current one. Otherwise the tighter Gloas deadline would apply one
                    // slot late at the fork boundary.
                    let Some(next_slot) = self_clone_phase2.slot_clock.now().map(|s| s + 1) else {
                        error!("Failed to read slot clock");
                        sleep(slot_duration).await;
                        continue;
                    };

                    // Cross the slot boundary first so we race head events only within
                    // the slot we're about to attest in.
                    sleep(duration_to_next_slot).await;

                    let fallback = self_clone_phase2.spec.get_attestation_due::<E>(next_slot);
                    let head_event = match head_monitor_rx.as_mut() {
                        Some(rx) => tokio::select! {
                            _ = sleep(fallback) => None,
                            event = wait_for_head_event(rx, &self_clone_phase2.slot_clock) => event,
                        },
                        None => {
                            sleep(fallback).await;
                            None
                        }
                    };

                    let trigger = if head_event.is_some() {
                        metrics::TRIGGER_HEAD_EVENT
                    } else {
                        metrics::TRIGGER_TIMER
                    };
                    metrics::inc_counter_vec(
                        &metrics::METADATA_SERVICE_VOTING_CONTEXT_TRIGGERS_TOTAL,
                        &[trigger],
                    );
                    if let Some(offset) = self_clone_phase2
                        .slot_clock
                        .millis_from_current_slot_start()
                    {
                        metrics::observe_timer_vec(
                            &metrics::METADATA_SERVICE_VOTING_CONTEXT_OFFSET_SECONDS,
                            &[trigger],
                            offset,
                        );
                    }

                    if let Err(err) = self_clone_phase2.update_voting_context(head_event).await {
                        error!(err, "Failed to update voting context")
                    }
                }
            },
            "voting_context_service",
        );

        // ═══════════════════════════════════════════════════════════════════════
        // PHASE 3: AggregationAssignments (fork-specific aggregation deadline)
        // Re-fetches `duties_service.attesters()` after selection proofs are computed.
        // At this point, `DutyAndProof.selection_proof.is_some()` accurately indicates
        // `is_aggregator` for attestation duties.
        // ═══════════════════════════════════════════════════════════════════════
        let self_clone_phase3 = self.clone();
        executor.spawn(
            async move {
                run_aggregation_publisher::<E, _, _>(
                    self_clone_phase3.slot_clock.clone(),
                    self_clone_phase3.spec.clone(),
                    || async {
                        if let Err(err) = self_clone_phase3.update_aggregation_assignments().await {
                            error!(err, "Failed to update aggregator voting assignments");
                        }
                    },
                )
                .await;
            },
            "aggregation_assignments_service",
        );

        Ok(())
    }

    /// Phase 1: Build and publish `VotingAssignments` at slot start.
    fn update_voting_assignments(&self) -> Result<(), String> {
        let slot = self.slot_clock.now().ok_or("Failed to read slot clock")?;

        // Get attestation validators
        let (attesting_validators, attesting_committees): (Vec<_>, HashMap<_, _>) = self
            .duties_service
            .attesters(slot)
            .into_iter()
            .map(|duty| {
                (
                    ValidatorIndex(duty.duty.validator_index as usize),
                    (duty.duty.pubkey, duty.duty.committee_index),
                )
            })
            .unzip();

        // Get sync validators by subnet
        let sync_validators_by_subnet = self
            .duties_service
            .sync_duties
            .get_duties_for_slot::<E>(slot, &self.spec)
            .as_ref()
            .map(|sync_duties| {
                build_sync_validator_assignments::<E, _, _>(sync_duties.duties.iter().map(|duty| {
                    (
                        ValidatorIndex(duty.validator_index as usize),
                        duty.validator_sync_committee_indices.iter().copied(),
                    )
                }))
            })
            .unwrap_or_default();

        let attester_count = attesting_validators.len();
        let sync_count = sync_validators_by_subnet.len();

        let voting_assignments = VotingAssignments {
            slot,
            attesting_validators,
            attesting_committees,
            sync_validators_by_subnet,
        };

        self.validator_store
            .update_voting_assignments(voting_assignments);

        // Record validator count metrics
        metrics::set_gauge(
            &metrics::METADATA_SERVICE_ATTESTING_VALIDATORS,
            attester_count as i64,
        );
        metrics::set_gauge(
            &metrics::METADATA_SERVICE_SYNC_VALIDATORS,
            sync_count as i64,
        );
        if attester_count == 0 && sync_count == 0 {
            metrics::inc_counter(&metrics::METADATA_SERVICE_EMPTY_ASSIGNMENTS_TOTAL);
        }

        trace!(%slot, attester_count, sync_count, "Published VotingAssignments at slot start");
        Ok(())
    }

    /// Phase 2: Build and publish `VotingContext`, triggered by a head event or
    /// the spec-derived fallback timer. When `head_event` is `Some`, the firing
    /// BN is queried directly (bypassing WAD); any failure or block-root
    /// mismatch falls back to the WAD/first_success path.
    async fn update_voting_context(&self, head_event: Option<HeadEvent>) -> Result<(), String> {
        let slot = self.slot_clock.now().ok_or("Failed to read slot clock")?;

        let voting_assignments = self
            .validator_store
            .get_voting_assignments(slot)
            .await
            .map_err(|e| format!("Failed to get cached voting assignments: {:?}", e))?;

        let same_slot_head_root = same_slot_head_root(slot, head_event.as_ref());
        let attestation_data = match head_event {
            Some(event) => match self.fetch_attestation_data_from_event(slot, &event).await {
                Some(data) => data,
                None => self.fetch_attestation_data(slot).await?,
            },
            None => self.fetch_attestation_data(slot).await?,
        };

        let vote = slot_vote_from_attestation_data::<E>(&self.spec, slot, attestation_data);

        let voting_context = VotingContext {
            voting_assignments,
            vote,
            same_slot_head_root,
            decided_votes: Default::default(),
        };

        self.validator_store.update_voting_context(voting_context);

        trace!(%slot, "Published VotingContext");
        Ok(())
    }

    /// Eager-attest path: query the BN that fired the head event. Verifies the
    /// returned block_root matches the event's. Returns `None` on any failure
    /// or mismatch so the caller can fall back.
    async fn fetch_attestation_data_from_event(
        &self,
        slot: Slot,
        event: &HeadEvent,
    ) -> Option<AttestationData> {
        match self
            .beacon_nodes
            .run_on_candidate_index(event.beacon_node_index, |beacon_node| async move {
                beacon_node.get_validator_attestation_data(slot, 0).await
            })
            .await
        {
            Ok(response) if response.data.beacon_block_root == event.beacon_block_root => {
                Some(response.data)
            }
            Ok(response) => {
                warn!(
                    expected = ?event.beacon_block_root,
                    got = ?response.data.beacon_block_root,
                    "Head-event BN returned mismatched block root, falling back",
                );
                None
            }
            Err(e) => {
                warn!(error = ?e, "Failed to fetch attestation data from head-event BN, falling back");
                None
            }
        }
    }

    /// Fallback path: WAD if enabled, otherwise first_success across all BNs.
    async fn fetch_attestation_data(&self, slot: Slot) -> Result<AttestationData, String> {
        if self.weighted_attestation_data {
            self.weighted_calculation(slot).await
        } else {
            self.beacon_nodes
                .first_success(|beacon_node| async move {
                    let _timer = validator_metrics::start_timer_vec(
                        &validator_metrics::ATTESTATION_SERVICE_TIMES,
                        &[validator_metrics::ATTESTATIONS_HTTP_GET],
                    );
                    beacon_node
                        .get_validator_attestation_data(slot, 0)
                        .await
                        .map_err(|e| format!("Failed to produce attestation data: {e:?}"))
                        .map(|result| result.data)
                })
                .await
                .map_err(|e| e.to_string())
        }
    }

    /// Phase 3: Build and publish `AggregationAssignments` at the fork-specific aggregation
    /// deadline.
    ///
    /// Uses single-pass data transformation to minimize iterations:
    /// - ONE pass over attesters (those with `selection_proof`) to build all attester-related data
    /// - ONE pass over `sync_aggregators` to build all sync-related data
    /// - Then beacon fetches and consensus data building
    async fn update_aggregation_assignments(&self) -> Result<(), String> {
        let slot = self.slot_clock.now().ok_or("Failed to read slot clock")?;

        // Get selection proofs from `duties_service`
        let attesters = self.duties_service.attesters(slot);
        let sync_duties = self
            .duties_service
            .sync_duties
            .get_duties_for_slot::<E>(slot, &self.spec);

        // ═══════════════════════════════════════════════════════════════════════
        // SINGLE PASS over attesters with `selection_proof`
        // Only processes validators with valid, non-liquidated SSV committees.
        // Collects: `aggregator_committees`, `attesters_by_ssv_committee`
        // ═══════════════════════════════════════════════════════════════════════
        let mut aggregator_committees: HashMap<PublicKeyBytes, u64> =
            HashMap::with_capacity(attesters.len());
        let mut attesters_by_ssv_committee: HashMap<CommitteeId, Vec<&DutyAndProof>> =
            HashMap::new();
        for attester in attesters.iter().filter(|d| d.selection_proof.is_some()) {
            // Only process validators with valid, non-liquidated SSV committees
            if let Some(ssv_committee_id) = self
                .validator_store
                .get_validator_and_cluster(attester.duty.pubkey)
                .ok()
                .map(|(_, cluster)| cluster.committee_id())
            {
                // For AggregationAssignments output
                aggregator_committees.insert(attester.duty.pubkey, attester.duty.committee_index);

                // For consensus data building - group by SSV committee
                attesters_by_ssv_committee
                    .entry(ssv_committee_id)
                    .or_default()
                    .push(attester);
            }
        }

        // ═══════════════════════════════════════════════════════════════════════
        // SINGLE PASS over `sync_aggregators`
        // Only processes validators with valid, non-liquidated SSV committees.
        // Collects: `validator_subnet_counts` (for multi_sync), `sync_by_ssv_committee`
        // ═══════════════════════════════════════════════════════════════════════
        let sync_aggregators = sync_duties.as_ref().map(|duties| &duties.aggregators);

        let mut validator_subnet_counts: HashMap<PublicKeyBytes, usize> = HashMap::new();
        let mut sync_by_ssv_committee: SyncByCommitteeMap = HashMap::new();
        if let Some(aggregators) = sync_aggregators {
            for (subnet_id, subnet_aggregators) in aggregators {
                for (validator_index, pubkey, selection_proof) in subnet_aggregators {
                    let sync_aggregator = SyncAggregatorData {
                        validator_index: *validator_index,
                        pubkey: *pubkey,
                        selection_proof: selection_proof.clone(),
                    };

                    // Only process validators with valid, non-liquidated SSV committees
                    if let Some(ssv_committee_id) = self
                        .validator_store
                        .get_validator_and_cluster(sync_aggregator.pubkey)
                        .ok()
                        .map(|(_, cluster)| cluster.committee_id())
                    {
                        // For AggregationAssignments output
                        *validator_subnet_counts
                            .entry(sync_aggregator.pubkey)
                            .or_insert(0) += 1;

                        // For consensus data building - group by SSV committee
                        sync_by_ssv_committee
                            .entry(ssv_committee_id)
                            .or_default()
                            .push((*subnet_id, sync_aggregator));
                    }
                }
            }
        }

        // Derive multi_sync_aggregators from validator_subnet_counts
        // (validators aggregating on multiple subnets need coordination)
        let multi_sync_aggregators: HashMap<PublicKeyBytes, ContributionWaiter<E>> =
            validator_subnet_counts
                .into_iter()
                .filter(|(_, count)| *count > 1)
                .map(|(pubkey, count)| (pubkey, ContributionWaiter::new(count)))
                .collect();

        // ═══════════════════════════════════════════════════════════════════════
        // Build consensus data for Boole+ forks
        // ═══════════════════════════════════════════════════════════════════════
        let epoch = slot.epoch(E::slots_per_epoch());

        // The exact complement of the Lighthouse callback gates by construction: consensus data
        // (and therefore the publisher) exists precisely when Lighthouse does not own
        // publication.
        let consensus_data_by_ssv_committee =
            if self.validator_store.lighthouse_owns_publication(epoch) {
                HashMap::new()
            } else {
                self.build_consensus_data_for_all_committees(
                    slot,
                    attesters_by_ssv_committee,
                    sync_by_ssv_committee,
                )
                .await?
            };

        let aggregator_info = AggregationAssignments {
            slot,
            aggregator_committees,
            multi_sync_aggregators,
            consensus_data_by_ssv_committee,
        };

        let new_executions = self
            .validator_store
            .update_aggregation_assignments(aggregator_info);
        self.spawn_aggregate_publisher(slot, new_executions);

        trace!(%slot, "Published AggregationAssignments");
        Ok(())
    }

    /// Resolve the effective vote for every SSV committee with aggregation duties, then derive the
    /// Beacon API request keys required by those duties.
    ///
    /// A committee uses its decided vote when available and otherwise falls back to the slot seed.
    /// Identical request keys are deduplicated across committees.
    fn resolve_committee_requests(
        spec: &ChainSpec,
        slot: Slot,
        voting_context: &VotingContext,
        attesters_by_ssv_committee: &HashMap<CommitteeId, Vec<&DutyAndProof>>,
        sync_by_ssv_committee: &SyncByCommitteeMap,
    ) -> ResolvedCommitteeRequests {
        let ssv_committees: HashSet<CommitteeId> = attesters_by_ssv_committee
            .keys()
            .chain(sync_by_ssv_committee.keys())
            .copied()
            .collect();
        let mut votes = HashMap::with_capacity(ssv_committees.len());
        let mut aggregate_attestation_keys = HashSet::new();
        let mut sync_contribution_keys = HashSet::new();

        for ssv_committee_id in &ssv_committees {
            let vote = voting_context.vote_for_committee(ssv_committee_id);

            if let Some(attesters) = attesters_by_ssv_committee.get(ssv_committee_id) {
                aggregate_attestation_keys.extend(attesters.iter().map(|attester| {
                    aggregate_fetch_key::<E>(spec, slot, &vote, attester.duty.committee_index)
                }));
            }

            if let Some(sync_entries) = sync_by_ssv_committee.get(ssv_committee_id) {
                sync_contribution_keys.extend(
                    sync_entries
                        .iter()
                        .map(|(subnet_id, _)| sync_contribution_fetch_key(&vote, *subnet_id)),
                );
            }

            votes.insert(*ssv_committee_id, vote);
        }

        ResolvedCommitteeRequests {
            votes,
            aggregate_attestation_keys,
            sync_contribution_keys,
        }
    }

    /// Spawn the detached publisher for this slot's newly registered post-consensus executions.
    ///
    /// The publisher owns Boole+ publication of aggregates and sync contributions: the Lighthouse
    /// callbacks return empty batches at Boole+ because their duty snapshots are cloned before
    /// slot-start selection proofs finish, so roots the snapshots never saw would otherwise be
    /// silently dropped. (Anchor's sync selection proofs are NOT precomputed a slot ahead: its
    /// one-slot Lighthouse lookahead config starts slot N's proof signing at slot N's boundary,
    /// in the same slot-start partial-signature batch as the attestation proofs.)
    ///
    /// Exactly-once: `new_executions` holds only vacant registrations, so a repeated Phase 3 run
    /// for one slot cannot spawn a second publisher for the same `(committee, slot)`.
    ///
    /// The publisher is implicitly Boole-gated (consensus data exists only for Boole+ slots),
    /// while the empty Lighthouse callbacks gate on the duty's epoch. The two agree because
    /// `attestation.data.target.epoch` and `contribution.slot`'s epoch equal the slot's own epoch
    /// by construction, which is what rules out both paths publishing for one duty.
    fn spawn_aggregate_publisher(
        &self,
        slot: Slot,
        new_executions: Vec<(CommitteeId, AggregatorPostConsensusShared<E>)>,
    ) {
        if new_executions.is_empty() {
            return;
        }

        let validator_store = self.validator_store.clone();
        let beacon_nodes = self.beacon_nodes.clone();

        self.executor.spawn(
            async move {
                validator_store
                    .publish_decided_aggregates(
                        slot,
                        new_executions,
                        |fork_name, signed| post_aggregate(&beacon_nodes, fork_name, signed),
                        |signed| post_contribution(&beacon_nodes, signed),
                    )
                    .await;
            },
            "aggregator_committee_publisher",
        );
    }

    /// Build `AggregatorCommitteeConsensusData` for each committee that has aggregators.
    ///
    /// Takes pre-grouped data from `update_aggregation_assignments` to avoid redundant iteration.
    async fn build_consensus_data_for_all_committees(
        &self,
        slot: Slot,
        attesters_by_ssv_committee: HashMap<CommitteeId, Vec<&DutyAndProof>>,
        sync_by_ssv_committee: SyncByCommitteeMap,
    ) -> Result<HashMap<CommitteeId, Arc<AggregatorCommitteeConsensusData<E>>>, String> {
        // Get `VotingContext` for the slot's `vote` (cached at 1/3 slot)
        let voting_context = self
            .validator_store
            .get_voting_context(slot)
            .await
            .map_err(|e| format!("Failed to get voting context: {:?}", e))?;

        let committee_requests = Self::resolve_committee_requests(
            &self.spec,
            slot,
            &voting_context,
            &attesters_by_ssv_committee,
            &sync_by_ssv_committee,
        );

        // Fetch both categories concurrently. Each helper keeps the existing partial-result
        // behavior and one two-second deadline for all unique requests in its category.
        //
        // Both arms are boxed to keep the `Send` proof for the spawned phase-3 task shallow. The
        // trait solver otherwise walks the whole chain from `executor.spawn` down through
        // `tokio::join!`'s `MaybeDone` nesting into these two futures, which exceeds the default
        // recursion limit. `BoxFuture` is unconditionally `Send`, so the walk stops here. See
        // https://github.com/rust-lang/rust/issues/159228.
        let (aggregated_attestations, sync_contributions) = tokio::join!(
            self.fetch_aggregated_attestations(
                slot,
                &committee_requests.aggregate_attestation_keys,
                BEACON_API_FETCH_TIMEOUT,
            )
            .boxed(),
            self.fetch_sync_contributions(
                slot,
                &committee_requests.sync_contribution_keys,
                BEACON_API_FETCH_TIMEOUT,
            )
            .boxed(),
        );
        let fetch_results = AggregationFetchResults {
            aggregated_attestations,
            sync_contributions,
        };

        let mut result = HashMap::with_capacity(committee_requests.votes.len());
        for (ssv_committee_id, vote) in &committee_requests.votes {
            let consensus_data = self.build_consensus_data_for_committee(
                slot,
                ssv_committee_id,
                attesters_by_ssv_committee.get(ssv_committee_id),
                sync_by_ssv_committee.get(ssv_committee_id),
                vote,
                &fetch_results,
            )?;

            if let Some(data) = consensus_data {
                result.insert(*ssv_committee_id, Arc::new(data));
            }
        }

        Ok(result)
    }

    /// Build `AggregatorCommitteeConsensusData` for a single committee.
    ///
    /// CRITICAL REQUIREMENTS (must match SSV Go/Spec exactly for consensus):
    /// 1. Aggregators: Call `is_aggregator()` on the selection proof before including
    /// 2. Contributors: Call `is_sync_committee_aggregator()` on the selection proof before
    ///    including
    /// 3. Aggregators: Sort by `validator_index` (all share same signing root)
    /// 4. Contributors: Sort by (`signing_root`, `validator_index`) to match SSV Go's root-sorted
    ///    processing
    /// 5. Committee indexes: First-seen order from sorted aggregators (not sorted separately)
    /// 6. Subnet IDs: First-seen order from sorted contributors (not sorted separately)
    fn build_consensus_data_for_committee(
        &self,
        slot: Slot,
        ssv_committee_id: &CommitteeId,
        ssv_committee_attesters: Option<&Vec<&DutyAndProof>>,
        ssv_committee_sync: Option<&Vec<(SyncSubnetId, SyncAggregatorData)>>,
        vote: &SlotVote,
        fetch_results: &AggregationFetchResults<E>,
    ) -> Result<Option<AggregatorCommitteeConsensusData<E>>, String> {
        // === AGGREGATORS ===
        // Process pre-filtered attesters for this committee
        // These validators are already confirmed to be in this committee
        let mut aggregators: Vec<AssignedAggregator> = match ssv_committee_attesters {
            Some(attesters) => Vec::with_capacity(attesters.len()),
            None => Vec::new(),
        };
        if let Some(attesters) = ssv_committee_attesters {
            for duty_and_proof in attesters.iter() {
                let validator_index = ValidatorIndex(duty_and_proof.duty.validator_index as usize);

                // Verify the aggregated selection proof passes beacon node is_aggregator check.
                // Without this check, validators whose proofs don't meet the modulo threshold
                // would be included, causing consensus hash mismatch with other operators.
                // NOTE: Since we don't have direct beacon node access to is_aggregator() in
                // Anchor's setup, we rely on the fact that `selection_proof.is_some()` means
                // Lighthouse has already verified this validator is an aggregator.
                // Defensive: this should never happen since `update_aggregation_assignments`
                // filters with `.filter(|d| d.selection_proof.is_some())` before grouping.
                let Some(selection_proof) = duty_and_proof.selection_proof.clone() else {
                    warn!(
                        %slot,
                        ?ssv_committee_id,
                        ?validator_index,
                        "[AggregatorCommittee] BUG: aggregator missing selection_proof despite upstream filter"
                    );
                    continue;
                };
                aggregators.push(AssignedAggregator {
                    validator_index,
                    selection_proof: selection_proof.into(),
                    committee_index: duty_and_proof.duty.committee_index,
                });
            }
        }

        // === CONTRIBUTORS ===
        // Process pre-filtered sync aggregators for this committee
        // These validators are already confirmed to be in this committee
        let mut contributors_with_roots: Vec<(Hash256, AssignedAggregator)> =
            match ssv_committee_sync {
                Some(sync_entries) => Vec::with_capacity(sync_entries.len()),
                None => Vec::new(),
            };
        if let Some(sync_entries) = ssv_committee_sync {
            for (subnet_id, sync_aggregator) in sync_entries.iter() {
                // Compute signing root for this subnet
                let sync_selection_root = self
                    .validator_store
                    .compute_sync_selection_root(slot, (*subnet_id).into());

                let validator_index = ValidatorIndex(sync_aggregator.validator_index as usize);

                // Lighthouse duties service already filters sync duties to only include valid
                // aggregators (those whose proofs meet the modulo threshold), so we can proceed.
                contributors_with_roots.push((
                    sync_selection_root,
                    AssignedAggregator {
                        validator_index,
                        selection_proof: sync_aggregator.selection_proof.clone().into(),
                        committee_index: (*subnet_id).into(),
                    },
                ));
            }
        }

        build_consensus_data_from_candidates::<E>(
            &self.spec,
            slot,
            vote,
            aggregators,
            contributors_with_roots,
            fetch_results,
        )
    }

    /// Fetch aggregated attestations from beacon nodes for the given request identities.
    ///
    /// Uses `FuturesUnordered` to collect results as they complete. When the timeout is reached,
    /// returns whatever results have been collected so far (partial results). This ensures
    /// we don't block on slow beacon nodes while still using successful fetches.
    async fn fetch_aggregated_attestations(
        &self,
        slot: Slot,
        requests: &HashSet<AggregateFetchKey>,
        timeout: Duration,
    ) -> HashMap<AggregateFetchKey, Attestation<E>> {
        let _timer = metrics::start_timer_vec(
            &metrics::AGGREGATOR_COMMITTEE_FETCH_TIMES,
            &["aggregated_attestations"],
        );

        // Create `FuturesUnordered` for concurrent execution with partial result collection
        let beacon_nodes = &self.beacon_nodes;
        let mut futures: FuturesUnordered<_> = requests
            .iter()
            .map(|&request| {
                async move {
                    let result = beacon_nodes
                        .first_success(|beacon_node| async move {
                            let _timer = validator_metrics::start_timer_vec(
                                &validator_metrics::ATTESTATION_SERVICE_TIMES,
                                &[validator_metrics::AGGREGATES_HTTP_GET],
                            );
                            beacon_node
                                .get_validator_aggregate_attestation_v2(
                                    slot,
                                    request.attestation_data_root,
                                    request.committee_index,
                                )
                                .await
                                .map_err(|e| {
                                    format!("[AggregatorCommittee] Failed to produce aggregate attestation: {:?}", e)
                                })?
                                .ok_or_else(|| {
                                    format!(
                                        "[AggregatorCommittee] No aggregate available for slot {}, committee {}",
                                        slot, request.committee_index
                                    )
                                })
                                .map(|result| result.into_data())
                        })
                        .await;
                    (request, result)
                }
            })
            .collect();

        let total_requests = requests.len();
        let mut aggregated_attestations = HashMap::with_capacity(total_requests);
        let deadline = Instant::now() + timeout;

        // Collect results as they complete, until timeout or all done
        loop {
            // Exit when all futures completed
            if futures.is_empty() {
                break;
            }

            tokio::select! {
                Some((request, result)) = futures.next() => {
                    match result {
                        Ok(attestation) => {
                            aggregated_attestations.insert(request, attestation);
                        }
                        Err(e) => {
                            warn!(
                                %slot,
                                committee_index = request.committee_index,
                                attestation_data_root = ?request.attestation_data_root,
                                error = %e,
                                "[AggregatorCommittee] Failed to fetch aggregated attestation for committee"
                            );
                        }
                    }
                }
                _ = sleep_until(deadline) => {
                    if aggregated_attestations.len() < total_requests {
                        warn!(
                            %slot,
                            collected = aggregated_attestations.len(),
                            total = total_requests,
                            "[AggregatorCommittee] Timeout fetching aggregated attestations, returning partial results"
                        );
                        metrics::inc_counter_vec(
                            &metrics::AGGREGATOR_COMMITTEE_PARTIAL_RESULTS,
                            &["aggregated_attestations"],
                        );
                    }
                    break;
                }
            }
        }

        // Track success/failure counts
        let successful = aggregated_attestations.len();
        let total = total_requests;
        let failed = total.saturating_sub(successful);
        metrics::inc_counter_vec_by(
            &metrics::AGGREGATOR_COMMITTEE_FETCH_SUCCESS,
            &["aggregated_attestations", "success"],
            successful as u64,
        );
        metrics::inc_counter_vec_by(
            &metrics::AGGREGATOR_COMMITTEE_FETCH_SUCCESS,
            &["aggregated_attestations", "failed"],
            failed as u64,
        );

        aggregated_attestations
    }

    /// Fetch sync committee contributions from beacon nodes for the given request identities.
    ///
    /// Uses `FuturesUnordered` to collect results as they complete. When the timeout is reached,
    /// returns whatever results have been collected so far (partial results). This ensures
    /// we don't block on slow beacon nodes while still using successful fetches.
    async fn fetch_sync_contributions(
        &self,
        slot: Slot,
        requests: &HashSet<SyncContributionFetchKey>,
        timeout: Duration,
    ) -> HashMap<SyncContributionFetchKey, SyncCommitteeContribution<E>> {
        let _timer = metrics::start_timer_vec(
            &metrics::AGGREGATOR_COMMITTEE_FETCH_TIMES,
            &["sync_contributions"],
        );
        // Create `FuturesUnordered` for concurrent execution with partial result collection
        let beacon_nodes = &self.beacon_nodes;
        let mut futures: FuturesUnordered<_> = requests
            .iter()
            .map(|&request| async move {
                let result = beacon_nodes
                    .first_success(|beacon_node| async move {
                        let sync_contribution_data = SyncContributionData {
                            slot,
                            beacon_block_root: request.block_root,
                            subcommittee_index: request.subnet_id.into(),
                        };

                        beacon_node
                            .get_validator_sync_committee_contribution(&sync_contribution_data)
                            .await
                    })
                    .instrument(info_span!("fetch_sync_contribution"))
                    .await;
                (request, result)
            })
            .collect();

        let total_requests = requests.len();
        let mut sync_contributions = HashMap::with_capacity(total_requests);
        let deadline = Instant::now() + timeout;

        // Collect results as they complete, until timeout or all done
        loop {
            // Exit when all futures completed
            if futures.is_empty() {
                break;
            }

            tokio::select! {
                Some((request, result)) = futures.next() => {
                    match result {
                        Ok(Some(response)) => {
                            sync_contributions.insert(request, response.data);
                        }
                        Ok(None) => {
                            warn!(
                                %slot,
                                beacon_block_root = ?request.block_root,
                                subnet_id = ?request.subnet_id,
                                "[AggregatorCommittee] No sync contribution found for subnet"
                            );
                        }
                        Err(e) => {
                            error!(
                                %slot,
                                beacon_block_root = ?request.block_root,
                                subnet_id = ?request.subnet_id,
                                error = %e,
                                "[AggregatorCommittee] Failed to fetch sync contribution for subnet"
                            );
                        }
                    }
                }
                _ = sleep_until(deadline) => {
                    if sync_contributions.len() < total_requests {
                        warn!(
                            %slot,
                            collected = sync_contributions.len(),
                            total = total_requests,
                            "[AggregatorCommittee] Timeout fetching sync contributions, returning partial results"
                        );
                        metrics::inc_counter_vec(
                            &metrics::AGGREGATOR_COMMITTEE_PARTIAL_RESULTS,
                            &["sync_contributions"],
                        );
                    }
                    break;
                }
            }
        }

        // Track success/failure counts
        let successful = sync_contributions.len();
        let total = total_requests;
        let failed = total.saturating_sub(successful);
        metrics::inc_counter_vec_by(
            &metrics::AGGREGATOR_COMMITTEE_FETCH_SUCCESS,
            &["sync_contributions", "success"],
            successful as u64,
        );
        metrics::inc_counter_vec_by(
            &metrics::AGGREGATOR_COMMITTEE_FETCH_SUCCESS,
            &["sync_contributions", "failed"],
            failed as u64,
        );

        sync_contributions
    }

    /// Query all beacon nodes in parallel and select the best attestation data by score.
    async fn weighted_calculation(&self, slot: Slot) -> Result<AttestationData, String> {
        let _timer = metrics::start_timer(&metrics::WAD_FETCH_TIMES);
        let started = Instant::now();

        let clients: Vec<(String, BeaconNodeHttpClient)> = {
            let candidates = self.beacon_nodes.candidates.read().await;
            candidates
                .iter()
                .map(|c| (c.beacon_node.to_string(), c.beacon_node.clone()))
                .collect()
        };

        let num_clients = clients.len();
        debug!(num_clients, "Starting weighted attestation data fetch");

        let mut futures: FuturesUnordered<_> = clients
            .into_iter()
            .map(|(addr, client)| async move {
                let result = Self::fetch_and_score(&client, slot).await;
                (addr, result)
            })
            .collect();

        let mut succeeded = 0;
        let mut failed = 0;
        let mut best_data: Option<ScoredAttestationData> = None;
        let mut soft_timeout_hit = false;

        let soft_timeout = sleep(WAD_SOFT_TIMEOUT);
        tokio::pin!(soft_timeout);

        let hard_timeout = sleep(WAD_HARD_TIMEOUT);
        tokio::pin!(hard_timeout);

        loop {
            tokio::select! {
                biased;

                Some((addr, result)) = futures.next() => {
                    match result {
                        Ok(scored) => {
                            succeeded += 1;
                            trace!(
                                elapsed_ms = started.elapsed().as_millis(),
                                client = %scored.client_addr,
                                score = scored.score,
                                "Attestation data received"
                            );

                            best_data = Some(match best_data {
                                Some(current) if current.score >= scored.score => current,
                                _ => {
                                    debug!(client = %scored.client_addr, score = scored.score, "New best");
                                    scored
                                }
                            });
                        }
                        Err(e) => {
                            failed += 1;
                            warn!(client = %addr, error = %e, "Failed to fetch attestation data");
                        }
                    }

                    if succeeded + failed == num_clients {
                        break;
                    }
                }

                () = &mut soft_timeout, if !soft_timeout_hit => {
                    soft_timeout_hit = true;
                    debug!(
                        elapsed_ms = started.elapsed().as_millis(),
                        succeeded, failed,
                        pending = num_clients - succeeded - failed,
                        "Soft timeout reached"
                    );
                    if best_data.is_some() {
                        metrics::inc_counter(&metrics::WAD_SOFT_TIMEOUT_TOTAL);
                        break;
                    }
                }

                () = &mut hard_timeout => {
                    error!(
                        elapsed_ms = started.elapsed().as_millis(),
                        succeeded, failed,
                        timed_out = num_clients - succeeded - failed,
                        "Hard timeout reached"
                    );
                    break;
                }

                else => break,
            }
        }

        match best_data {
            Some(scored) => {
                debug!(
                    elapsed_ms = started.elapsed().as_millis(),
                    client = %scored.client_addr,
                    score = scored.score,
                    succeeded, failed,
                    "Selected best attestation data"
                );
                Ok(scored.attestation_data)
            }
            None => Err(format!(
                "No attestation data from {} beacon nodes (succeeded: {}, failed: {})",
                num_clients, succeeded, failed
            )),
        }
    }

    async fn fetch_and_score(
        client: &BeaconNodeHttpClient,
        slot: Slot,
    ) -> Result<ScoredAttestationData, String> {
        let client_addr = client.to_string();

        let attestation_data = {
            let _timer = validator_metrics::start_timer_vec(
                &validator_metrics::ATTESTATION_SERVICE_TIMES,
                &[validator_metrics::ATTESTATIONS_HTTP_GET],
            );
            client
                .get_validator_attestation_data(slot, 0)
                .await
                .map_err(|e| format!("{client_addr}: {e:?}"))?
                .data
        };

        let head_slot = Self::get_block_slot(client, attestation_data.beacon_block_root).await;
        let score = calculate_attestation_score(&attestation_data, head_slot);

        trace!(
            client = %client_addr,
            ?head_slot,
            base_score = score.base_score,
            ?score.distance,
            ?score.bonus,
            total_score = score.score,
            "Scored attestation data"
        );

        Ok(ScoredAttestationData {
            client_addr,
            attestation_data,
            score: score.score,
        })
    }

    async fn get_block_slot(client: &BeaconNodeHttpClient, block_root: Hash256) -> Option<Slot> {
        match tokio::time::timeout(BLOCK_SLOT_LOOKUP_TIMEOUT, async {
            client
                .get_beacon_headers_block_id(BlockId::Root(block_root))
                .await
        })
        .await
        {
            Ok(Ok(Some(resp))) => Some(resp.data.header.message.slot),
            Ok(Ok(None)) => {
                debug!(
                    client = %client,
                    ?block_root,
                    "Block header not found for root"
                );
                None
            }
            Ok(Err(e)) => {
                debug!(
                    client = %client,
                    ?block_root,
                    error = %e,
                    "Failed to fetch block header"
                );
                None
            }
            Err(_) => {
                debug!(
                    client = %client,
                    ?block_root,
                    "Block header lookup timed out"
                );
                None
            }
        }
    }
}

#[derive(Debug)]
struct ScoredAttestationData {
    client_addr: String,
    attestation_data: AttestationData,
    score: f64,
}

// ═══════════════════════════════════════════════════════════════════════════════════════
// Pure Helper Functions for Consensus Data Building
// ═══════════════════════════════════════════════════════════════════════════════════════
//
// These functions are extracted from `build_consensus_data_for_committee` to enable
// unit testing of the sorting and filtering logic without requiring beacon node mocks.
// The sorting order MUST match SSV-Go exactly for consensus compatibility.

/// Assemble consensus data deterministically from selected candidates and completed fetches.
///
/// For the same candidates and successfully fetched request identities, this preserves go-ssv's
/// ordering and object association. Beacon API retry scheduling is deliberately outside this
/// function, since network outcomes are not part of the encoded consensus-data contract.
fn build_consensus_data_from_candidates<E: EthSpec>(
    spec: &ChainSpec,
    slot: Slot,
    vote: &SlotVote,
    mut aggregators: Vec<AssignedAggregator>,
    mut contributors_with_roots: Vec<(Hash256, AssignedAggregator)>,
    fetch_results: &AggregationFetchResults<E>,
) -> Result<Option<AggregatorCommitteeConsensusData<E>>, String> {
    sort_aggregators_by_validator_index(&mut aggregators);
    aggregators.retain(|aggregator| {
        let key = aggregate_fetch_key::<E>(spec, slot, vote, aggregator.committee_index);
        fetch_results.aggregated_attestations.contains_key(&key)
    });

    sort_contributors_by_signing_root_then_validator_index(&mut contributors_with_roots);
    contributors_with_roots.retain(|(_, contributor)| {
        let subnet_id = SyncSubnetId::new(contributor.committee_index);
        let key = sync_contribution_fetch_key(vote, subnet_id);
        fetch_results.sync_contributions.contains_key(&key)
    });

    let contributors: Vec<AssignedAggregator> = contributors_with_roots
        .into_iter()
        .map(|(_, contributor)| contributor)
        .collect();

    if aggregators.is_empty() && contributors.is_empty() {
        return Ok(None);
    }

    // Preserve first-seen order from the sorted candidates, matching go-ssv's append order.
    let attestation_committee_indexes: IndexSet<u64> =
        aggregators.iter().map(|a| a.committee_index).collect();
    let attestations_bytes: Vec<VariableList<u8, _>> = attestation_committee_indexes
        .iter()
        .filter_map(|committee_index| {
            let key = aggregate_fetch_key::<E>(spec, slot, vote, *committee_index);
            fetch_results.aggregated_attestations.get(&key)
        })
        .map(|attestation| {
            let bytes = attestation.as_ssz_bytes();
            VariableList::new(bytes).map_err(|e| {
                warn!("Failed to create attestation bytes list: {:?}", e);
                e
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to create attestation bytes: {e:?}"))?;

    let subnet_ids: IndexSet<SyncSubnetId> = contributors
        .iter()
        .map(|c| SyncSubnetId::new(c.committee_index))
        .collect();
    let contributions: Vec<SyncCommitteeContribution<E>> = subnet_ids
        .iter()
        .filter_map(|id| {
            let key = sync_contribution_fetch_key(vote, *id);
            fetch_results.sync_contributions.get(&key).cloned()
        })
        .collect();

    let epoch = slot.epoch(E::slots_per_epoch());
    let version = DataVersion::from(spec.fork_name_at_epoch(epoch));

    Ok(Some(AggregatorCommitteeConsensusData {
        version,
        aggregators: aggregators
            .try_into()
            .map_err(|e| format!("aggregators: {e:?}"))?,
        aggregator_committee_indexes: attestation_committee_indexes
            .into_iter()
            .collect::<Vec<_>>()
            .try_into()
            .map_err(|e| format!("aggregator_committee_indexes: {e:?}"))?,
        aggregated_attestations: attestations_bytes
            .try_into()
            .map_err(|e| format!("aggregated_attestations: {e:?}"))?,
        contributors: contributors
            .try_into()
            .map_err(|e| format!("contributors: {e:?}"))?,
        sync_committee_contributions: contributions
            .try_into()
            .map_err(|e| format!("sync_committee_contributions: {e:?}"))?,
    }))
}

/// Sort aggregators by `validator_index` ascending.
///
/// CRITICAL for consensus: All aggregators share the same signing root (attestation data),
/// so we sort purely by `validator_index`. This matches SSV-Go's behavior.
pub fn sort_aggregators_by_validator_index(aggregators: &mut [AssignedAggregator]) {
    aggregators.sort_unstable_by_key(|a| a.validator_index.0);
}

/// Sort contributors by (`signing_root`, `validator_index`).
///
/// CRITICAL for consensus: Contributors may have different signing roots (different subnets),
/// so we sort first by signing root, then by `validator_index` within each root group.
/// This matches SSV-Go's root-sorted processing.
pub fn sort_contributors_by_signing_root_then_validator_index(
    contributors_with_roots: &mut [(Hash256, AssignedAggregator)],
) {
    contributors_with_roots.sort_unstable_by(|(root_a, contrib_a), (root_b, contrib_b)| {
        root_a
            .cmp(root_b)
            .then_with(|| contrib_a.validator_index.cmp(&contrib_b.validator_index))
    });
}

/// POST one signed aggregate, matching go-ssv's publication shape at the reference pin (one
/// aggregate per request, published as soon as its quorum lands). Endpoint policy mirrors
/// Lighthouse's at the pin: `first_success` across the beacon nodes (which makes two passes
/// over the candidate list before giving up), v2 with the fork header for Electra+ aggregates,
/// v1 otherwise.
///
/// `fork_name` is the fork the aggregate's payload was decoded under (the decided value's
/// `DataVersion`), so the endpoint and fork header cannot diverge from the payload variant.
async fn post_aggregate<T: SlotClock + 'static, E: EthSpec>(
    beacon_nodes: &BeaconNodeFallback<T>,
    fork_name: ForkName,
    signed: SignedAggregateAndProof<E>,
) -> Result<(), String> {
    beacon_nodes
        .first_success(|beacon_node| {
            let signed = std::slice::from_ref(&signed);
            async move {
                let _timer = validator_metrics::start_timer_vec(
                    &validator_metrics::ATTESTATION_SERVICE_TIMES,
                    &[validator_metrics::AGGREGATES_HTTP_POST],
                );
                if fork_name.electra_enabled() {
                    beacon_node
                        .post_validator_aggregate_and_proof_v2(signed, fork_name)
                        .await
                } else {
                    beacon_node
                        .post_validator_aggregate_and_proof_v1(signed)
                        .await
                }
            }
        })
        .await
        .map_err(|e| format!("{e}"))
}

/// POST one signed sync contribution, matching go-ssv's publication shape at the reference pin
/// (one contribution per request, published as soon as its quorum lands). Endpoint policy mirrors
/// Lighthouse's at the pin: `first_success` across the beacon nodes; the endpoint takes no fork
/// header.
async fn post_contribution<T: SlotClock + 'static, E: EthSpec>(
    beacon_nodes: &BeaconNodeFallback<T>,
    signed: SignedContributionAndProof<E>,
) -> Result<(), String> {
    beacon_nodes
        .first_success(|beacon_node| {
            let signed = std::slice::from_ref(&signed);
            async move {
                beacon_node
                    .post_validator_contribution_and_proofs(signed)
                    .await
            }
        })
        .await
        .map_err(|e| format!("{e}"))
}

#[cfg(test)]
mod tests {
    use bls::{AggregateSignature, FixedBytesExtended, Signature};
    use eth2::types::AttesterData;
    use ssv_types::{
        IndexSet, VariableList,
        consensus::{
            AggregatorCommitteeConsensusData, AggregatorCommitteeDataValidator, AssignedAggregator,
            DataVersion, MaxAggregatedAttestationBytes, QbftDataValidator,
        },
    };
    use ssz::{Encode, ProgressiveBitList};
    use ssz_types::{BitList, BitVector};
    use types::{
        AttestationBase, AttestationData, AttestationGloas, Checkpoint, Epoch, ForkName,
        MainnetEthSpec, SelectionProof, Slot, SyncCommitteeContribution,
    };

    use super::*;

    // ═══════════════════════════════════════════════════════════════════════════════════
    // Test Helpers
    // ═══════════════════════════════════════════════════════════════════════════════════

    #[test]
    fn sync_assignments_union_repeated_duties_and_deduplicate_positions() {
        let validator = ValidatorIndex(42);
        let empty_validator = ValidatorIndex(43);
        let assignments = build_sync_validator_assignments::<MainnetEthSpec, _, _>([
            (validator, vec![0, 1]),
            (validator, vec![1, 128]),
            (empty_validator, vec![]),
        ]);

        assert_eq!(
            assignments[&validator],
            HashMap::from([(SyncSubnetId::new(0), 2), (SyncSubnetId::new(1), 1)])
        );
        assert!(
            assignments[&empty_validator].is_empty(),
            "a present empty duty must remain distinguishable from a missing validator"
        );
    }

    /// Create a test AssignedAggregator with specified validator_index and committee_index
    fn create_aggregator(validator_index: usize, committee_index: u64) -> AssignedAggregator {
        AssignedAggregator {
            validator_index: ValidatorIndex(validator_index),
            selection_proof: Signature::empty(),
            committee_index,
        }
    }

    /// Create a contributor with signing root for testing
    fn create_contributor_with_root(
        signing_root: Hash256,
        validator_index: usize,
        subnet_id: u64,
    ) -> (Hash256, AssignedAggregator) {
        (
            signing_root,
            AssignedAggregator {
                validator_index: ValidatorIndex(validator_index),
                selection_proof: Signature::empty(),
                committee_index: subnet_id,
            },
        )
    }

    /// Create test attestation bytes for consensus data
    fn create_attestation_bytes(index: u64) -> VariableList<u8, MaxAggregatedAttestationBytes> {
        let attestation = AttestationBase::<MainnetEthSpec> {
            aggregation_bits: BitList::with_capacity(128).expect("valid capacity"),
            data: AttestationData {
                slot: Slot::new(1000),
                index,
                beacon_block_root: Hash256::zero(),
                source: Checkpoint {
                    epoch: Epoch::new(10),
                    root: Hash256::zero(),
                },
                target: Checkpoint {
                    epoch: Epoch::new(11),
                    root: Hash256::zero(),
                },
            },
            signature: AggregateSignature::infinity(),
        };
        VariableList::new(attestation.as_ssz_bytes()).expect("valid attestation bytes")
    }

    /// Create a test sync committee contribution
    fn create_test_contribution(subnet_id: u64) -> SyncCommitteeContribution<MainnetEthSpec> {
        SyncCommitteeContribution {
            slot: Slot::new(1000),
            beacon_block_root: Hash256::zero(),
            subcommittee_index: subnet_id,
            aggregation_bits: Default::default(),
            signature: AggregateSignature::infinity(),
        }
    }

    fn create_test_contribution_for_vote(
        slot: Slot,
        vote: &SlotVote,
        subnet_id: u64,
    ) -> SyncCommitteeContribution<MainnetEthSpec> {
        SyncCommitteeContribution {
            slot,
            beacon_block_root: vote.block_root(),
            subcommittee_index: subnet_id,
            aggregation_bits: Default::default(),
            signature: AggregateSignature::infinity(),
        }
    }

    fn distinct_vote_fields() -> (Hash256, Checkpoint, Checkpoint) {
        (
            Hash256::repeat_byte(0xB1),
            Checkpoint {
                epoch: Epoch::new(5),
                root: Hash256::repeat_byte(0x51),
            },
            Checkpoint {
                epoch: Epoch::new(6),
                root: Hash256::repeat_byte(0x71),
            },
        )
    }

    fn gloas_test_spec() -> ChainSpec {
        let mut spec = ChainSpec::mainnet();
        spec.electra_fork_epoch = Some(Epoch::new(0));
        spec.gloas_fork_epoch = Some(Epoch::new(0));
        spec
    }

    fn gloas_test_vote(root_byte: u8, attestation_data_index: u64) -> SlotVote {
        SlotVote::Gloas(GloasBeaconVote {
            block_root: Hash256::repeat_byte(root_byte),
            source: Checkpoint {
                epoch: Epoch::new(5),
                root: Hash256::repeat_byte(0x51),
            },
            target: Checkpoint {
                epoch: Epoch::new(6),
                root: Hash256::repeat_byte(0x71),
            },
            attestation_data_index,
        })
    }

    fn test_voting_context(slot: Slot, seed: SlotVote) -> VotingContext {
        VotingContext {
            voting_assignments: Arc::new(VotingAssignments {
                slot,
                attesting_validators: vec![],
                attesting_committees: HashMap::new(),
                sync_validators_by_subnet: HashMap::new(),
            }),
            same_slot_head_root: None,
            vote: seed,
            decided_votes: Default::default(),
        }
    }

    fn test_attester(slot: Slot, committee_index: u64) -> DutyAndProof {
        let mut duty_and_proof = DutyAndProof::new_without_selection_proof(
            AttesterData {
                pubkey: PublicKeyBytes::empty(),
                validator_index: 0,
                committees_at_slot: 1,
                committee_index,
                committee_length: 1,
                validator_committee_index: 0,
                slot,
            },
            slot.saturating_sub(1_u64),
        );
        duty_and_proof.selection_proof = Some(SelectionProof::from(Signature::empty()));
        duty_and_proof
    }

    fn test_sync_aggregator() -> SyncAggregatorData {
        SyncAggregatorData {
            validator_index: 0,
            pubkey: PublicKeyBytes::empty(),
            selection_proof: SyncSelectionProof::from(Signature::empty()),
        }
    }

    /// Committee request resolution must carry committee-local QBFT decisions into both Beacon API
    /// request categories. Sharing the beacon committee index and sync subnet ensures only the
    /// resolved vote can distinguish the two committees' request identities.
    #[test]
    fn resolved_committee_requests_use_each_committee_decision() {
        let slot = Slot::new(1);
        let spec = gloas_test_spec();
        let shared_committee_index = 7;
        let shared_subnet = SyncSubnetId::new(2);
        let committee_a = CommitteeId([0xA1; 32]);
        let committee_b = CommitteeId([0xB2; 32]);
        let seed = gloas_test_vote(0x10, 0);
        let decision_a = gloas_test_vote(0x21, 1);
        let decision_b = gloas_test_vote(0x32, 1);
        let voting_context = test_voting_context(slot, seed.clone());
        voting_context
            .remember_decided_vote(committee_a, decision_a.clone())
            .expect("committee A decision should be stored");
        voting_context
            .remember_decided_vote(committee_b, decision_b.clone())
            .expect("committee B decision should be stored");

        let attester_a = test_attester(slot, shared_committee_index);
        let attester_b = test_attester(slot, shared_committee_index);
        let attesters_by_ssv_committee = HashMap::from([
            (committee_a, vec![&attester_a]),
            (committee_b, vec![&attester_b]),
        ]);
        let sync_by_ssv_committee = HashMap::from([
            (committee_a, vec![(shared_subnet, test_sync_aggregator())]),
            (committee_b, vec![(shared_subnet, test_sync_aggregator())]),
        ]);

        let requests =
            MetadataService::<MainnetEthSpec, ManualSlotClock>::resolve_committee_requests(
                &spec,
                slot,
                &voting_context,
                &attesters_by_ssv_committee,
                &sync_by_ssv_committee,
            );

        assert_eq!(requests.votes.get(&committee_a), Some(&decision_a));
        assert_eq!(requests.votes.get(&committee_b), Some(&decision_b));

        let aggregate_a =
            aggregate_fetch_key::<MainnetEthSpec>(&spec, slot, &decision_a, shared_committee_index);
        let aggregate_b =
            aggregate_fetch_key::<MainnetEthSpec>(&spec, slot, &decision_b, shared_committee_index);
        let aggregate_seed =
            aggregate_fetch_key::<MainnetEthSpec>(&spec, slot, &seed, shared_committee_index);
        assert_ne!(aggregate_a, aggregate_b);
        assert_ne!(aggregate_a, aggregate_seed);
        assert_ne!(aggregate_b, aggregate_seed);
        assert_eq!(
            requests.aggregate_attestation_keys,
            HashSet::from([aggregate_a, aggregate_b])
        );
        assert!(
            !requests
                .aggregate_attestation_keys
                .contains(&aggregate_seed)
        );

        let sync_a = sync_contribution_fetch_key(&decision_a, shared_subnet);
        let sync_b = sync_contribution_fetch_key(&decision_b, shared_subnet);
        let sync_seed = sync_contribution_fetch_key(&seed, shared_subnet);
        assert_ne!(sync_a, sync_b);
        assert_ne!(sync_a, sync_seed);
        assert_ne!(sync_b, sync_seed);
        assert_eq!(
            requests.sync_contribution_keys,
            HashSet::from([sync_a, sync_b])
        );
        assert!(!requests.sync_contribution_keys.contains(&sync_seed));
    }

    fn create_test_gloas_attestation(
        slot: Slot,
        vote: &SlotVote,
        committee_index: usize,
    ) -> Attestation<MainnetEthSpec> {
        let mut aggregation_bits = ProgressiveBitList::with_capacity(128);
        aggregation_bits
            .set(committee_index, true)
            .expect("committee index fits aggregation bits");
        let mut committee_bits = BitVector::default();
        committee_bits
            .set(committee_index, true)
            .expect("committee index fits committee bits");

        Attestation::Gloas(AttestationGloas {
            aggregation_bits,
            data: AttestationData {
                slot,
                index: vote.index(),
                beacon_block_root: vote.block_root(),
                source: vote.source(),
                target: vote.target(),
            },
            signature: AggregateSignature::infinity(),
            committee_bits,
        })
    }

    #[test]
    fn slot_vote_preserves_the_fork_specific_attestation_shape() {
        let slot = Slot::new(1);
        let (block_root, source, target) = distinct_vote_fields();
        let attestation_data = AttestationData {
            slot,
            index: 1,
            beacon_block_root: block_root,
            source,
            target,
        };

        let mut pre_gloas_spec = ChainSpec::mainnet();
        pre_gloas_spec.gloas_fork_epoch = None;
        let base = slot_vote_from_attestation_data::<MainnetEthSpec>(
            &pre_gloas_spec,
            slot,
            attestation_data.clone(),
        );
        match base {
            SlotVote::Base(vote) => {
                assert_eq!(vote.block_root, block_root);
                assert_eq!(vote.source, source);
                assert_eq!(vote.target, target);
            }
            SlotVote::Gloas(_) => panic!("pre-Gloas data must produce a Base vote"),
        }

        let mut gloas_spec = ChainSpec::mainnet();
        gloas_spec.electra_fork_epoch = Some(Epoch::new(0));
        gloas_spec.gloas_fork_epoch = Some(Epoch::new(0));
        let gloas =
            slot_vote_from_attestation_data::<MainnetEthSpec>(&gloas_spec, slot, attestation_data);
        match gloas {
            SlotVote::Gloas(vote) => {
                assert_eq!(vote.block_root, block_root);
                assert_eq!(vote.source, source);
                assert_eq!(vote.target, target);
                assert_eq!(vote.attestation_data_index, 1);
            }
            SlotVote::Base(_) => panic!("Gloas data must produce a Gloas vote"),
        }
    }

    #[test]
    fn aggregate_fetch_attestation_data_uses_the_fork_specific_index() {
        let slot = Slot::new(1);
        let committee_index = 7;
        let (block_root, source, target) = distinct_vote_fields();
        let base_vote = SlotVote::Base(BeaconVote {
            block_root,
            source,
            target,
        });

        let mut pre_electra_spec = ChainSpec::mainnet();
        pre_electra_spec.electra_fork_epoch = Some(Epoch::new(1));
        pre_electra_spec.gloas_fork_epoch = None;
        let pre_electra = aggregate_fetch_attestation_data::<MainnetEthSpec>(
            &pre_electra_spec,
            slot,
            &base_vote,
            committee_index,
        );
        assert_eq!(pre_electra.index, committee_index);
        assert_eq!(pre_electra.slot, slot);
        assert_eq!(pre_electra.beacon_block_root, block_root);
        assert_eq!(pre_electra.source, source);
        assert_eq!(pre_electra.target, target);

        let mut electra_spec = ChainSpec::mainnet();
        electra_spec.electra_fork_epoch = Some(Epoch::new(0));
        electra_spec.gloas_fork_epoch = None;
        let electra = aggregate_fetch_attestation_data::<MainnetEthSpec>(
            &electra_spec,
            slot,
            &base_vote,
            committee_index,
        );
        assert_eq!(electra.index, 0);
        assert_eq!(electra.slot, slot);
        assert_eq!(electra.beacon_block_root, block_root);
        assert_eq!(electra.source, source);
        assert_eq!(electra.target, target);

        let mut gloas_spec = ChainSpec::mainnet();
        gloas_spec.electra_fork_epoch = Some(Epoch::new(0));
        gloas_spec.gloas_fork_epoch = Some(Epoch::new(0));
        let gloas_vote = SlotVote::Gloas(GloasBeaconVote {
            block_root,
            source,
            target,
            attestation_data_index: 1,
        });
        let gloas = aggregate_fetch_attestation_data::<MainnetEthSpec>(
            &gloas_spec,
            slot,
            &gloas_vote,
            committee_index,
        );
        assert_eq!(gloas.index, 1);
        assert_ne!(gloas.index, committee_index);
        assert_eq!(gloas.slot, slot);
        assert_eq!(gloas.beacon_block_root, block_root);
        assert_eq!(gloas.source, source);
        assert_eq!(gloas.target, target);

        let gloas_with_zero_index = AttestationData {
            index: 0,
            ..gloas.clone()
        };
        assert_ne!(
            gloas.tree_hash_root(),
            gloas_with_zero_index.tree_hash_root(),
            "the aggregate-fetch root must bind the Gloas attestation index"
        );
    }

    #[test]
    fn composite_fetch_keys_deduplicate_only_identical_requests() {
        let slot = Slot::new(1);
        let subnet_id = SyncSubnetId::new(2);
        let other_subnet_id = SyncSubnetId::new(3);
        let mut spec = ChainSpec::mainnet();
        spec.electra_fork_epoch = Some(Epoch::new(0));
        spec.gloas_fork_epoch = Some(Epoch::new(0));

        let vote_a = SlotVote::Gloas(GloasBeaconVote {
            block_root: Hash256::repeat_byte(0xA1),
            source: Checkpoint {
                epoch: Epoch::new(1),
                root: Hash256::repeat_byte(0x51),
            },
            target: Checkpoint {
                epoch: Epoch::new(2),
                root: Hash256::repeat_byte(0x71),
            },
            attestation_data_index: 1,
        });
        let vote_b = SlotVote::Gloas(GloasBeaconVote {
            block_root: Hash256::repeat_byte(0xB2),
            source: vote_a.source(),
            target: vote_a.target(),
            attestation_data_index: 1,
        });

        let aggregate_a = aggregate_fetch_key::<MainnetEthSpec>(&spec, slot, &vote_a, 7);
        let aggregate_identical = aggregate_fetch_key::<MainnetEthSpec>(&spec, slot, &vote_a, 7);
        let aggregate_divergent = aggregate_fetch_key::<MainnetEthSpec>(&spec, slot, &vote_b, 7);
        let aggregate_other_committee =
            aggregate_fetch_key::<MainnetEthSpec>(&spec, slot, &vote_a, 8);

        assert_eq!(aggregate_a, aggregate_identical);
        assert_ne!(aggregate_a, aggregate_divergent);
        assert_eq!(
            aggregate_a.attestation_data_root, aggregate_other_committee.attestation_data_root,
            "Gloas committees sharing one decided vote use the same attestation-data root"
        );
        assert_ne!(
            aggregate_a, aggregate_other_committee,
            "the beacon committee index must remain part of the request identity"
        );
        assert_eq!(
            HashSet::from([
                aggregate_a,
                aggregate_identical,
                aggregate_divergent,
                aggregate_other_committee,
            ])
            .len(),
            3,
            "only byte-for-byte identical request identities may deduplicate"
        );

        let sync_a = sync_contribution_fetch_key(&vote_a, subnet_id);
        let sync_identical = sync_contribution_fetch_key(&vote_a, subnet_id);
        let sync_divergent = sync_contribution_fetch_key(&vote_b, subnet_id);
        let sync_other_subnet = sync_contribution_fetch_key(&vote_a, other_subnet_id);
        assert_eq!(sync_a, sync_identical);
        assert_ne!(sync_a, sync_divergent);
        assert_ne!(sync_a, sync_other_subnet);
        assert_eq!(
            HashSet::from([sync_a, sync_identical, sync_divergent, sync_other_subnet,]).len(),
            3
        );
    }

    /// Verifies two consensus-critical properties:
    /// 1. Composite-key isolation: candidates ignore fetch results whose Beacon API request
    ///    identity does not match the one derived from the current vote and committee index or
    ///    subnet.
    /// 2. Wire compatibility: surviving aggregators are ordered by validator index, contributors by
    ///    signing root then validator index, with one aligned beacon object per first-seen
    ///    committee index or subnet.
    #[test]
    fn build_consensus_data_uses_composite_results_and_preserves_wire_order() {
        let slot = Slot::new(1);
        let spec = gloas_test_spec();
        let vote = gloas_test_vote(0xA1, 1);
        let foreign_vote = gloas_test_vote(0xB2, 0);

        let attestation_5 = create_test_gloas_attestation(slot, &vote, 5);
        let attestation_10 = create_test_gloas_attestation(slot, &vote, 10);
        let foreign_attestation_7 = create_test_gloas_attestation(slot, &foreign_vote, 7);
        let contribution_0 = create_test_contribution_for_vote(slot, &vote, 0);
        let contribution_1 = create_test_contribution_for_vote(slot, &vote, 1);
        let foreign_contribution_2 = create_test_contribution_for_vote(slot, &foreign_vote, 2);

        let fetch_results = AggregationFetchResults {
            aggregated_attestations: HashMap::from([
                (
                    aggregate_fetch_key::<MainnetEthSpec>(&spec, slot, &vote, 5),
                    attestation_5.clone(),
                ),
                (
                    aggregate_fetch_key::<MainnetEthSpec>(&spec, slot, &vote, 10),
                    attestation_10.clone(),
                ),
                (
                    aggregate_fetch_key::<MainnetEthSpec>(&spec, slot, &foreign_vote, 7),
                    foreign_attestation_7,
                ),
            ]),
            sync_contributions: HashMap::from([
                (
                    sync_contribution_fetch_key(&vote, SyncSubnetId::new(0)),
                    contribution_0.clone(),
                ),
                (
                    sync_contribution_fetch_key(&vote, SyncSubnetId::new(1)),
                    contribution_1.clone(),
                ),
                (
                    sync_contribution_fetch_key(&foreign_vote, SyncSubnetId::new(2)),
                    foreign_contribution_2,
                ),
            ]),
        };

        let aggregators = vec![
            create_aggregator(300, 10),
            create_aggregator(100, 5),
            create_aggregator(200, 5),
            create_aggregator(50, 7),
        ];
        let contributors_with_roots = vec![
            create_contributor_with_root(Hash256::repeat_byte(0xB0), 150, 1),
            create_contributor_with_root(Hash256::repeat_byte(0xA0), 250, 0),
            create_contributor_with_root(Hash256::repeat_byte(0xA0), 50, 0),
            create_contributor_with_root(Hash256::repeat_byte(0xC0), 75, 2),
        ];

        let data = build_consensus_data_from_candidates::<MainnetEthSpec>(
            &spec,
            slot,
            &vote,
            aggregators,
            contributors_with_roots,
            &fetch_results,
        )
        .expect("valid consensus data")
        .expect("matching fetches produce consensus data");

        assert_eq!(data.version, DataVersion::from(ForkName::Gloas));
        assert_eq!(
            data.aggregators
                .iter()
                .map(|aggregator| (aggregator.validator_index.0, aggregator.committee_index))
                .collect::<Vec<_>>(),
            vec![(100, 5), (200, 5), (300, 10)]
        );
        assert_eq!(
            data.aggregator_committee_indexes
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![5, 10]
        );
        assert_eq!(
            data.aggregated_attestations
                .iter()
                .map(|attestation| attestation.to_vec())
                .collect::<Vec<_>>(),
            vec![attestation_5.as_ssz_bytes(), attestation_10.as_ssz_bytes()]
        );
        assert_eq!(
            data.contributors
                .iter()
                .map(|contributor| (contributor.validator_index.0, contributor.committee_index))
                .collect::<Vec<_>>(),
            vec![(50, 0), (250, 0), (150, 1)]
        );
        assert_eq!(
            data.sync_committee_contributions
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            vec![contribution_0, contribution_1]
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════════════
    // P0 Tests: Wire Compatibility
    // ═══════════════════════════════════════════════════════════════════════════════════

    /// P0-1: Verify aggregators are sorted by validator_index ascending.
    ///
    /// This is CRITICAL for consensus - all operators must produce the same hash
    /// for the same input data. If aggregators are not sorted identically,
    /// consensus will fail due to hash mismatch.
    #[test]
    fn test_aggregators_sorted_by_validator_index() {
        // Create aggregators in unsorted order
        let mut aggregators = vec![
            create_aggregator(500, 10),
            create_aggregator(100, 5),
            create_aggregator(300, 7),
            create_aggregator(200, 5), // Same committee as validator 100
            create_aggregator(50, 3),
        ];

        // Sort using the production function
        sort_aggregators_by_validator_index(&mut aggregators);

        // Verify sorted by validator_index ascending
        let indices: Vec<usize> = aggregators.iter().map(|a| a.validator_index.0).collect();
        assert_eq!(
            indices,
            vec![50, 100, 200, 300, 500],
            "Aggregators must be sorted by validator_index ascending for consensus compatibility"
        );

        // Verify sorting is stable for same validator_index edge case
        let mut same_index_aggregators = vec![
            create_aggregator(100, 10),
            create_aggregator(100, 5), // Same validator_index, different committee
        ];
        sort_aggregators_by_validator_index(&mut same_index_aggregators);

        // Both should have validator_index 100, order may vary but that's ok
        // since SSV-Go would produce same result
        assert!(
            same_index_aggregators
                .iter()
                .all(|a| a.validator_index.0 == 100)
        );
    }

    /// P0-2: Verify contributors are sorted by (signing_root, validator_index).
    ///
    /// This is CRITICAL for consensus - sync committee contributors have different
    /// signing roots per subnet, so we must sort by root first, then validator_index.
    /// This matches SSV-Go's root-sorted processing.
    #[test]
    fn test_contributors_sorted_by_signing_root_then_validator_index() {
        // Create signing roots with predictable ordering
        // Hash256 comparison is lexicographic on the underlying bytes
        let root_a = Hash256::from_low_u64_be(1); // Smaller hash
        let root_b = Hash256::from_low_u64_be(2); // Larger hash
        let root_c = Hash256::from_low_u64_be(3); // Even larger hash

        // Create contributors in unsorted order with various roots
        let mut contributors = vec![
            create_contributor_with_root(root_c, 100, 2), // root_c, validator 100
            create_contributor_with_root(root_a, 300, 0), // root_a, validator 300
            create_contributor_with_root(root_b, 50, 1),  // root_b, validator 50
            create_contributor_with_root(root_a, 100, 0), /* root_a, validator 100 (same root as
                                                           * above) */
            create_contributor_with_root(root_b, 200, 1), /* root_b, validator 200 (same root as
                                                           * above) */
        ];

        // Sort using the production function
        sort_contributors_by_signing_root_then_validator_index(&mut contributors);

        // Expected order:
        // 1. root_a, validator 100 (smallest root, smaller validator)
        // 2. root_a, validator 300 (smallest root, larger validator)
        // 3. root_b, validator 50 (middle root, smaller validator)
        // 4. root_b, validator 200 (middle root, larger validator)
        // 5. root_c, validator 100 (largest root)

        let result: Vec<(Hash256, usize)> = contributors
            .iter()
            .map(|(root, agg)| (*root, agg.validator_index.0))
            .collect();

        assert_eq!(
            result,
            vec![
                (root_a, 100),
                (root_a, 300),
                (root_b, 50),
                (root_b, 200),
                (root_c, 100),
            ],
            "Contributors must be sorted by (signing_root, validator_index) for consensus compatibility"
        );
    }

    /// P0-3: Verify same inputs produce identical hash (deterministic output).
    ///
    /// This test verifies that given the same input data, the consensus data
    /// will always produce the same hash. This is essential for distributed
    /// consensus - all operators must agree on the hash for the same data.
    #[test]
    fn test_deterministic_output_same_inputs() {
        // Create identical consensus data twice using the same methodology
        // that build_consensus_data_for_committee would use

        let build_consensus_data = || {
            // Create aggregators and sort them
            let mut aggregators = vec![
                create_aggregator(300, 10),
                create_aggregator(100, 5),
                create_aggregator(200, 5),
            ];
            sort_aggregators_by_validator_index(&mut aggregators);

            // Create contributors with roots and sort them
            let root_a = Hash256::from_low_u64_be(100);
            let root_b = Hash256::from_low_u64_be(200);
            let mut contributors_with_roots = vec![
                create_contributor_with_root(root_b, 50, 1),
                create_contributor_with_root(root_a, 100, 0),
                create_contributor_with_root(root_a, 50, 0),
            ];
            sort_contributors_by_signing_root_then_validator_index(&mut contributors_with_roots);

            let contributors: Vec<AssignedAggregator> = contributors_with_roots
                .into_iter()
                .map(|(_, contrib)| contrib)
                .collect();

            // Extract committee indexes in first-seen order from sorted aggregators
            let committee_indexes: IndexSet<u64> =
                aggregators.iter().map(|a| a.committee_index).collect();

            // Extract subnet IDs in first-seen order from sorted contributors
            // (kept for illustration but unused since we create fixed contributions)
            let _subnet_ids: IndexSet<SyncSubnetId> = contributors
                .iter()
                .map(|c| SyncSubnetId::new(c.committee_index))
                .collect();

            // Build the consensus data structure
            AggregatorCommitteeConsensusData::<MainnetEthSpec> {
                version: DataVersion::from(ForkName::Deneb),
                aggregators: aggregators.try_into().expect("valid aggregators"),
                aggregator_committee_indexes: committee_indexes
                    .into_iter()
                    .collect::<Vec<_>>()
                    .try_into()
                    .expect("valid indexes"),
                aggregated_attestations: VariableList::new(vec![
                    create_attestation_bytes(5),
                    create_attestation_bytes(10),
                ])
                .expect("valid attestations"),
                contributors: contributors.try_into().expect("valid contributors"),
                sync_committee_contributions: VariableList::new(vec![
                    create_test_contribution(0),
                    create_test_contribution(1),
                ])
                .expect("valid contributions"),
            }
        };

        // Build twice and compare hashes
        let data1 = build_consensus_data();
        let data2 = build_consensus_data();

        // Use the QbftData::hash() method which is used for consensus
        use ssv_types::consensus::QbftData;
        let hash1 = data1.hash();
        let hash2 = data2.hash();

        assert_eq!(
            hash1, hash2,
            "Same inputs must produce identical hash for consensus to work"
        );

        // Also verify SSZ encoding is identical (which is what hash() uses)
        assert_eq!(
            data1.as_ssz_bytes(),
            data2.as_ssz_bytes(),
            "Same inputs must produce identical SSZ encoding"
        );
    }

    /// P0-4: Verify built data passes AggregatorCommitteeDataValidator.
    ///
    /// This test ensures that the consensus data built using our sorting and
    /// construction logic passes the validation that will be performed during
    /// QBFT consensus. If our data fails validation, consensus will fail.
    #[test]
    fn test_output_passes_validator_with_both() {
        // Build consensus data with both aggregators and contributors
        // following the exact same logic as build_consensus_data_for_committee

        // Create and sort aggregators
        let mut aggregators = vec![
            create_aggregator(300, 10),
            create_aggregator(100, 5),
            create_aggregator(200, 5), // Same committee_index as validator 100
        ];
        sort_aggregators_by_validator_index(&mut aggregators);

        // Create and sort contributors
        let root_a = Hash256::from_low_u64_be(100);
        let root_b = Hash256::from_low_u64_be(200);
        let mut contributors_with_roots = vec![
            create_contributor_with_root(root_b, 150, 1),
            create_contributor_with_root(root_a, 250, 0),
            create_contributor_with_root(root_a, 50, 0), // Same subnet as validator 250
        ];
        sort_contributors_by_signing_root_then_validator_index(&mut contributors_with_roots);

        let contributors: Vec<AssignedAggregator> = contributors_with_roots
            .into_iter()
            .map(|(_, contrib)| contrib)
            .collect();

        // Extract committee indexes in first-seen order (preserving order from sorted aggregators)
        let committee_indexes: IndexSet<u64> =
            aggregators.iter().map(|a| a.committee_index).collect();

        // Extract subnet IDs in first-seen order (preserving order from sorted contributors)
        let subnet_ids: IndexSet<SyncSubnetId> = contributors
            .iter()
            .map(|c| SyncSubnetId::new(c.committee_index))
            .collect();

        // Build attestation bytes matching the committee indexes
        let attestation_bytes: Vec<VariableList<u8, MaxAggregatedAttestationBytes>> =
            committee_indexes
                .iter()
                .map(|&idx| create_attestation_bytes(idx))
                .collect();

        // Build contributions matching the subnet IDs
        let contributions: Vec<SyncCommitteeContribution<MainnetEthSpec>> = subnet_ids
            .iter()
            .map(|id| create_test_contribution((*id).into()))
            .collect();

        // Build the complete consensus data
        let consensus_data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: aggregators.try_into().expect("valid aggregators"),
            aggregator_committee_indexes: committee_indexes
                .into_iter()
                .collect::<Vec<_>>()
                .try_into()
                .expect("valid indexes"),
            aggregated_attestations: attestation_bytes.try_into().expect("valid attestations"),
            contributors: contributors.try_into().expect("valid contributors"),
            sync_committee_contributions: contributions.try_into().expect("valid contributions"),
        };

        // Create validator and run validation
        let validator = AggregatorCommitteeDataValidator::<MainnetEthSpec>::new();

        // Validate using the do_validation method for detailed error reporting
        let result = validator.do_validation(&consensus_data);
        assert!(
            result.is_ok(),
            "Consensus data built with proper sorting/construction must pass validation. Error: {:?}",
            result.err()
        );

        // Also test via the QbftDataValidator trait (used during actual consensus)
        let passes_trait_validation =
            QbftDataValidator::validate(&validator, &consensus_data, &consensus_data);
        assert!(
            passes_trait_validation,
            "Consensus data must pass QbftDataValidator trait validation"
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════════════
    // Additional Edge Case Tests
    // ═══════════════════════════════════════════════════════════════════════════════════

    /// Test that IndexSet preserves first-seen order from sorted aggregators
    #[test]
    fn test_committee_indexes_preserve_first_seen_order() {
        // Create aggregators with repeated committee indexes
        let mut aggregators = vec![
            create_aggregator(300, 10), // First occurrence of 10
            create_aggregator(100, 5),  // First occurrence of 5
            create_aggregator(200, 5),  // Second occurrence of 5 (should be deduped)
            create_aggregator(400, 10), // Second occurrence of 10 (should be deduped)
            create_aggregator(50, 3),   // First occurrence of 3
        ];

        // Sort aggregators first
        sort_aggregators_by_validator_index(&mut aggregators);

        // Extract committee indexes preserving first-seen order
        let committee_indexes: IndexSet<u64> =
            aggregators.iter().map(|a| a.committee_index).collect();

        // After sorting by validator_index: [50, 100, 200, 300, 400]
        // Committee indexes in order: [3, 5, 5, 10, 10]
        // First-seen unique: [3, 5, 10]
        let indexes: Vec<u64> = committee_indexes.into_iter().collect();
        assert_eq!(
            indexes,
            vec![3, 5, 10],
            "Committee indexes must be in first-seen order from sorted aggregators"
        );
    }

    /// Test that subnet IDs preserve first-seen order from sorted contributors
    #[test]
    fn test_subnet_ids_preserve_first_seen_order() {
        // Create signing roots
        let root_a = Hash256::from_low_u64_be(1);
        let root_b = Hash256::from_low_u64_be(2);

        // Create contributors with repeated subnet IDs
        let mut contributors_with_roots = vec![
            create_contributor_with_root(root_b, 100, 1), // root_b, subnet 1
            create_contributor_with_root(root_a, 200, 0), // root_a, subnet 0
            create_contributor_with_root(root_a, 50, 0),  // root_a, subnet 0 (duplicate)
            create_contributor_with_root(root_b, 150, 1), // root_b, subnet 1 (duplicate)
        ];

        // Sort contributors first
        sort_contributors_by_signing_root_then_validator_index(&mut contributors_with_roots);

        // Extract just the contributors
        let contributors: Vec<AssignedAggregator> = contributors_with_roots
            .into_iter()
            .map(|(_, contrib)| contrib)
            .collect();

        // Extract subnet IDs preserving first-seen order
        let subnet_ids: IndexSet<SyncSubnetId> = contributors
            .iter()
            .map(|c| SyncSubnetId::new(c.committee_index))
            .collect();

        // After sorting: root_a first (validators 50, 200 both with subnet 0),
        // then root_b (validators 100, 150 both with subnet 1)
        // First-seen unique: [subnet 0, subnet 1]
        let ids: Vec<u64> = subnet_ids.into_iter().map(|id| id.into()).collect();
        assert_eq!(
            ids,
            vec![0, 1],
            "Subnet IDs must be in first-seen order from sorted contributors"
        );
    }

    /// Test empty inputs produce valid (None) result
    #[test]
    fn test_empty_aggregators_and_contributors() {
        let mut aggregators: Vec<AssignedAggregator> = vec![];
        sort_aggregators_by_validator_index(&mut aggregators);
        assert!(aggregators.is_empty());

        let mut contributors: Vec<(Hash256, AssignedAggregator)> = vec![];
        sort_contributors_by_signing_root_then_validator_index(&mut contributors);
        assert!(contributors.is_empty());
    }

    // ═══════════════════════════════════════════════════════════════════════════════════
    // P1 Tests: High Priority - Failure Scenarios
    // ═══════════════════════════════════════════════════════════════════════════════════

    /// P1-1: When all attestation fetches fail AND all contribution fetches fail,
    /// the result should indicate no valid data (empty aggregators AND empty contributors
    /// after filtering).
    ///
    /// This simulates the scenario where beacon node is unavailable or returns errors
    /// for all requested attestations and contributions. The consensus data building
    /// logic should gracefully handle this by producing no consensus data (None result
    /// in build_consensus_data_for_committee).
    #[test]
    fn test_all_fetches_fail_returns_none() {
        let slot = Slot::new(1);
        let spec = gloas_test_spec();
        let vote = gloas_test_vote(0xA1, 1);
        let aggregators = vec![
            create_aggregator(100, 5),
            create_aggregator(200, 10),
            create_aggregator(300, 15),
        ];
        let root = Hash256::zero();
        let contributors = vec![
            create_contributor_with_root(root, 50, 0),
            create_contributor_with_root(root, 150, 1),
            create_contributor_with_root(root, 250, 2),
        ];
        let fetch_results = AggregationFetchResults {
            aggregated_attestations: HashMap::new(),
            sync_contributions: HashMap::new(),
        };

        let result = build_consensus_data_from_candidates::<MainnetEthSpec>(
            &spec,
            slot,
            &vote,
            aggregators,
            contributors,
            &fetch_results,
        )
        .expect("empty fetch results are valid");

        assert!(result.is_none());
    }

    // ═══════════════════════════════════════════════════════════════════════════════════
    // P2 Tests: Medium Priority - Valid Edge Cases
    // ═══════════════════════════════════════════════════════════════════════════════════

    /// P2-1: Valid scenario - no attestation aggregators, only sync contributors.
    ///
    /// Should produce valid consensus data with empty aggregators list but populated
    /// contributors. This is a valid scenario when a committee only has sync committee
    /// aggregation duties and no attestation aggregation duties for a given slot.
    #[test]
    fn test_empty_aggregators_only_contributors() {
        let slot = Slot::new(1);
        let spec = gloas_test_spec();
        let vote = gloas_test_vote(0xA1, 1);
        let root_a = Hash256::from_low_u64_be(1);
        let root_b = Hash256::from_low_u64_be(2);
        let contributors_with_roots = vec![
            create_contributor_with_root(root_b, 150, 1),
            create_contributor_with_root(root_a, 250, 0),
            create_contributor_with_root(root_a, 50, 0), // Same subnet as validator 250
        ];
        let contribution_0 = create_test_contribution_for_vote(slot, &vote, 0);
        let contribution_1 = create_test_contribution_for_vote(slot, &vote, 1);
        let fetch_results = AggregationFetchResults {
            aggregated_attestations: HashMap::new(),
            sync_contributions: HashMap::from([
                (
                    sync_contribution_fetch_key(&vote, SyncSubnetId::new(0)),
                    contribution_0,
                ),
                (
                    sync_contribution_fetch_key(&vote, SyncSubnetId::new(1)),
                    contribution_1,
                ),
            ]),
        };

        let consensus_data = build_consensus_data_from_candidates::<MainnetEthSpec>(
            &spec,
            slot,
            &vote,
            vec![],
            contributors_with_roots,
            &fetch_results,
        )
        .expect("valid contributor-only data")
        .expect("contributors produce consensus data");

        assert!(consensus_data.aggregators.is_empty());
        assert!(consensus_data.aggregator_committee_indexes.is_empty());
        assert!(consensus_data.aggregated_attestations.is_empty());
        assert_eq!(
            consensus_data
                .contributors
                .iter()
                .map(|contributor| (contributor.validator_index.0, contributor.committee_index))
                .collect::<Vec<_>>(),
            vec![(50, 0), (250, 0), (150, 1)]
        );
        assert_eq!(consensus_data.sync_committee_contributions.len(), 2);

        let validator = AggregatorCommitteeDataValidator::<MainnetEthSpec>::new();
        let result = validator.do_validation(&consensus_data);
        assert!(
            result.is_ok(),
            "Consensus data with only contributors should pass validation. Error: {:?}",
            result.err()
        );
    }

    /// P2-2: Valid scenario - no sync contributors, only attestation aggregators.
    ///
    /// Should produce valid consensus data with empty contributors list but populated
    /// aggregators. This is a valid scenario when a committee only has attestation
    /// aggregation duties and no sync committee aggregation duties for a given slot.
    #[test]
    fn test_empty_contributors_only_aggregators() {
        let slot = Slot::new(1);
        let spec = gloas_test_spec();
        let vote = gloas_test_vote(0xA1, 1);
        let aggregators = vec![
            create_aggregator(300, 10),
            create_aggregator(100, 5),
            create_aggregator(200, 7),
        ];
        let attestation_5 = create_test_gloas_attestation(slot, &vote, 5);
        let attestation_7 = create_test_gloas_attestation(slot, &vote, 7);
        let attestation_10 = create_test_gloas_attestation(slot, &vote, 10);
        let fetch_results = AggregationFetchResults {
            aggregated_attestations: HashMap::from([
                (
                    aggregate_fetch_key::<MainnetEthSpec>(&spec, slot, &vote, 5),
                    attestation_5,
                ),
                (
                    aggregate_fetch_key::<MainnetEthSpec>(&spec, slot, &vote, 7),
                    attestation_7,
                ),
                (
                    aggregate_fetch_key::<MainnetEthSpec>(&spec, slot, &vote, 10),
                    attestation_10,
                ),
            ]),
            sync_contributions: HashMap::new(),
        };

        let consensus_data = build_consensus_data_from_candidates::<MainnetEthSpec>(
            &spec,
            slot,
            &vote,
            aggregators,
            vec![],
            &fetch_results,
        )
        .expect("valid aggregator-only data")
        .expect("aggregators produce consensus data");

        assert_eq!(
            consensus_data
                .aggregators
                .iter()
                .map(|aggregator| (aggregator.validator_index.0, aggregator.committee_index))
                .collect::<Vec<_>>(),
            vec![(100, 5), (200, 7), (300, 10)]
        );
        assert_eq!(
            consensus_data
                .aggregator_committee_indexes
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![5, 7, 10]
        );
        assert_eq!(consensus_data.aggregated_attestations.len(), 3);
        assert!(consensus_data.contributors.is_empty());
        assert!(consensus_data.sync_committee_contributions.is_empty());

        let validator = AggregatorCommitteeDataValidator::<MainnetEthSpec>::new();
        let result = validator.do_validation(&consensus_data);
        assert!(
            result.is_ok(),
            "Consensus data with only aggregators should pass validation. Error: {:?}",
            result.err()
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════════════
    // Weighted Attestation Data (WAD) Scoring Tests
    // ═══════════════════════════════════════════════════════════════════════════════════

    fn create_wad_attestation_data(
        source_epoch: u64,
        target_epoch: u64,
        slot: u64,
    ) -> AttestationData {
        AttestationData {
            slot: Slot::new(slot),
            index: 0,
            beacon_block_root: Hash256::zero(),
            source: Checkpoint {
                epoch: Epoch::new(source_epoch),
                root: Hash256::zero(),
            },
            target: Checkpoint {
                epoch: Epoch::new(target_epoch),
                root: Hash256::zero(),
            },
        }
    }

    #[test]
    fn test_scoring_higher_epochs_win() {
        let newer = create_wad_attestation_data(100, 101, 3232);
        let newer_result = calculate_attestation_score(&newer, Some(Slot::new(3230)));

        // base = 100 + 101 = 201, distance = 2, bonus = 1/(1+2) = 0.333...
        assert!((newer_result.score - 201.333333).abs() < 0.001);

        let older = create_wad_attestation_data(99, 100, 3232);
        let older_result = calculate_attestation_score(&older, Some(Slot::new(3230)));

        // base = 199, distance = 2, bonus = 0.333...
        assert!((older_result.score - 199.333333).abs() < 0.001);

        assert!(newer_result.score > older_result.score);
    }

    #[test]
    fn test_scoring_proximity_bonus() {
        let data = create_wad_attestation_data(100, 101, 3232);

        let result_distance_zero = calculate_attestation_score(&data, Some(Slot::new(3232)));
        // distance = 0, bonus = 1/(1+0) = 1.0, score = 202.0
        assert_eq!(result_distance_zero.score, 202.0);

        let result_distance_one = calculate_attestation_score(&data, Some(Slot::new(3231)));
        // distance = 1, bonus = 0.5, score = 201.5
        assert_eq!(result_distance_one.score, 201.5);

        assert!(result_distance_zero.score > result_distance_one.score);
    }

    #[test]
    fn test_scoring_no_head_slot() {
        let data = create_wad_attestation_data(100, 101, 3232);
        let result = calculate_attestation_score(&data, None);
        // no head slot = no bonus, score = 201.0
        assert_eq!(result.score, 201.0);
    }

    #[test]
    fn test_scoring_head_after_attestation_slot() {
        // head_slot (3232) > attestation_slot (3230), no bonus
        let data = create_wad_attestation_data(100, 101, 3230);
        let result = calculate_attestation_score(&data, Some(Slot::new(3232)));
        assert_eq!(result.score, 201.0);
    }

    #[test]
    fn test_scoring_same_base_different_proximity() {
        let data = create_wad_attestation_data(100, 101, 3232);

        let result_distance_1 = calculate_attestation_score(&data, Some(Slot::new(3231)));
        assert_eq!(result_distance_1.score, 201.5);

        let result_distance_2 = calculate_attestation_score(&data, Some(Slot::new(3230)));
        assert!((result_distance_2.score - 201.333333).abs() < 0.001);
    }

    use slot_clock::ManualSlotClock;
    use tokio::time::{Duration as TokioDuration, timeout};

    const TEST_SLOT: u64 = 100;
    const SLOT_DURATION_SECS: u64 = 12;
    const CHANNEL_CAPACITY: usize = 8;
    const FAST_RESOLVE_TIMEOUT: TokioDuration = TokioDuration::from_secs(1);
    const TIMER_ARM_DURATION: TokioDuration = TokioDuration::from_millis(50);

    fn make_test_slot_clock() -> ManualSlotClock {
        let clock = ManualSlotClock::new(
            Slot::new(0),
            std::time::Duration::from_secs(0),
            std::time::Duration::from_secs(SLOT_DURATION_SECS),
        );
        clock.set_slot(TEST_SLOT);
        clock
    }

    fn make_head_event_channel() -> (mpsc::Sender<HeadEvent>, mpsc::Receiver<HeadEvent>) {
        mpsc::channel(CHANNEL_CAPACITY)
    }

    fn make_head_event(slot: u64) -> HeadEvent {
        HeadEvent {
            beacon_node_index: 0,
            slot: Slot::new(slot),
            beacon_block_root: Hash256::zero(),
        }
    }

    /// A head event for the context's slot fixes its root as same-slot knowledge.
    #[test]
    fn same_slot_head_root_records_matching_event() {
        let event = HeadEvent {
            beacon_node_index: 0,
            slot: Slot::new(TEST_SLOT),
            beacon_block_root: Hash256::repeat_byte(0x5a),
        };
        assert_eq!(
            same_slot_head_root(Slot::new(TEST_SLOT), Some(&event)),
            Some(Hash256::repeat_byte(0x5a))
        );
    }

    /// The timer path carries no head knowledge.
    #[test]
    fn same_slot_head_root_is_none_on_timer() {
        assert_eq!(same_slot_head_root(Slot::new(TEST_SLOT), None), None);
    }

    /// An event matched against an earlier clock read must not bind the previous slot's head to
    /// this slot's context after a boundary crossing.
    #[test]
    fn same_slot_head_root_is_none_when_event_slot_differs() {
        let event = make_head_event(TEST_SLOT - 1);
        assert_eq!(same_slot_head_root(Slot::new(TEST_SLOT), Some(&event)), None);
    }

    // A head event for the current slot should resolve the wait immediately.
    #[tokio::test(start_paused = true)]
    async fn wait_for_head_event_returns_matching_slot_event() {
        let slot_clock = make_test_slot_clock();
        let (tx, mut rx) = make_head_event_channel();
        tx.send(make_head_event(TEST_SLOT)).await.unwrap();

        let result = timeout(
            FAST_RESOLVE_TIMEOUT,
            wait_for_head_event(&mut rx, &slot_clock),
        )
        .await
        .unwrap();

        let event = result.unwrap();
        assert_eq!(event.slot, Slot::new(TEST_SLOT));
    }

    // A stale event (past slot) gets dropped, so the timer arm wins the race.
    #[tokio::test(start_paused = true)]
    async fn wait_for_head_event_drops_stale_events() {
        let slot_clock = make_test_slot_clock();
        let (tx, mut rx) = make_head_event_channel();
        tx.send(make_head_event(TEST_SLOT - 1)).await.unwrap();

        let from_head_event = tokio::select! {
            biased;
            _ = tokio::time::sleep(TIMER_ARM_DURATION) => false,
            _ = wait_for_head_event(&mut rx, &slot_clock) => true,
        };

        assert!(!from_head_event);
    }

    // ═══════════════════════════════════════════════════════════════════════════════════
    // Slot start publisher tests
    //
    // The virtual tokio timer stands in for the monotonic timer the publisher sleeps on,
    // and the manual slot clock stands in for the wall clock it reads slots from. Driving
    // them separately is what lets these tests reproduce the drift between the two.
    // ═══════════════════════════════════════════════════════════════════════════════════

    use parking_lot::Mutex;

    const SLOT_DURATION: Duration = Duration::from_secs(SLOT_DURATION_SECS);
    /// How far the wall clock trails the virtual timer when the boundary sleep wakes. The
    /// drift observed in issue #1223 was 1 ms to 5 ms.
    const WALL_CLOCK_LAG: Duration = Duration::from_millis(3);
    /// Slot boundaries crossed by `publishes_each_slot_exactly_once`.
    const LOCKSTEP_SLOTS: u64 = 3;

    /// Spawn the publisher, recording the slot its clock reads at each publish. Production
    /// reads the slot inside `update_voting_assignments`, so the recorded slot is the one
    /// that would have been published.
    async fn spawn_slot_start_publisher(slot_clock: &ManualSlotClock) -> Arc<Mutex<Vec<Slot>>> {
        let published = Arc::new(Mutex::new(Vec::new()));
        let publish_clock = slot_clock.clone();
        let recorded = published.clone();
        tokio::spawn(run_slot_start_publisher(slot_clock.clone(), move || {
            let slot = publish_clock.now().expect("manual slot clock is readable");
            recorded.lock().push(slot);
        }));
        // Let the publisher arm its first sleep before either clock moves.
        tokio::task::yield_now().await;
        published
    }

    /// Advance the virtual timer, giving a sleep that expires within the step a scheduling
    /// round to run. `tokio::time::advance` only wakes the sleeper, it does not poll it, so
    /// without the extra round the publish would be observed a step late.
    async fn advance_virtual_time(duration: Duration) {
        tokio::time::advance(duration).await;
        tokio::task::yield_now().await;
    }

    /// Move the wall clock and the virtual timer forward by the same amount. The wall clock
    /// moves first so a timer firing within the step reads the time the step ends at.
    async fn advance_in_lockstep(slot_clock: &ManualSlotClock, duration: Duration) {
        slot_clock.advance_time(duration);
        advance_virtual_time(duration).await;
    }

    /// Regression test for issue #1223.
    ///
    /// A boundary sleep that wakes a few milliseconds before the wall clock reaches the
    /// slot boundary used to republish the previous slot, and the next iteration then slept
    /// past the new slot entirely, so waiters on it saw `MetadataSlotPassed`. The publish
    /// offset has to hold the publish back until the wall clock has crossed.
    #[tokio::test(start_paused = true)]
    async fn early_wake_still_publishes_the_new_slot() {
        // Arrange: publisher armed at the start of TEST_SLOT.
        let slot_clock = make_test_slot_clock();
        let published = spawn_slot_start_publisher(&slot_clock).await;

        // Act: fire the boundary sleep with the wall clock still short of the boundary.
        slot_clock.advance_time(SLOT_DURATION - WALL_CLOCK_LAG);
        advance_virtual_time(SLOT_DURATION).await;

        assert!(
            published.lock().is_empty(),
            "publishing at the early wake would read slot {TEST_SLOT} again"
        );

        // The wall clock crosses the boundary within the publish offset.
        advance_in_lockstep(&slot_clock, VOTING_ASSIGNMENTS_PUBLISH_DELAY).await;

        // Assert: the new slot is published once, and the previous slot is not repeated.
        assert_eq!(
            published.lock().as_slice(),
            [Slot::new(TEST_SLOT + 1)],
            "an early wake must still publish the new slot exactly once"
        );
    }

    /// Without drift, every slot boundary produces exactly one publish for that slot.
    #[tokio::test(start_paused = true)]
    async fn publishes_each_slot_exactly_once() {
        // Arrange: publisher armed at the start of TEST_SLOT.
        let slot_clock = make_test_slot_clock();
        let published = spawn_slot_start_publisher(&slot_clock).await;

        // Act: settle onto the publish offset, then cross one boundary per step.
        advance_in_lockstep(&slot_clock, VOTING_ASSIGNMENTS_PUBLISH_DELAY).await;
        for _ in 0..LOCKSTEP_SLOTS {
            advance_in_lockstep(&slot_clock, SLOT_DURATION).await;
        }

        // Assert: one publish per boundary, slots strictly increasing by one.
        let expected: Vec<Slot> = (1..=LOCKSTEP_SLOTS)
            .map(|offset| Slot::new(TEST_SLOT + offset))
            .collect();
        assert_eq!(published.lock().as_slice(), expected);
    }
}
