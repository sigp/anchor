//! Duty Input Publisher
//!
//! This service publishes duty inputs at scheduled times during each slot:
//!
//! - **Phase 1 (slot start)**: `VotingAssignments` - caches which validators are attesting/syncing
//! - **Phase 2 (1/3 slot)**: `VotingContext` - fetches beacon_vote and combines with assignments
//! - **Phase 3 (2/3 slot)**: `AggregationAssignments` - builds consensus data for aggregation
//!   duties
//!
//! # Separation of Concerns
//!
//! This service focuses on **timing and publishing** of duty inputs. The actual consensus data
//! building logic is delegated to [`AggregatorConsensusBuilder`].
//!
//! Previously named `MetadataService`, this was renamed to better reflect its actual role
//! as a publisher of duty inputs rather than a generic "metadata" handler.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use beacon_node_fallback::BeaconNodeFallback;
use bls::PublicKeyBytes;
use fork::{Fork, ForkSchedule};
use slot_clock::SlotClock;
use ssv_types::{CommitteeId, ValidatorIndex, consensus::BeaconVote};
use task_executor::TaskExecutor;
use tokio::time::sleep;
use tracing::{error, info, trace};
use types::{ChainSpec, EthSpec};
use validator_services::duties_service::DutiesService;

use crate::{
    AggregationAssignments, AnchorValidatorStore, ContributionWaiter, VotingAssignments,
    VotingContext,
    aggregator_consensus_builder::{
        AggregatorConsensusBuilder, SyncAggregatorData, SyncByCommitteeMap,
    },
    metrics,
};

/// Maximum time to wait for beacon node API calls to fetch aggregated attestations
/// and sync contributions. After this timeout, we return whatever partial results
/// have been collected. This is shorter than the standard 3-second Lighthouse timeout
/// because SSV has additional latency for QBFT consensus and P2P propagation.
const BEACON_API_FETCH_TIMEOUT: Duration = Duration::from_secs(2);

/// Publishes duty inputs at scheduled times during each slot.
///
/// This service is responsible for:
/// - Caching voting assignments at slot start
/// - Fetching beacon_vote at 1/3 slot
/// - Building and publishing aggregation assignments at 2/3 slot
#[derive(Clone)]
pub struct DutyInputPublisher<E: EthSpec, T: SlotClock + 'static> {
    duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>,
    validator_store: Arc<AnchorValidatorStore<T, E>>,
    slot_clock: T,
    beacon_nodes: Arc<BeaconNodeFallback<T>>,
    executor: TaskExecutor,
    spec: Arc<ChainSpec>,
    fork_schedule: Arc<ForkSchedule>,
}

