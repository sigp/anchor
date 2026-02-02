use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use beacon_node_fallback::BeaconNodeFallback;
use slot_clock::SlotClock;
use ssv_types::{ValidatorIndex, consensus::BeaconVote};
use task_executor::TaskExecutor;
use tokio::time::sleep;
use tracing::{error, info, trace};
use types::{ChainSpec, EthSpec, SyncSubnetId};
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
        Self {
            duties_service,
            validator_store,
            slot_clock,
            beacon_nodes,
            executor,
            spec,
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
