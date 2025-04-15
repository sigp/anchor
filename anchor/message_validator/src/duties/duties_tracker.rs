use std::{future::Future, sync::Arc};

use beacon_node_fallback::BeaconNodeFallback;
use database::NetworkState;
use safe_arith::ArithError;
use slot_clock::SlotClock;
use ssv_types::ValidatorIndex;
use task_executor::TaskExecutor;
use tokio::{sync::watch, time::sleep};
use tracing::{debug, error, info, warn};
use types::{ChainSpec, Epoch, Slot};

use crate::duties::{Duties, DutiesProvider, ValidatorDuties};

/// Only retain `HISTORICAL_DUTIES_EPOCHS` duties prior to the current epoch.
const HISTORICAL_DUTIES_EPOCHS: u64 = 2;

#[derive(Debug)]
pub enum Error {
    UnableToReadSlotClock,
    Arith(#[allow(dead_code)] ArithError),
    SyncDutiesNotFound(#[allow(dead_code)] u64),
}

pub struct DutiesTracker<T: SlotClock + 'static> {
    /// Duties data structures
    pub duties: Duties,
    /// The beacon node fallback clients
    pub beacon_nodes: Arc<BeaconNodeFallback<T>>,
    pub spec: Arc<ChainSpec>,
    slots_per_epoch: u64,
    /// The slot clock.
    pub slot_clock: T,
    /// The runtime for spawning tasks.
    pub executor: TaskExecutor,
    network_state_rx: watch::Receiver<NetworkState>,
}

impl<T: SlotClock + 'static> DutiesTracker<T> {
    pub fn new(
        beacon_nodes: Arc<BeaconNodeFallback<T>>,
        spec: Arc<ChainSpec>,
        slots_per_epoch: u64,
        slot_clock: T,
        executor: TaskExecutor,
        network_state_rx: watch::Receiver<NetworkState>,
    ) -> Self {
        Self {
            duties: Duties::new(),
            beacon_nodes,
            spec,
            slots_per_epoch,
            slot_clock,
            executor,
            network_state_rx,
        }
    }

