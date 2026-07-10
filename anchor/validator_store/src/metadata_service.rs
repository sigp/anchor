use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use beacon_node_fallback::{BeaconNodeFallback, beacon_head_monitor::HeadEvent};
use bls::PublicKeyBytes;
use eth2::{
    BeaconNodeHttpClient,
    types::{BlockId, SyncContributionData},
};
use fork::{Fork, ForkSchedule};
use futures::stream::{FuturesUnordered, StreamExt};
use slot_clock::SlotClock;
use ssv_types::{
    CommitteeId, IndexSet, ValidatorIndex, VariableList,
    consensus::{
        AggregatorCommitteeConsensusData, AssignedAggregator, BeaconVote, DataVersion,
        GloasBeaconVote,
    },
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
    Attestation, AttestationData, ChainSpec, EthSpec, ForkName, Hash256, Slot,
    SyncCommitteeContribution, SyncSelectionProof, SyncSubnetId,
};
use validator_services::duties_service::{DutiesService, DutyAndProof};

use crate::{
    AggregationAssignments, AnchorValidatorStore, ContributionWaiter, SlotVote, VotingAssignments,
    VotingContext, metrics,
};

/// Data for sync committee aggregators.
struct SyncAggregatorData {
    validator_index: u64,
    pubkey: PublicKeyBytes,
    selection_proof: SyncSelectionProof,
}

/// Map from SSV committee to its sync aggregators grouped by subnet.
type SyncByCommitteeMap = HashMap<CommitteeId, Vec<(SyncSubnetId, SyncAggregatorData)>>;

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

#[derive(Clone)]
pub struct MetadataService<E: EthSpec, T: SlotClock + 'static> {
    duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>,
    validator_store: Arc<AnchorValidatorStore<T, E>>,
    slot_clock: T,
    beacon_nodes: Arc<BeaconNodeFallback<T>>,
    executor: TaskExecutor,
    spec: Arc<ChainSpec>,
    fork_schedule: Arc<ForkSchedule>,
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