impl<E: EthSpec, T: SlotClock + 'static> DutyInputPublisher<E, T> {
    pub fn new(
        duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>,
        validator_store: Arc<AnchorValidatorStore<T, E>>,
        slot_clock: T,
        beacon_nodes: Arc<BeaconNodeFallback<T>>,
        executor: TaskExecutor,
        spec: Arc<ChainSpec>,
        fork_schedule: Arc<ForkSchedule>,
    ) -> Self {
        Self {
            duties_service,
            validator_store,
            slot_clock,
            beacon_nodes,
            executor,
            spec,
            fork_schedule,
        }
    }

    pub fn start_update_service(self) -> Result<(), String> {
        let slot_duration = Duration::from_secs(self.spec.seconds_per_slot);
        let duration_to_next_slot = self
            .slot_clock
            .duration_to_next_slot()
            .ok_or("Unable to determine duration to next slot")?;

        info!(
            next_update_millis = duration_to_next_slot.as_millis(),
            "Duty input publisher started"
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

                        if let Err(err) = self_clone_phase1.publish_voting_assignments() {
                            error!(err, "Failed to publish voting assignments");
                        }
                    } else {
                        error!("Failed to read slot clock");
                        sleep(slot_duration).await;
                    }
                }
            },
            "voting_assignments_publisher",
        );

        // ═══════════════════════════════════════════════════════════════════════
        // PHASE 2: VotingContext (1/3 slot)
        // Gets cached voting assignments, fetches beacon_vote, builds VotingContext.
        // ═══════════════════════════════════════════════════════════════════════
        let self_clone_phase2 = self.clone();
        executor.spawn(
            async move {
                loop {
                    if let Some(duration_to_next_slot) =
                        self_clone_phase2.slot_clock.duration_to_next_slot()
                    {
                        // Sleep until 1/3 into slot
                        sleep(duration_to_next_slot + slot_duration / 3).await;

                        if let Err(err) = self_clone_phase2.publish_voting_context().await {
                            error!(err, "Failed to publish voting context")
                        } else {
                            trace!("Published voting context");
                        }
                    } else {
                        error!("Failed to read slot clock");
                        sleep(slot_duration).await;
                    }
                }
            },
            "voting_context_publisher",
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

                        if let Err(err) = self_clone_phase3.publish_aggregation_assignments().await
                        {
                            error!(err, "Failed to publish aggregation assignments");
                        }
                    } else {
                        error!("Failed to read slot clock");
                        sleep(slot_duration).await;
                    }
                }
            },
            "aggregation_assignments_publisher",
        );

        Ok(())
    }

    /// Phase 1: Build and publish `VotingAssignments` at slot start.
    fn publish_voting_assignments(&self) -> Result<(), String> {
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
                let mut map = HashMap::<ValidatorIndex, HashSet<types::SyncSubnetId>>::new();
                sync_duties
                    .duties
                    .iter()
                    .filter_map(|duty| {
                        types::SyncSubnetId::compute_subnets_for_sync_committee::<E>(
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

    /// Phase 2: Build and publish `VotingContext` at 1/3 slot.
    async fn publish_voting_context(&self) -> Result<(), String> {
        let slot = self.slot_clock.now().ok_or("Failed to read slot clock")?;

        let voting_assignments = self
            .validator_store
            .get_voting_assignments(slot)
            .await
            .map_err(|e| format!("Failed to get cached voting assignments: {:?}", e))?;

        // Fetch beacon_vote from beacon node
        let attestation_data = self
            .beacon_nodes
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
            .map_err(|e| e.to_string())?;

        let beacon_vote = BeaconVote {
            block_root: attestation_data.beacon_block_root,
            source: attestation_data.source,
            target: attestation_data.target,
        };

        let voting_context = VotingContext {
            voting_assignments,
            beacon_vote,
        };

        self.validator_store.update_voting_context(voting_context);

        trace!(%slot, "Published VotingContext at 1/3 slot");
        Ok(())
    }

    /// Phase 3: Build and publish `AggregationAssignments` at 2/3 slot.
    ///
    /// Uses single-pass data transformation to minimize iterations:
    /// - ONE pass over attesters (those with `selection_proof`) to build all attester-related data
    /// - ONE pass over `sync_aggregators` to build all sync-related data
    /// - Then delegates to `AggregatorConsensusBuilder` for beacon fetches and consensus data
    async fn publish_aggregation_assignments(&self) -> Result<(), String> {
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
        let mut attesters_by_ssv_committee: HashMap<CommitteeId, Vec<_>> = HashMap::new();
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
        let mut all_subnet_ids: HashSet<types::SyncSubnetId> =
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
        // Delegate to AggregatorConsensusBuilder for the heavy lifting
        // ═══════════════════════════════════════════════════════════════════════
        let epoch = slot.epoch(E::slots_per_epoch());

        let consensus_data_by_ssv_committee =
            if self.fork_schedule.active_fork(epoch) >= Fork::Boole {
                // Get voting context for beacon_vote (cached at 1/3 slot)
                let voting_context = self
                    .validator_store
                    .get_voting_context(slot)
                    .await
                    .map_err(|e| format!("Failed to get voting context: {:?}", e))?;

                let builder = AggregatorConsensusBuilder::new(
                    &self.validator_store,
                    &self.beacon_nodes,
                    &self.spec,
                );

                builder
                    .build_consensus_data_for_all_committees(
                        slot,
                        attesters_by_ssv_committee,
                        sync_by_ssv_committee,
                        attestation_committee_indexes,
                        all_subnet_ids,
                        &voting_context,
                        BEACON_API_FETCH_TIMEOUT,
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
}
