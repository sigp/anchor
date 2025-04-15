use std::sync::Arc;

use beacon_node_fallback::BeaconNodeFallback;
use database::NetworkState;
use safe_arith::ArithError;
use slot_clock::SlotClock;
use task_executor::TaskExecutor;
use tokio::{sync::watch, time::sleep};
use tracing::{debug, error, info, warn};
use types::ChainSpec;

use crate::duties::{Duties, ValidatorDuties};

#[derive(Debug)]
pub enum Error {
    UnableToReadSlotClock,
    FailedToDownloadAttesters(#[allow(dead_code)] String),
    InvalidModulo(#[allow(dead_code)] ArithError),
    Arith(#[allow(dead_code)] ArithError),
    SyncDutiesNotFound(#[allow(dead_code)] u64),
}

pub struct DutiesTracker<T: SlotClock + 'static> {
    /// The duties tracker.
    pub duties: Duties,
    ///
    pub beacon_nodes: Arc<BeaconNodeFallback<T>>,
    ///
    pub spec: Arc<ChainSpec>,
    ///
    slots_per_epoch: u64,
    /// The slot clock.
    pub slot_clock: Arc<T>,
    //
    /// The runtime for spawning tasks.
    pub executor: TaskExecutor,
    ///
    network_state_rx: watch::Receiver<NetworkState>,
}

impl<T: SlotClock + 'static> DutiesTracker<T> {
    pub async fn poll_sync_committee_duties(&self) -> Result<(), Error> {
        let sync_duties = &self.duties.sync_duties;
        let spec = &self.spec;
        let current_slot = self.slot_clock.now().ok_or(Error::UnableToReadSlotClock)?;
        let current_epoch = current_slot.epoch(self.slots_per_epoch);

        // If the Altair fork is yet to be activated, do not attempt to poll for duties.
        if spec
            .altair_fork_epoch
            .map_or(true, |altair_epoch| current_epoch < altair_epoch)
        {
            return Ok(());
        }

        let current_sync_committee_period = current_epoch
            .sync_committee_period(spec)
            .map_err(Error::Arith)?;
        let next_sync_committee_period = current_sync_committee_period + 1;

        // Clone the indices to avoid holding the borrow across .await points
        let local_indices = {
            let network_state = self.network_state_rx.borrow();
            network_state.validator_indices().clone()
        };

        // If duties aren't known for the current period, poll for them.
        if !sync_duties.all_duties_known(current_sync_committee_period, &local_indices) {
            self.poll_sync_committee_duties_for_period(
                &local_indices.as_slice(),
                current_sync_committee_period,
            )
            .await?;

            // Prune previous duties (we avoid doing this too often as it locks the whole map).
            sync_duties.prune(current_sync_committee_period);
        }

        // If we're past the point in the current period where we should determine duties for the
        // next period and they are not yet known, then poll.
        if current_epoch.as_u64() % spec.epochs_per_sync_committee_period.as_u64()
            >= epoch_offset(spec)
            && !sync_duties.all_duties_known(next_sync_committee_period, &local_indices)
        {
            self.poll_sync_committee_duties_for_period(&local_indices, next_sync_committee_period)
                .await?;

            // Prune (this is the main code path for updating duties, so we should almost always hit
            // this prune).
            sync_duties.prune(current_sync_committee_period);
        }

        Ok(())
    }

    pub async fn poll_sync_committee_duties_for_period(
        &self,
        local_indices: &[u64],
        sync_committee_period: u64,
    ) -> Result<(), Error> {
        // no local validators don't need to poll for sync committee
        if local_indices.is_empty() {
            debug!(
                sync_committee_period,
                "No validators, not polling for sync committee duties"
            );
            return Ok(());
        }

        debug!(
            sync_committee_period,
            num_validators = local_indices.len(),
            "Fetching sync committee duties"
        );

        let period_start_epoch = self.spec.epochs_per_sync_committee_period * sync_committee_period;

        let duties_response = self
            .beacon_nodes
            .first_success(|beacon_node| async move {
                // let _timer = validator_metrics::start_timer_vec(
                //     &validator_metrics::DUTIES_SERVICE_TIMES,
                //     &[validator_metrics::VALIDATOR_DUTIES_SYNC_HTTP_POST],
                // );
                beacon_node
                    .post_validator_duties_sync(period_start_epoch, local_indices)
                    .await
            })
            .await;

        let duties = match duties_response {
            Ok(res) => res.data,
            Err(e) => {
                warn!(
                    sync_committee_period,
                    error = %e,
                    "Failed to download sync committee duties"
                );
                return Ok(());
            }
        };

        debug!(count = duties.len(), "Fetched sync duties from BN");

        // Add duties to map.
        let committee_duties = self
            .duties
            .sync_duties
            .get_or_create_committee_duties(sync_committee_period, local_indices);

        let mut validator_writer = committee_duties.validators.write();
        for duty in duties {
            let validator_duties = validator_writer
                .get_mut(&duty.validator_index)
                .ok_or(Error::SyncDutiesNotFound(duty.validator_index))?;

            let updated = validator_duties.as_ref().map_or(true, |existing_duties| {
                let updated_due_to_reorg = existing_duties.duty.validator_sync_committee_indices
                    != duty.validator_sync_committee_indices;
                if updated_due_to_reorg {
                    warn!(
                        message = "this could be due to a really long re-org, or a bug",
                        "Sync committee duties changed"
                    );
                }
                updated_due_to_reorg
            });

            if updated {
                info!(
                    validator_index = duty.validator_index,
                    sync_committee_period, "Validator in sync committee"
                );

                *validator_duties = Some(ValidatorDuties::new(duty));
            }
        }

        Ok(())
    }

    pub fn start_update_service(self: Arc<Self>) {
        // Spawn the task which keeps track of local sync committee duties.
        let duties_tracker = self.clone();
        self.executor.spawn(
            async move {
                loop {
                    if let Err(e) = duties_tracker.poll_sync_committee_duties().await {
                        error!(
                            error = ?e,
                           "Failed to poll sync committee duties"
                        );
                    }

                    // Wait until the next slot before polling again.
                    // This doesn't mean that the beacon node will get polled every slot
                    // as the sync duties service will return early if it deems it already has
                    // enough information.
                    if let Some(duration) = duties_tracker.slot_clock.duration_to_next_slot() {
                        sleep(duration).await;
                    } else {
                        // Just sleep for one slot if we are unable to read the system clock, this
                        // gives us an opportunity for the clock to
                        // eventually come good.
                        sleep(duties_tracker.slot_clock.slot_duration()).await;
                        continue;
                    }
                }
            },
            "duties_service_sync_committee",
        );
    }
}

/// Number of epochs to wait from the start of the period before actually fetching duties.
fn epoch_offset(spec: &ChainSpec) -> u64 {
    spec.epochs_per_sync_committee_period.as_u64() / 2
}