impl<E: EthSpec, T: SlotClock + 'static> MetadataService<E, T> {
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>,
        validator_store: Arc<AnchorValidatorStore<T, E>>,
        slot_clock: T,
        beacon_nodes: Arc<BeaconNodeFallback<T>>,
        executor: TaskExecutor,
        spec: Arc<ChainSpec>,
        fork_schedule: Arc<ForkSchedule>,
        weighted_attestation_data: bool,
    ) -> Self {
        Self {
            duties_service,
            validator_store,
            slot_clock,
            beacon_nodes,
            executor,
            spec,
            fork_schedule,
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
                loop {
                    if let Some(duration_to_next_slot) =
                        self_clone_phase1.slot_clock.duration_to_next_slot()
                    {
                        // Sleep until slot start
                        sleep(duration_to_next_slot).await;

                        if let Err(err) = self_clone_phase1.update_voting_assignments() {
                            error!(err, "Failed to update validator voting assignments");
                        }
                    } else {
                        error!("Failed to read slot clock");
                        sleep(slot_duration).await;
                    }
                }
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
        // PHASE 3: AggregationAssignments (2/3 slot)
        // Re-fetches `duties_service.attesters()` after selection proofs are computed.
        // At this point, `DutyAndProof.selection_proof.is_some()` accurately indicates
        // `is_aggregator` for attestation duties.
        // ═══════════════════════════════════════════════════════════════════════
        let self_clone_phase3 = self.clone();
        executor.spawn(
            async move {
                loop {
                    if let Some(duration_to_next_slot) =
                        self_clone_phase3.slot_clock.duration_to_next_slot()
                    {
                        // Sleep until 2/3 into slot
                        sleep(duration_to_next_slot + slot_duration * 2 / 3).await;

                        if let Err(err) = self_clone_phase3.update_aggregation_assignments().await {
                            error!(err, "Failed to update aggregator voting assignments");
                        }
                    } else {
                        error!("Failed to read slot clock");
                        sleep(slot_duration).await;
                    }
                }
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
                let mut map = HashMap::<ValidatorIndex, HashSet<SyncSubnetId>>::new();
                sync_duties
                    .duties
                    .iter()
                    .filter_map(|duty| {
                        SyncSubnetId::compute_subnets_for_sync_committee::<E>(
                            &duty.validator_sync_committee_indices,
                        )
                        .map_err(|e| {
                            tracing::warn!(
                                "Failed to compute sync subnets for validator {}: {e:?}",
                                duty.validator_index
                            );
                        })
                        .ok()
                        .map(|subnet_ids| {
                            (ValidatorIndex(duty.validator_index as usize), subnet_ids)
                        })
                    })
                    .for_each(|(validator_index, subnet_ids)| {
                        map.entry(validator_index).or_default().extend(subnet_ids);
                    });
                map
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

    /// Phase 3: Build and publish `AggregationAssignments` at 2/3 slot.
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
        // Collects: `aggregator_committees`, `attesters_by_ssv_committee`,
        //           `attestation_committee_indexes`
        // ═══════════════════════════════════════════════════════════════════════
        let mut aggregator_committees: HashMap<PublicKeyBytes, u64> =
            HashMap::with_capacity(attesters.len());
        let mut attesters_by_ssv_committee: HashMap<CommitteeId, Vec<&DutyAndProof>> =
            HashMap::new();
        let mut attestation_committee_indexes: HashSet<u64> =
            HashSet::with_capacity(attesters.len());

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
                attestation_committee_indexes.insert(attester.duty.committee_index);
            }
        }

        // ═══════════════════════════════════════════════════════════════════════
        // SINGLE PASS over `sync_aggregators`
        // Only processes validators with valid, non-liquidated SSV committees.
        // Collects: `validator_subnet_counts` (for multi_sync), `sync_by_ssv_committee`,
        //           `all_subnet_ids`
        // ═══════════════════════════════════════════════════════════════════════
        let sync_aggregators = sync_duties.as_ref().map(|duties| &duties.aggregators);

        let mut validator_subnet_counts: HashMap<PublicKeyBytes, usize> = HashMap::new();
        let mut sync_by_ssv_committee: SyncByCommitteeMap = HashMap::new();
        let mut all_subnet_ids: HashSet<SyncSubnetId> =
            HashSet::with_capacity(sync_aggregators.map(|a| a.len()).unwrap_or(0));

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
                        all_subnet_ids.insert(*subnet_id);
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

        let consensus_data_by_ssv_committee =
            if self.fork_schedule.active_fork(epoch) >= Fork::Boole {
                self.build_consensus_data_for_all_committees(
                    slot,
                    attesters_by_ssv_committee,
                    sync_by_ssv_committee,
                    attestation_committee_indexes,
                    all_subnet_ids,
                )
                .await?
            } else {
                HashMap::new()
            };

        let aggregator_info = AggregationAssignments {
            slot,
            aggregator_committees,
            multi_sync_aggregators,
            consensus_data_by_ssv_committee,
        };

        self.validator_store
            .update_aggregation_assignments(aggregator_info);

        trace!(%slot, "Published AggregationAssignments at 2/3 slot");
        Ok(())
    }

    /// Build `AggregatorCommitteeConsensusData` for each committee that has aggregators.
    ///
    /// Takes pre-grouped data from `update_aggregation_assignments` to avoid redundant iteration.
    async fn build_consensus_data_for_all_committees(
        &self,
        slot: Slot,
        attesters_by_ssv_committee: HashMap<CommitteeId, Vec<&DutyAndProof>>,
        sync_by_ssv_committee: SyncByCommitteeMap,
        attestation_committee_indexes: HashSet<u64>,
        all_subnet_ids: HashSet<SyncSubnetId>,
    ) -> Result<HashMap<CommitteeId, Arc<AggregatorCommitteeConsensusData<E>>>, String> {
        // Get `VotingContext` for the slot's `vote` (cached at 1/3 slot)
        let voting_context = self
            .validator_store
            .get_voting_context(slot)
            .await
            .map_err(|e| format!("Failed to get voting context: {:?}", e))?;

        // Parallel fetch from beacon node with timeout for partial results.
        // Uses `FuturesUnordered` internally to collect results as they complete.
        // After BEACON_API_FETCH_TIMEOUT (2s), returns whatever has been collected.
        // This ensures we don't block on slow beacon nodes while still getting partial data.
        let (aggregated_attestations, sync_contributions) = tokio::join!(
            self.fetch_aggregated_attestations(
                slot,
                &voting_context.vote,
                &attestation_committee_indexes,
                BEACON_API_FETCH_TIMEOUT,
            ),
            self.fetch_sync_contributions(
                slot,
                voting_context.vote.block_root(),
                &all_subnet_ids,
                BEACON_API_FETCH_TIMEOUT,
            ),
        );

        // Build consensus data per committee
        // All committees with work
        let ssv_committees: HashSet<CommitteeId> = attesters_by_ssv_committee
            .keys()
            .chain(sync_by_ssv_committee.keys())
            .copied()
            .collect();

        let mut result = HashMap::with_capacity(ssv_committees.len());
        for ssv_committee_id in ssv_committees {
            let ssv_committee_attesters = attesters_by_ssv_committee.get(&ssv_committee_id);
            let ssv_committee_sync = sync_by_ssv_committee.get(&ssv_committee_id);

            let consensus_data = self.build_consensus_data_for_committee(
                slot,
                &ssv_committee_id,
                ssv_committee_attesters,
                ssv_committee_sync,
                &aggregated_attestations,
                &sync_contributions,
            )?;

            if let Some(data) = consensus_data {
                result.insert(ssv_committee_id, Arc::new(data));
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
        aggregated_attestations: &HashMap<u64, Attestation<E>>,
        sync_contributions: &HashMap<SyncSubnetId, SyncCommitteeContribution<E>>,
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

        // Sort and filter aggregators using helper functions
        sort_aggregators_by_validator_index(&mut aggregators);
        filter_aggregators_with_attestations(&mut aggregators, aggregated_attestations);

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

        // Sort and filter contributors using helper functions
        sort_contributors_by_signing_root_then_validator_index(&mut contributors_with_roots);
        filter_contributors_with_contributions(&mut contributors_with_roots, sync_contributions);

        let contributors: Vec<AssignedAggregator> = contributors_with_roots
            .into_iter()
            .map(|(_, contributor)| contributor)
            .collect();

        // Early exit if no aggregators/contributors
        if aggregators.is_empty() && contributors.is_empty() {
            return Ok(None);
        }

        // === COMMITTEE INDEXES & ATTESTATIONS ===
        // Extract unique committee indexes preserving first-seen order from sorted aggregators.
        // `IndexSet` deduplicates while maintaining insertion order, matching SSV Go's approach
        // of adding new indexes as they're encountered during iteration.
        let attestation_committee_indexes: IndexSet<u64> =
            aggregators.iter().map(|a| a.committee_index).collect();

        // Get attestations in attestation_committee_indexes order (1:1 correspondence)
        let attestations_bytes: Vec<VariableList<u8, _>> = attestation_committee_indexes
            .iter()
            .filter_map(|committee_index| aggregated_attestations.get(committee_index))
            .map(|attestation| {
                let bytes = attestation.as_ssz_bytes();
                VariableList::new(bytes).map_err(|e| {
                    warn!("Failed to create attestation bytes list: {:?}", e);
                    e
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to create attestation bytes: {:?}", e))?;

        // === SUBNET IDS & CONTRIBUTIONS ===
        // Extract unique subnet IDs preserving first-seen order from sorted contributors.
        // `IndexSet` deduplicates while maintaining insertion order, matching SSV Go's approach
        // of adding new IDs as they're encountered during iteration.
        let subnet_ids: IndexSet<SyncSubnetId> = contributors
            .iter()
            .map(|c| SyncSubnetId::new(c.committee_index))
            .collect();

        let contributions: Vec<SyncCommitteeContribution<E>> = subnet_ids
            .iter()
            .filter_map(|id| sync_contributions.get(id).cloned())
            .collect();

        // Get the fork version
        let epoch = slot.epoch(E::slots_per_epoch());
        let fork_name = self.spec.fork_name_at_epoch(epoch);
        let version = DataVersion::from(fork_name);

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

    /// Fetch aggregated attestations from beacon node for the given committee indexes.
    /// Returns a map of `committee_index` -> `Attestation`.
    ///
    /// Uses `FuturesUnordered` to collect results as they complete. When the timeout is reached,
    /// returns whatever results have been collected so far (partial results). This ensures
    /// we don't block on slow beacon nodes while still using successful fetches.
    async fn fetch_aggregated_attestations(
        &self,
        slot: Slot,
        vote: &SlotVote,
        attestation_committee_indexes: &HashSet<u64>,
        timeout: Duration,
    ) -> HashMap<u64, Attestation<E>> {
        let _timer = metrics::start_timer_vec(
            &metrics::AGGREGATOR_COMMITTEE_FETCH_TIMES,
            &["aggregated_attestations"],
        );

        // Create `FuturesUnordered` for concurrent execution with partial result collection
        let beacon_nodes = &self.beacon_nodes;
        let mut futures: FuturesUnordered<_> = attestation_committee_indexes
            .iter()
            .map(|&committee_index| {
                // Reconstruct the attestation data to compute its tree hash root.
                // Pre-Electra uses the committee index, Electra through pre-Gloas uses zero,
                // and Gloas uses the BN-supplied index carried by the slot vote.
                let attestation_data = aggregate_fetch_attestation_data::<E>(
                    &self.spec,
                    slot,
                    vote,
                    committee_index,
                );
                let attestation_data_root = attestation_data.tree_hash_root();

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
                                    attestation_data_root,
                                    committee_index,
                                )
                                .await
                                .map_err(|e| {
                                    format!("[AggregatorCommittee] Failed to produce aggregate attestation: {:?}", e)
                                })?
                                .ok_or_else(|| {
                                    format!(
                                        "[AggregatorCommittee] No aggregate available for slot {}, committee {}",
                                        slot, committee_index
                                    )
                                })
                                .map(|result| result.into_data())
                        })
                        .await;
                    (committee_index, result)
                }
            })
            .collect();

        let total_committees = attestation_committee_indexes.len();
        let mut aggregated_attestations = HashMap::with_capacity(total_committees);
        let deadline = Instant::now() + timeout;

        // Collect results as they complete, until timeout or all done
        loop {
            // Exit when all futures completed
            if futures.is_empty() {
                break;
            }

            tokio::select! {
                Some((committee_index, result)) = futures.next() => {
                    match result {
                        Ok(attestation) => {
                            aggregated_attestations.insert(committee_index, attestation);
                        }
                        Err(e) => {
                            warn!(
                                %slot,
                                %committee_index,
                                error = %e,
                                "[AggregatorCommittee] Failed to fetch aggregated attestation for committee"
                            );
                        }
                    }
                }
                _ = sleep_until(deadline) => {
                    if aggregated_attestations.len() < total_committees {
                        warn!(
                            %slot,
                            collected = aggregated_attestations.len(),
                            total = total_committees,
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
        let total = total_committees;
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

    /// Fetch sync committee contributions from beacon node for the given subnet IDs.
    /// Returns a map of subnet_id -> SyncCommitteeContribution.
    ///
    /// Uses `FuturesUnordered` to collect results as they complete. When the timeout is reached,
    /// returns whatever results have been collected so far (partial results). This ensures
    /// we don't block on slow beacon nodes while still using successful fetches.
    async fn fetch_sync_contributions(
        &self,
        slot: Slot,
        beacon_block_root: Hash256,
        subnet_ids: &HashSet<SyncSubnetId>,
        timeout: Duration,
    ) -> HashMap<SyncSubnetId, SyncCommitteeContribution<E>> {
        let _timer = metrics::start_timer_vec(
            &metrics::AGGREGATOR_COMMITTEE_FETCH_TIMES,
            &["sync_contributions"],
        );
        // Create `FuturesUnordered` for concurrent execution with partial result collection
        let beacon_nodes = &self.beacon_nodes;
        let mut futures: FuturesUnordered<_> = subnet_ids
            .iter()
            .map(|&subnet_id| async move {
                let result = beacon_nodes
                    .first_success(|beacon_node| async move {
                        let sync_contribution_data = SyncContributionData {
                            slot,
                            beacon_block_root,
                            subcommittee_index: subnet_id.into(),
                        };

                        beacon_node
                            .get_validator_sync_committee_contribution(&sync_contribution_data)
                            .await
                    })
                    .instrument(info_span!("fetch_sync_contribution"))
                    .await;
                (subnet_id, result)
            })
            .collect();

        let total_subnets = subnet_ids.len();
        let mut sync_contributions = HashMap::with_capacity(total_subnets);
        let deadline = Instant::now() + timeout;

        // Collect results as they complete, until timeout or all done
        loop {
            // Exit when all futures completed
            if futures.is_empty() {
                break;
            }

            tokio::select! {
                Some((subnet_id, result)) = futures.next() => {
                    match result {
                        Ok(Some(response)) => {
                            sync_contributions.insert(subnet_id, response.data);
                        }
                        Ok(None) => {
                            warn!(
                                %slot,
                                ?beacon_block_root,
                                ?subnet_id,
                                "[AggregatorCommittee] No sync contribution found for subnet"
                            );
                        }
                        Err(e) => {
                            error!(
                                %slot,
                                ?beacon_block_root,
                                ?subnet_id,
                                error = %e,
                                "[AggregatorCommittee] Failed to fetch sync contribution for subnet"
                            );
                        }
                    }
                }
                _ = sleep_until(deadline) => {
                    if sync_contributions.len() < total_subnets {
                        warn!(
                            %slot,
                            collected = sync_contributions.len(),
                            total = total_subnets,
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
        let total = total_subnets;
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

/// Filter aggregators to only those whose attestation was successfully fetched.
///
/// This maintains 1:1 correspondence between `aggregator_committee_indexes` and attestations.
pub fn filter_aggregators_with_attestations<E: EthSpec>(
    aggregators: &mut Vec<AssignedAggregator>,
    aggregated_attestations: &HashMap<u64, Attestation<E>>,
) {
    aggregators.retain(|agg| aggregated_attestations.contains_key(&agg.committee_index));
}

/// Filter contributors to only those whose sync contribution was successfully fetched.
///
/// This maintains 1:1 correspondence between `subnet_ids` and contributions.
pub fn filter_contributors_with_contributions<E: EthSpec>(
    contributors_with_roots: &mut Vec<(Hash256, AssignedAggregator)>,
    sync_contributions: &HashMap<SyncSubnetId, SyncCommitteeContribution<E>>,
) {
    contributors_with_roots.retain(|(_, contrib)| {
        sync_contributions.contains_key(&SyncSubnetId::new(contrib.committee_index))
    });
}

#[cfg(test)]
mod tests {
    use bls::{AggregateSignature, FixedBytesExtended, Signature};
    use ssv_types::{
        IndexSet, VariableList,
        consensus::{
            AggregatorCommitteeConsensusData, AggregatorCommitteeDataValidator, AssignedAggregator,
            DataVersion, MaxAggregatedAttestationBytes, QbftDataValidator,
        },
    };
    use ssz::Encode;
    use ssz_types::BitList;
    use types::{
        AttestationBase, AttestationData, Checkpoint, Epoch, ForkName, MainnetEthSpec, Slot,
        SyncCommitteeContribution,
    };

    use super::*;

    // ═══════════════════════════════════════════════════════════════════════════════════
    // Test Helpers
    // ═══════════════════════════════════════════════════════════════════════════════════

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

    /// Create a test attestation for a given committee index
    fn create_test_attestation(index: u64) -> Attestation<MainnetEthSpec> {
        Attestation::Base(AttestationBase {
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
        })
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

    /// Test filter_aggregators_with_attestations removes aggregators without attestations
    #[test]
    fn test_filter_aggregators_removes_unfetched() {
        let mut aggregators = vec![
            create_aggregator(100, 5),
            create_aggregator(200, 10),
            create_aggregator(300, 15),
        ];

        // Only have attestation for committee 5 and 15, not 10
        let mut attestations = HashMap::new();
        attestations.insert(5, create_test_attestation(5));
        attestations.insert(15, create_test_attestation(15));

        filter_aggregators_with_attestations(&mut aggregators, &attestations);

        assert_eq!(
            aggregators.len(),
            2,
            "Should filter out aggregator with committee_index 10"
        );
        let remaining_indexes: Vec<usize> =
            aggregators.iter().map(|a| a.validator_index.0).collect();
        assert_eq!(remaining_indexes, vec![100, 300]);
    }

    /// Test filter_contributors_with_contributions removes contributors without contributions
    #[test]
    fn test_filter_contributors_removes_unfetched() {
        let root = Hash256::zero();
        let mut contributors = vec![
            create_contributor_with_root(root, 100, 0),
            create_contributor_with_root(root, 200, 1),
            create_contributor_with_root(root, 300, 2),
        ];

        // Only have contributions for subnet 0 and 2, not 1
        let mut contributions = HashMap::new();
        contributions.insert(SyncSubnetId::new(0), create_test_contribution(0));
        contributions.insert(SyncSubnetId::new(2), create_test_contribution(2));

        filter_contributors_with_contributions(&mut contributors, &contributions);

        assert_eq!(
            contributors.len(),
            2,
            "Should filter out contributor with subnet_id 1"
        );
        let remaining_indexes: Vec<usize> = contributors
            .iter()
            .map(|(_, a)| a.validator_index.0)
            .collect();
        assert_eq!(remaining_indexes, vec![100, 300]);
    }

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
        // Create aggregators with various committee indexes
        let mut aggregators = vec![
            create_aggregator(100, 5),
            create_aggregator(200, 10),
            create_aggregator(300, 15),
        ];
        sort_aggregators_by_validator_index(&mut aggregators);

        // Create contributors with various subnet IDs
        let root = Hash256::zero();
        let mut contributors = vec![
            create_contributor_with_root(root, 50, 0),
            create_contributor_with_root(root, 150, 1),
            create_contributor_with_root(root, 250, 2),
        ];
        sort_contributors_by_signing_root_then_validator_index(&mut contributors);

        // Empty attestations map - simulates all fetches failing
        let aggregated_attestations: HashMap<u64, Attestation<MainnetEthSpec>> = HashMap::new();

        // Empty contributions map - simulates all fetches failing
        let sync_contributions: HashMap<SyncSubnetId, SyncCommitteeContribution<MainnetEthSpec>> =
            HashMap::new();

        // Apply the filter functions (as done in build_consensus_data_for_committee)
        filter_aggregators_with_attestations(&mut aggregators, &aggregated_attestations);
        filter_contributors_with_contributions(&mut contributors, &sync_contributions);

        // After filtering, both should be empty because no attestations/contributions were fetched
        assert!(
            aggregators.is_empty(),
            "All aggregators should be filtered out when no attestations are fetched"
        );
        assert!(
            contributors.is_empty(),
            "All contributors should be filtered out when no contributions are fetched"
        );

        // This matches the early exit condition in build_consensus_data_for_committee:
        // if aggregators.is_empty() && contributors.is_empty() { return Ok(None); }
        let should_return_none = aggregators.is_empty() && contributors.is_empty();
        assert!(
            should_return_none,
            "When all fetches fail, build_consensus_data_for_committee should return None"
        );
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
        // No aggregators
        let aggregators: Vec<AssignedAggregator> = vec![];

        // Create contributors with contributions
        let root_a = Hash256::from_low_u64_be(1);
        let root_b = Hash256::from_low_u64_be(2);
        let mut contributors_with_roots = vec![
            create_contributor_with_root(root_b, 150, 1),
            create_contributor_with_root(root_a, 250, 0),
            create_contributor_with_root(root_a, 50, 0), // Same subnet as validator 250
        ];
        sort_contributors_by_signing_root_then_validator_index(&mut contributors_with_roots);

        // Create contributions for all subnets
        let mut sync_contributions = HashMap::new();
        sync_contributions.insert(SyncSubnetId::new(0), create_test_contribution(0));
        sync_contributions.insert(SyncSubnetId::new(1), create_test_contribution(1));

        // Filter contributors (all should remain since we have contributions)
        filter_contributors_with_contributions(&mut contributors_with_roots, &sync_contributions);

        // Extract contributors after filtering
        let contributors: Vec<AssignedAggregator> = contributors_with_roots
            .into_iter()
            .map(|(_, contrib)| contrib)
            .collect();

        assert!(
            !contributors.is_empty(),
            "Contributors should not be empty when contributions are fetched"
        );
        assert_eq!(
            contributors.len(),
            3,
            "All 3 contributors should remain after filtering"
        );

        // Extract subnet IDs in first-seen order
        let subnet_ids: IndexSet<SyncSubnetId> = contributors
            .iter()
            .map(|c| SyncSubnetId::new(c.committee_index))
            .collect();

        // Build contributions matching subnet IDs
        let contributions: Vec<SyncCommitteeContribution<MainnetEthSpec>> = subnet_ids
            .iter()
            .filter_map(|id| sync_contributions.get(id).cloned())
            .collect();

        // Build the consensus data with empty aggregators but populated contributors
        let consensus_data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: aggregators.try_into().expect("valid empty aggregators"),
            aggregator_committee_indexes: Vec::<u64>::new()
                .try_into()
                .expect("valid empty indexes"),
            aggregated_attestations: VariableList::empty(),
            contributors: contributors.try_into().expect("valid contributors"),
            sync_committee_contributions: contributions.try_into().expect("valid contributions"),
        };

        // Verify structure is valid
        assert!(
            consensus_data.aggregators.is_empty(),
            "Aggregators should be empty"
        );
        assert!(
            !consensus_data.contributors.is_empty(),
            "Contributors should be populated"
        );
        assert!(
            consensus_data.aggregated_attestations.is_empty(),
            "Attestations should be empty"
        );
        assert!(
            !consensus_data.sync_committee_contributions.is_empty(),
            "Contributions should be populated"
        );

        // Validate with the consensus data validator
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
        // Create aggregators with attestations
        let mut aggregators = vec![
            create_aggregator(300, 10),
            create_aggregator(100, 5),
            create_aggregator(200, 7),
        ];
        sort_aggregators_by_validator_index(&mut aggregators);

        // Create attestations for all committees
        let mut aggregated_attestations = HashMap::new();
        aggregated_attestations.insert(5, create_test_attestation(5));
        aggregated_attestations.insert(7, create_test_attestation(7));
        aggregated_attestations.insert(10, create_test_attestation(10));

        // Filter aggregators (all should remain since we have attestations)
        filter_aggregators_with_attestations(&mut aggregators, &aggregated_attestations);

        assert!(
            !aggregators.is_empty(),
            "Aggregators should not be empty when attestations are fetched"
        );
        assert_eq!(
            aggregators.len(),
            3,
            "All 3 aggregators should remain after filtering"
        );

        // Extract committee indexes in first-seen order from sorted aggregators
        let committee_indexes: IndexSet<u64> =
            aggregators.iter().map(|a| a.committee_index).collect();

        // Build attestation bytes matching committee indexes
        let attestation_bytes: Vec<VariableList<u8, MaxAggregatedAttestationBytes>> =
            committee_indexes
                .iter()
                .map(|&idx| create_attestation_bytes(idx))
                .collect();

        // No contributors
        let contributors: Vec<AssignedAggregator> = vec![];

        // Build the consensus data with populated aggregators but empty contributors
        let consensus_data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: aggregators.try_into().expect("valid aggregators"),
            aggregator_committee_indexes: committee_indexes
                .into_iter()
                .collect::<Vec<_>>()
                .try_into()
                .expect("valid indexes"),
            aggregated_attestations: attestation_bytes.try_into().expect("valid attestations"),
            contributors: contributors.try_into().expect("valid empty contributors"),
            sync_committee_contributions: VariableList::empty(),
        };

        // Verify structure is valid
        assert!(
            !consensus_data.aggregators.is_empty(),
            "Aggregators should be populated"
        );
        assert!(
            consensus_data.contributors.is_empty(),
            "Contributors should be empty"
        );
        assert!(
            !consensus_data.aggregated_attestations.is_empty(),
            "Attestations should be populated"
        );
        assert!(
            consensus_data.sync_committee_contributions.is_empty(),
            "Contributions should be empty"
        );

        // Validate with the consensus data validator
        let validator = AggregatorCommitteeDataValidator::<MainnetEthSpec>::new();
        let result = validator.do_validation(&consensus_data);
        assert!(
            result.is_ok(),
            "Consensus data with only aggregators should pass validation. Error: {:?}",
            result.err()
        );
    }

    /// P2-3: 5 aggregators all assigned to committee_index 42.
    ///
    /// Should:
    /// - Sort by validator_index
    /// - Deduplicate committee_index to single entry in IndexSet
    /// - All 5 aggregators remain in the list
    /// - Only 1 committee_index in the output
    ///
    /// This tests the deduplication behavior of IndexSet for committee indexes
    /// when multiple aggregators share the same committee.
    #[test]
    fn test_multiple_aggregators_same_committee() {
        // Create 5 aggregators all with committee_index 42, in unsorted order
        let mut aggregators = vec![
            create_aggregator(500, 42),
            create_aggregator(100, 42),
            create_aggregator(300, 42),
            create_aggregator(200, 42),
            create_aggregator(400, 42),
        ];

        // Sort by validator_index
        sort_aggregators_by_validator_index(&mut aggregators);

        // Verify sorted order
        let validator_indices: Vec<usize> =
            aggregators.iter().map(|a| a.validator_index.0).collect();
        assert_eq!(
            validator_indices,
            vec![100, 200, 300, 400, 500],
            "Aggregators must be sorted by validator_index ascending"
        );

        // Create attestation for committee 42
        let mut aggregated_attestations = HashMap::new();
        aggregated_attestations.insert(42, create_test_attestation(42));

        // Filter aggregators (all should remain since we have the attestation for committee 42)
        filter_aggregators_with_attestations(&mut aggregators, &aggregated_attestations);

        // All 5 aggregators should remain
        assert_eq!(
            aggregators.len(),
            5,
            "All 5 aggregators should remain after filtering since attestation for committee 42 exists"
        );

        // Extract committee indexes in first-seen order (should deduplicate to 1 entry)
        let committee_indexes: IndexSet<u64> =
            aggregators.iter().map(|a| a.committee_index).collect();

        // Verify only 1 unique committee_index
        assert_eq!(
            committee_indexes.len(),
            1,
            "IndexSet should deduplicate to single committee_index"
        );
        assert!(
            committee_indexes.contains(&42),
            "The single committee_index should be 42"
        );

        // Build attestation bytes (only 1 attestation for the single committee_index)
        let attestation_bytes: Vec<VariableList<u8, MaxAggregatedAttestationBytes>> =
            committee_indexes
                .iter()
                .map(|&idx| create_attestation_bytes(idx))
                .collect();

        assert_eq!(
            attestation_bytes.len(),
            1,
            "Should have exactly 1 attestation bytes entry for the deduplicated committee"
        );

        // Build the consensus data
        let consensus_data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: aggregators.try_into().expect("valid aggregators"),
            aggregator_committee_indexes: committee_indexes
                .into_iter()
                .collect::<Vec<_>>()
                .try_into()
                .expect("valid indexes"),
            aggregated_attestations: attestation_bytes.try_into().expect("valid attestations"),
            contributors: Vec::<AssignedAggregator>::new()
                .try_into()
                .expect("valid empty contributors"),
            sync_committee_contributions: VariableList::empty(),
        };

        // Verify final structure
        assert_eq!(
            consensus_data.aggregators.len(),
            5,
            "Should have all 5 aggregators in final consensus data"
        );
        assert_eq!(
            consensus_data.aggregator_committee_indexes.len(),
            1,
            "Should have only 1 committee_index in final consensus data"
        );
        assert_eq!(
            consensus_data.aggregated_attestations.len(),
            1,
            "Should have only 1 attestation in final consensus data"
        );

        // Validate with the consensus data validator
        let validator = AggregatorCommitteeDataValidator::<MainnetEthSpec>::new();
        let result = validator.do_validation(&consensus_data);
        assert!(
            result.is_ok(),
            "Consensus data with multiple aggregators same committee should pass validation. Error: {:?}",
            result.err()
        );

        // Also verify via QbftDataValidator trait
        let passes_trait_validation =
            QbftDataValidator::validate(&validator, &consensus_data, &consensus_data);
        assert!(
            passes_trait_validation,
            "Consensus data must pass QbftDataValidator trait validation"
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
}
