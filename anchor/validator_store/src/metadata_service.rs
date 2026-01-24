use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use beacon_node_fallback::BeaconNodeFallback;
use slot_clock::SlotClock;
use ssv_types::{ValidatorIndex, consensus::BeaconVote};
use task_executor::TaskExecutor;
use tokio::{sync::watch, time::sleep};
use tracing::{error, info, trace, warn};
use types::{ChainSpec, EthSpec, Slot, SyncSubnetId};
use validator_services::duties_service::DutiesService;

use crate::{
    AggregationAssignments, AnchorValidatorStore, ContributionWaiter, VotingAssignments,
    VotingContext, metrics,
};

#[derive(Clone)]
pub struct MetadataService<E: EthSpec, T: SlotClock + 'static> {
    duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>,
    validator_store: Arc<AnchorValidatorStore<T, E>>,
    slot_clock: T,
    beacon_nodes: Arc<BeaconNodeFallback<T>>,
    executor: TaskExecutor,
    spec: Arc<ChainSpec>,
    attesters_poll_rx: watch::Receiver<Slot>,
    sync_poll_rx: watch::Receiver<Slot>,
}

impl<E: EthSpec, T: SlotClock + 'static> MetadataService<E, T> {
    pub fn new(
        duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>,
        validator_store: Arc<AnchorValidatorStore<T, E>>,
        slot_clock: T,
        beacon_nodes: Arc<BeaconNodeFallback<T>>,
        executor: TaskExecutor,
        spec: Arc<ChainSpec>,
    ) -> Self {
        let attesters_poll_rx = duties_service.subscribe_to_attesters_poll();
        let sync_poll_rx = duties_service.subscribe_to_sync_poll();

        Self {
            duties_service,
            validator_store,
            slot_clock,
            beacon_nodes,
            executor,
            spec,
            attesters_poll_rx,
            sync_poll_rx,
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
            "Metadata service started"
        );

        let executor = self.executor.clone();

        // ═══════════════════════════════════════════════════════════════════════
        // PHASE 1: VotingAssignments (slot start)
        // Caches voting assignments for use by both selection proofs AND voting context.
        // Waits for DutiesService to complete polling before reading duties.
        //
        // RESILIENCE: We wait up to 3.5s for poll signals, but always proceed to
        // read from duties cache regardless of poll outcome. This handles:
        // - Normal operation: Signals arrive quickly (< 100ms), we read fresh duties
        // - Beacon node slow: Signal arrives late (< 3s), we still get fresh duties
        // - Beacon node down: Timeout after 3.5s, we read from cache (stale or empty)
        //
        // The 3.5s timeout is chosen because:
        // - Lighthouse BN API timeout is 3 seconds (slot_duration / 4)
        // - Phase 2 starts at 4 seconds (1/3 slot)
        // - This gives 500ms buffer after BN timeout before Phase 2 deadline
        // ═══════════════════════════════════════════════════════════════════════
        let self_clone_phase1 = self.clone();
        executor.spawn(
            async move {
                let mut attesters_poll_rx = self_clone_phase1.attesters_poll_rx.clone();
                let mut sync_poll_rx = self_clone_phase1.sync_poll_rx.clone();
                let poll_timeout = Duration::from_millis(3500);

                loop {
                    if let Some(duration_to_next_slot) =
                        self_clone_phase1.slot_clock.duration_to_next_slot()
                    {
                        // Sleep until slot start
                        sleep(duration_to_next_slot).await;

                        let slot: Slot = self_clone_phase1.slot_clock.now().unwrap_or_default();
                        let poll_start = Instant::now();

                        // Wait for poll signals with timeout (parallel execution).
                        let (attesters_ready, sync_ready) = tokio::join!(
                            async {
                                tokio::time::timeout(
                                    poll_timeout,
                                    attesters_poll_rx.wait_for(|&s| s >= slot),
                                )
                                .await
                                .is_ok_and(|r| r.is_ok())
                            },
                            async {
                                tokio::time::timeout(
                                    poll_timeout,
                                    sync_poll_rx.wait_for(|&s| s >= slot),
                                )
                                .await
                                .is_ok_and(|r| r.is_ok())
                            }
                        );

                        let poll_duration = poll_start.elapsed();

                        // Log poll failures
                        if !attesters_ready {
                            warn!(
                                %slot,
                                poll_duration_ms = poll_duration.as_millis(),
                                "Attesters poll failed or timed out - will use cached duties"
                            );
                        }
                        if !sync_ready {
                            warn!(
                                %slot,
                                poll_duration_ms = poll_duration.as_millis(),
                                "Sync poll failed or timed out - will use cached duties"
                            );
                        }

                        // Record poll telemetry
                        metrics::inc_counter_vec(
                            &metrics::METADATA_SERVICE_POLL_TOTAL,
                            &[
                                metrics::ATTESTERS,
                                if attesters_ready {
                                    metrics::SUCCESS
                                } else {
                                    metrics::FAILED
                                },
                            ],
                        );
                        metrics::inc_counter_vec(
                            &metrics::METADATA_SERVICE_POLL_TOTAL,
                            &[
                                metrics::SYNC,
                                if sync_ready {
                                    metrics::SUCCESS
                                } else {
                                    metrics::FAILED
                                },
                            ],
                        );
                        metrics::observe(
                            &metrics::METADATA_SERVICE_POLL_DURATION,
                            poll_duration.as_secs_f64(),
                        );

                        // Always proceed to build VotingAssignments regardless of poll results.
                        // Even if polls failed, the duties cache may have:
                        // - Fresh data from a previous poll this slot
                        // - Stale data from previous slot/epoch (better than nothing)
                        // - Empty data (only if poll timed out AND this is a fresh restart)
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

                        if let Err(err) = self_clone_phase2.update_voting_context().await {
                            error!(err, "Failed to update voting context")
                        } else {
                            trace!("Updated voting context");
                        }
                    } else {
                        error!("Failed to read slot clock");
                        sleep(slot_duration).await;
                    }
                }
            },
            "voting_context_service",
        );

        // ═══════════════════════════════════════════════════════════════════════
        // PHASE 3: AggregationAssignments (2/3 slot)
        // Re-fetches duties_service.attesters() after selection proofs are computed.
        // At this point, DutyAndProof.selection_proof.is_some() accurately indicates
        // is_aggregator for attestation duties.
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

                        if let Err(err) = self_clone_phase3.update_aggregation_assignments() {
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

    /// Phase 1: Build and publish VotingAssignments at slot start.
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

    /// Phase 2: Build and publish VotingContext at 1/3 slot.
    async fn update_voting_context(&self) -> Result<(), String> {
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

    /// Phase 3: Build and publish AggregationAssignments at 2/3 slot.
    fn update_aggregation_assignments(&self) -> Result<(), String> {
        let slot = self.slot_clock.now().ok_or("Failed to read slot clock")?;

        // Re-fetch attesters - `selection_proof.is_some()` means is_aggregator
        let (aggregating_attesters, aggregator_committees) = self
            .duties_service
            .attesters(slot)
            .into_iter()
            .filter(|duty_and_proof| duty_and_proof.selection_proof.is_some())
            .map(|duty_and_proof| {
                (
                    ValidatorIndex(duty_and_proof.duty.validator_index as usize),
                    (
                        duty_and_proof.duty.pubkey,
                        duty_and_proof.duty.committee_index,
                    ),
                )
            })
            .unzip();

        // Get sync aggregators from sync duties
        let sync_duties = self
            .duties_service
            .sync_duties
            .get_duties_for_slot::<E>(slot, &self.spec);

        // Build sync_aggregators_by_subnet
        let sync_aggregators_by_subnet = sync_duties
            .as_ref()
            .map(|duties| {
                let mut validator_subnets_map =
                    HashMap::<ValidatorIndex, HashSet<SyncSubnetId>>::new();
                duties
                    .aggregators
                    .iter()
                    .flat_map(|(subnet_id, aggregators)| {
                        aggregators.iter().map(move |(validator_index, _, _)| {
                            (ValidatorIndex(*validator_index as usize), *subnet_id)
                        })
                    })
                    .for_each(|(validator_index, subnet_id)| {
                        validator_subnets_map
                            .entry(validator_index)
                            .or_default()
                            .insert(subnet_id);
                    });
                validator_subnets_map
            })
            .unwrap_or_default();

        // Build multi_sync_aggregators - validators aggregating on multiple subnets need
        // coordination
        let multi_sync_aggregators = sync_duties
            .map(|duties| {
                let mut aggregators_by_validator = HashMap::new();
                for (_, aggregators) in duties.aggregators {
                    for (_, pk, _) in aggregators {
                        *aggregators_by_validator.entry(pk).or_insert(0) += 1;
                    }
                }
                aggregators_by_validator
                    .into_iter()
                    .filter(|(_, count)| *count > 1)
                    .map(|(pk, count)| (pk, ContributionWaiter::new(count)))
                    .collect()
            })
            .unwrap_or_default();

        let aggregator_info = AggregationAssignments {
            slot,
            aggregating_attesters,
            aggregator_committees,
            sync_aggregators_by_subnet,
            multi_sync_aggregators,
        };

        self.validator_store
            .update_aggregation_assignments(aggregator_info);

        trace!(%slot, "Published AggregationAssignments at 2/3 slot");
        Ok(())
    }
}
