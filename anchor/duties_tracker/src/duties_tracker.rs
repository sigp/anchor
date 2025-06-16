use std::{future::Future, sync::Arc};

use beacon_node_fallback::BeaconNodeFallback;
use bls::PublicKeyBytes;
use database::NetworkState;
use safe_arith::ArithError;
use slot_clock::SlotClock;
use ssv_types::ValidatorIndex;
use task_executor::TaskExecutor;
use thiserror::Error;
use tokio::{sync::watch, time::sleep};
use tracing::{debug, error, trace, warn};
use types::{ChainSpec, Epoch, Slot};

use crate::{Duties, DutiesProvider, voluntary_exit_tracker::VoluntaryExitTracker};

/// Only retain `HISTORICAL_DUTIES_EPOCHS` duties prior to the current epoch.
const HISTORICAL_DUTIES_EPOCHS: u64 = 2;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Unable to read the slot clock")]
    UnableToReadSlotClock,
    #[error("Arithmetic error")]
    Arith(#[allow(dead_code)] ArithError),
    #[error("Failed to poll proposers: {0}")]
    FailedToPollProposers(String),
}

pub struct DutiesTracker<T: SlotClock + 'static> {
    /// Duties data structures
    duties: Duties,
    /// The voluntary exit tracker
    voluntary_exit_tracker: Arc<VoluntaryExitTracker>,
    /// The beacon node fallback clients
    beacon_nodes: Arc<BeaconNodeFallback<T>>,
    /// The chain spec
    spec: Arc<ChainSpec>,
    /// The number of slots per epoch
    slots_per_epoch: u64,
    /// The slot clock.
    slot_clock: T,
    /// The network state receiver.
    network_state_rx: watch::Receiver<NetworkState>,
}

impl<T: SlotClock + 'static> DutiesTracker<T> {
    pub fn new(
        voluntary_exit_tracker: Arc<VoluntaryExitTracker>,
        beacon_nodes: Arc<BeaconNodeFallback<T>>,
        spec: Arc<ChainSpec>,
        slots_per_epoch: u64,
        slot_clock: T,
        network_state_rx: watch::Receiver<NetworkState>,
    ) -> Self {
        Self {
            duties: Duties::new(),
            voluntary_exit_tracker,
            beacon_nodes,
            spec,
            slots_per_epoch,
            slot_clock,
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

            // Prune previous duties.
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
        validator_indices: &[u64],
        sync_committee_period: u64,
    ) -> Result<(), Error> {
        if validator_indices.is_empty() {
            debug!(
                sync_committee_period,
                "No validators, not polling for sync committee duties"
            );
            return Ok(());
        }

        debug!(
            sync_committee_period,
            num_validators = validator_indices.len(),
            "Fetching sync committee duties"
        );

        let period_start_epoch = self.spec.epochs_per_sync_committee_period * sync_committee_period;

        let duties_response = self
            .beacon_nodes
            .first_success(|beacon_node| async move {
                beacon_node
                    .post_validator_duties_sync(period_start_epoch, validator_indices)
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

        // Get or create the HashSet for this committee period
        let mut validators = self
            .duties
            .sync_duties
            .committees
            .entry(sync_committee_period)
            .or_default();

        // Insert only validators that have duties
        for duty in duties {
            debug!(
                validator_index = duty.validator_index,
                sync_committee_period, "Validator in sync committee"
            );

            // Insert the validator index
            validators.insert(duty.validator_index);
        }

        Ok(())
    }

    /// Download the proposer duties for the current epoch.
    async fn poll_beacon_proposers(&self) -> Result<(), Error> {
        let current_slot = self.slot_clock.now().ok_or(Error::UnableToReadSlotClock)?;
        let current_epoch = current_slot.epoch(self.slots_per_epoch);

        let download_result = self
            .beacon_nodes
            .first_success(|beacon_node| async move {
                beacon_node
                    .get_validator_duties_proposer(current_epoch)
                    .await
            })
            .await;

        let result = match download_result {
            Ok(response) => {
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

                trace!(
                    num_relevant_duties = relevant_duties.len(),
                    "Downloaded proposer duties"
                );

                self.duties
                    .proposers
                    .write()
                    .insert(current_epoch, relevant_duties);
                Ok(())
            }
            // Don't return early here, we"ll try again later
            Err(e) => Err(Error::FailedToPollProposers(e.to_string())),
        };

        // Prune old duties.
        self.duties
            .proposers
            .write()
            .retain(|&epoch, _| epoch + HISTORICAL_DUTIES_EPOCHS >= current_epoch);

        result
    }

    pub fn start(self: Arc<Self>, executor: TaskExecutor) {
        let self_clone = self.clone();
        self_clone.spawn_polling_task(
            |tracker| {
                let tracker = tracker.clone();
                async move { tracker.poll_sync_committee_duties().await }
            },
            "Failed to poll sync committee duties",
            "sync_committee_tracker",
            executor.clone(),
        );

        self.spawn_polling_task(
            |tracker| {
                let tracker = tracker.clone();
                async move { tracker.poll_beacon_proposers().await }
            },
            "Failed to poll beacon proposers",
            "proposers_tracker",
            executor,
        );
    }

    fn spawn_polling_task<F, Fut>(
        self: Arc<Self>,
        poll_fn: F,
        error_msg: &'static str,
        task_name: &'static str,
        executor: TaskExecutor,
    ) where
        F: Fn(Arc<Self>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), Error>> + Send + 'static,
    {
        let duties_tracker = self.clone();
        executor.spawn(
            async move {
                loop {
                    if let Err(e) = poll_fn(duties_tracker.clone()).await {
                        error!(
                            error = ?e,
                            error_msg
                        );
                    }

                    trace!(sync_committee = ?duties_tracker.duties.sync_duties);

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
            .map(|proposers| {
                proposers.iter().any(|proposer_data| {
                    proposer_data.slot == slot && proposer_data.validator_index == validator_index
                })
            })
            .unwrap_or_default()
    }

    fn get_voluntary_exit_duty_count(&self, slot: Slot, pubkey: &PublicKeyBytes) -> u64 {
        self.voluntary_exit_tracker.get_duty_count(slot, pubkey)
    }
}

/// Number of epochs to wait from the start of the period before actually fetching duties.
fn epoch_offset(spec: &ChainSpec) -> u64 {
    spec.epochs_per_sync_committee_period.as_u64() / 2
}