    async fn poll_sync_committee_duties(&self) -> Result<(), Error> {
        let sync_duties = &self.duties.sync_duties;
        let spec = &self.spec;
        let current_slot = self.slot_clock.now().ok_or(Error::UnableToReadSlotClock)?;
        let current_epoch = current_slot.epoch(self.slots_per_epoch);

        // If the Altair fork is yet to be activated, do not attempt to poll for duties.
        if spec
            .altair_fork_epoch
            .is_none_or(|altair_epoch| current_epoch < altair_epoch)
        {
            return Ok(());
        }

        let current_sync_committee_period = current_epoch
            .sync_committee_period(spec)
            .map_err(Error::Arith)?;
        let next_sync_committee_period = current_sync_committee_period + 1;

        // avoid holding the borrow across .await points
        let validator_indices = {
            let network_state = self.network_state_rx.borrow();
            network_state.validator_indices()
        };

        // If duties aren't known for the current period, poll for them.
        if !sync_duties.all_duties_known(current_sync_committee_period, &validator_indices) {
            self.poll_sync_committee_duties_for_period(
                validator_indices.as_slice(),
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
            && !sync_duties.all_duties_known(next_sync_committee_period, &validator_indices)
        {
            self.poll_sync_committee_duties_for_period(
                &validator_indices,
                next_sync_committee_period,
            )
            .await?;

            // Prune (this is the main code path for updating duties, so we should almost always hit
            // this prune).
            sync_duties.prune(current_sync_committee_period);
        }

        Ok(())
    }

    async fn poll_sync_committee_duties_for_period(
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

            let updated = validator_duties.as_ref().is_none_or(|existing_duties| {
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

    /// Download the proposer duties for the current epoch and store them in
    /// `duties_service.proposers`. If there are any proposer for this slot, send out a
    /// notification to the block proposers.
    ///
    /// ## Note
    ///
    /// This function will potentially send *two* notifications to the `BlockService`; it will send
    /// a notification initially, then it will download the latest duties and send a *second*
    /// notification if those duties have changed. This behaviour simultaneously achieves the
    /// following:
    ///
    /// 1. Block production can happen immediately and does not have to wait for the proposer duties
    ///    to download.
    /// 2. We won't miss a block if the duties for the current slot happen to change with this poll.
    ///
    /// This sounds great, but is it safe? Firstly, the additional notification will only contain
    /// block producers that were not included in the first notification. This should be safe
    /// enough. However, we also have the slashing protection as a second line of defence. These
    /// two factors provide an acceptable level of safety.
    ///
    /// It's important to note that since there is a 0-epoch look-ahead (i.e., no look-ahead) for
    /// block proposers then it's very likely that a proposal for the first slot of the epoch
    /// will need go through the slow path every time. I.e., the proposal will only happen after
    /// we've been able to download and process the duties from the BN. This means it is very
    /// important to ensure this function is as fast as possible.
    async fn poll_beacon_proposers(&self) -> Result<(), Error> {
        // let _timer = validator_metrics::start_timer_vec(
        //     &validator_metrics::DUTIES_SERVICE_TIMES,
        //     &[validator_metrics::UPDATE_PROPOSERS],
        // );

        let current_slot = self.slot_clock.now().ok_or(Error::UnableToReadSlotClock)?;
        let current_epoch = current_slot.epoch(self.slots_per_epoch);

        let download_result = self
            .beacon_nodes
            .first_success(|beacon_node| async move {
                // let _timer = validator_metrics::start_timer_vec(
                //     &validator_metrics::DUTIES_SERVICE_TIMES,
                //     &[validator_metrics::PROPOSER_DUTIES_HTTP_GET],
                // );
                beacon_node
                    .get_validator_duties_proposer(current_epoch)
                    .await
            })
            .await;

        match download_result {
            Ok(response) => {
                let dependent_root = response.dependent_root;

                // avoid holding the borrow across .await points
                let validator_indices = {
                    let network_state = self.network_state_rx.borrow();
                    network_state.validator_indices()
                };

                let relevant_duties = response
                    .data
                    .into_iter()
                    .filter(|proposer_duty| {
                        validator_indices.contains(&proposer_duty.validator_index)
                    })
                    .collect::<Vec<_>>();

                debug!(
                    %dependent_root,
                    num_relevant_duties = relevant_duties.len(),
                    "Downloaded proposer duties"
                );

                if let Some((prior_dependent_root, _)) = self
                    .duties
                    .proposers
                    .write()
                    .insert(current_epoch, (dependent_root, relevant_duties))
                {
                    if dependent_root != prior_dependent_root {
                        warn!(
                            %prior_dependent_root,
                            %dependent_root,
                            msg = "this may happen from time to time",
                            "Proposer duties re-org"
                        )
                    }
                }
            }
            // Don't return early here, we still want to try and produce blocks using the cached
            // values.
            Err(e) => error!(
                err = %e,
                "Failed to download proposer duties"
            ),
        }

        // Prune old duties.
        self.duties
            .proposers
            .write()
            .retain(|&epoch, _| epoch + HISTORICAL_DUTIES_EPOCHS >= current_epoch);

        Ok(())
    }

    pub fn start(self: Arc<Self>) {
        let self_clone = self.clone();
        self_clone.spawn_polling_task(
            |tracker| {
                let tracker = tracker.clone();
                async move { tracker.poll_sync_committee_duties().await }
            },
            "Failed to poll sync committee duties",
            "sync_committee_tracker",
        );

        self.spawn_polling_task(
            |tracker| {
                let tracker = tracker.clone();
                async move { tracker.poll_beacon_proposers().await }
            },
            "Failed to poll beacon proposers",
            "proposers_tracker",
        );
    }

    fn spawn_polling_task<F, Fut>(
        self: Arc<Self>,
        poll_fn: F,
        error_msg: &'static str,
        task_name: &'static str,
    ) where
        F: Fn(Arc<Self>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), Error>> + Send + 'static,
    {
        let duties_tracker = self.clone();
        self.executor.spawn(
            async move {
                loop {
                    if let Err(e) = poll_fn(duties_tracker.clone()).await {
                        error!(
                            error = ?e,
                            error_msg
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
            task_name,
        );
    }
}

impl<T: SlotClock + 'static> DutiesProvider for DutiesTracker<T> {
    fn is_validator_in_sync_committee(
        &self,
        committee_period: u64,
        validator_index: ValidatorIndex,
    ) -> bool {
        self.duties
            .sync_duties
            .is_validator_in_sync_committee(committee_period, validator_index.into())
    }

    fn is_epoch_known_for_proposers(&self, epoch: Epoch) -> bool {
        self.duties.proposers.read().contains_key(&epoch)
    }

    fn is_validator_proposer_at_slot(&self, slot: Slot, validator_index: ValidatorIndex) -> bool {
        let epoch = slot.epoch(self.slots_per_epoch);
        let validator_index: u64 = validator_index.into();
        self.duties
            .proposers
            .read()
            .get(&epoch)
            .map(|(_, proposers)| {
                proposers.iter().any(|proposer_data| {
                    proposer_data.slot == slot && proposer_data.validator_index == validator_index
                })
            })
            .unwrap_or_default()
    }
}

/// Number of epochs to wait from the start of the period before actually fetching duties.
fn epoch_offset(spec: &ChainSpec) -> u64 {
    spec.epochs_per_sync_committee_period.as_u64() / 2
}
