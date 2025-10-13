use std::{collections::HashMap, sync::Arc, time::Duration};

use beacon_node_fallback::BeaconNodeFallback;
use slot_clock::SlotClock;
use ssv_types::{ValidatorIndex, consensus::BeaconVote};
use task_executor::TaskExecutor;
use tokio::time::sleep;
use tracing::{error, info, trace};
use types::{ChainSpec, EthSpec};
use validator_services::duties_service::DutiesService;

use crate::{AnchorValidatorStore, ContributionWaiter, SlotMetadata};

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

        let interval_fut = async move {
            loop {
                if let Some(duration_to_next_slot) = self.slot_clock.duration_to_next_slot() {
                    sleep(duration_to_next_slot + slot_duration / 3).await;

                    if let Err(err) = self.update_metadata().await {
                        error!(err, "Failed to update slot metadata")
                    } else {
                        trace!("Updated slot metadata");
                    }
                } else {
                    error!("Failed to read slot clock");
                    // If we can't read the slot clock, just wait another slot.
                    sleep(slot_duration).await;
                }
            }
        };

        executor.spawn(interval_fut, "metadata_service");
        Ok(())
    }

    async fn update_metadata(&self) -> Result<(), String> {
        let slot = self.slot_clock.now().ok_or("Failed to read slot clock")?;

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

        let (attesting_validator_indices, attesting_validator_committees) = self
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

        let sync_duties = self
            .duties_service
            .sync_duties
            .get_duties_for_slot::<E>(slot, &self.spec);

        let sync_validators = sync_duties
            .as_ref()
            .map(|duties| {
                duties
                    .duties
                    .iter()
                    .map(|duty| ValidatorIndex(duty.validator_index as usize))
                    .collect()
            })
            .unwrap_or_default();

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

        let metadata = SlotMetadata {
            slot,
            beacon_vote,
            attesting_validator_indices,
            attesting_validator_committees,
            sync_validators,
            multi_sync_aggregators,
        };

        self.validator_store.update_slot_metadata(metadata);

        Ok(())
    }
}
