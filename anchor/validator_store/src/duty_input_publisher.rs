//! Publishes duty inputs at scheduled times during each slot.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use beacon_node_fallback::BeaconNodeFallback;
use fork::{Fork, ForkSchedule};
use slot_clock::SlotClock;
use ssv_types::{ValidatorIndex, consensus::BeaconVote};
use task_executor::TaskExecutor;
use tokio::time::sleep;
use tracing::{error, trace};
use types::{ChainSpec, EthSpec};
use validator_services::duties_service::DutiesService;

use crate::{
    AggregationAssignments, AnchorValidatorStore, VotingAssignments,
    aggregator_consensus_builder::{
        VotingContext, build_consensus_data_for_all_committees, group_duties_by_committee,
    },
    metrics,
};

const BEACON_API_FETCH_TIMEOUT: Duration = Duration::from_secs(2);

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

        tracing::info!(
            next_update_millis = duration_to_next_slot.as_millis(),
            "Duty input publisher started"
        );

        let executor = self.executor.clone();

        // Phase 1: VotingAssignments (slot start)
        let self_clone_phase1 = self.clone();
        executor.spawn(
            async move {
                loop {
                    if let Some(duration_to_next_slot) =
                        self_clone_phase1.slot_clock.duration_to_next_slot()
                    {
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

        // Phase 2: VotingContext (1/3 slot)
        let self_clone_phase2 = self.clone();
        executor.spawn(
            async move {
                loop {
                    if let Some(duration_to_next_slot) =
                        self_clone_phase2.slot_clock.duration_to_next_slot()
                    {
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

        // Phase 3: AggregationAssignments (2/3 slot)
        let self_clone_phase3 = self.clone();
        executor.spawn(
            async move {
                loop {
                    if let Some(duration_to_next_slot) =
                        self_clone_phase3.slot_clock.duration_to_next_slot()
                    {
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

    fn publish_voting_assignments(&self) -> Result<(), String> {
        let slot = self.slot_clock.now().ok_or("Failed to read slot clock")?;

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

        trace!(%slot, attester_count, sync_count, "Published VotingAssignments");
        Ok(())
    }

    async fn publish_voting_context(&self) -> Result<(), String> {
        let slot = self.slot_clock.now().ok_or("Failed to read slot clock")?;

        let voting_assignments = self
            .validator_store
            .get_voting_assignments(slot)
            .await
            .map_err(|e| format!("Failed to get cached voting assignments: {:?}", e))?;

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

        trace!(%slot, "Published VotingContext");
        Ok(())
    }

    async fn publish_aggregation_assignments(&self) -> Result<(), String> {
        let slot = self.slot_clock.now().ok_or("Failed to read slot clock")?;

        let attesters = self.duties_service.attesters(slot);
        let sync_duties = self
            .duties_service
            .sync_duties
            .get_duties_for_slot::<E>(slot, &self.spec);

        let sync_aggregators = sync_duties.as_ref().map(|duties| &duties.aggregators);

        let grouped =
            group_duties_by_committee(&attesters, sync_aggregators, &self.validator_store);

        let epoch = slot.epoch(E::slots_per_epoch());
        let consensus_data_by_ssv_committee =
            if self.fork_schedule.active_fork(epoch) >= Fork::Boole {
                let voting_context = self
                    .validator_store
                    .get_voting_context(slot)
                    .await
                    .map_err(|e| format!("Failed to get voting context: {:?}", e))?;

                build_consensus_data_for_all_committees(
                    slot,
                    grouped.attesters_by_ssv_committee,
                    grouped.sync_by_ssv_committee,
                    grouped.attestation_committee_indexes,
                    grouped.all_subnet_ids,
                    &voting_context,
                    BEACON_API_FETCH_TIMEOUT,
                    &self.validator_store,
                    &self.beacon_nodes,
                    &self.spec,
                )
                .await?
            } else {
                HashMap::new()
            };

        let aggregator_info = AggregationAssignments {
            slot,
            aggregator_committees: grouped.aggregator_committees,
            multi_sync_aggregators: grouped.multi_sync_aggregators,
            consensus_data_by_ssv_committee,
        };

        self.validator_store
            .update_aggregation_assignments(aggregator_info);

        trace!(%slot, "Published AggregationAssignments");
        Ok(())
    }
}
