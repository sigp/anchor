use std::{future::Future, sync::Arc};

use beacon_node_fallback::BeaconNodeFallback;
use bls::PublicKeyBytes;
use database::NetworkState;
use eth2::types::ProposerData;
use safe_arith::ArithError;
use slot_clock::SlotClock;
use ssv_types::ValidatorIndex;
use task_executor::TaskExecutor;
use thiserror::Error;
use tokio::{sync::watch, time::sleep};
use tracing::{debug, error, trace, warn};
use types::{ChainSpec, Epoch, Slot};

use crate::{Duties, DutiesProvider, MembershipKey, voluntary_exit_tracker::VoluntaryExitTracker};

/// Only retain `HISTORICAL_DUTIES_EPOCHS` duties prior to the current epoch.
const HISTORICAL_DUTIES_EPOCHS: u64 = 2;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Unable to read the slot clock")]
    UnableToReadSlotClock,
    #[error("Arithmetic error")]
    Arith(ArithError),
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
        self.poll_missing_sync_committee_duties_for_period(
            validator_indices.as_slice(),
            current_sync_committee_period,
        )
        .await?;

        // If we're past the point in the current period where we should determine duties for the
        // next period and they are not yet known, then poll.
        if current_epoch.as_u64() % spec.epochs_per_sync_committee_period.as_u64()
            >= epoch_offset(spec)
        {
            self.poll_missing_sync_committee_duties_for_period(
                validator_indices.as_slice(),
                next_sync_committee_period,
            )
            .await?;
        }

        // Prune previous duties.
        sync_duties.prune(current_sync_committee_period);

        Ok(())
    }

    async fn poll_missing_sync_committee_duties_for_period(
        &self,
        validator_indices: &[u64],
        sync_committee_period: u64,
    ) -> Result<(), Error> {
        let missing_duties = self
            .duties
            .sync_duties
            .get_missing_indices_for_period(sync_committee_period, validator_indices);
        if !missing_duties.is_empty() {
            self.poll_sync_committee_duties_for_period(
                missing_duties.as_slice(),
                sync_committee_period,
            )
            .await?;
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

        for &validator_index in validator_indices {
            let has_duty = duties
                .iter()
                .any(|duty| duty.validator_index == validator_index);

            if has_duty {
                debug!(
                    validator_index,
                    sync_committee_period, "Validator in sync committee"
                );
            }

            // Insert the validator index
            self.duties.sync_duties.committee_membership.insert(
                MembershipKey {
                    committee_period: sync_committee_period,
                    validator_index,
                },
                has_duty,
            );
        }

        Ok(())
    }

    /// Download the proposer duties for the current epoch.
    async fn poll_beacon_proposers(&self) -> Result<(), Error> {
        let current_slot = self.slot_clock.now().ok_or(Error::UnableToReadSlotClock)?;
        let current_epoch = current_slot.epoch(self.slots_per_epoch);

        let mut fetched = Vec::with_capacity(2);
        for epoch in [current_epoch, current_epoch + 1] {
            let result = self
                .beacon_nodes
                .first_success(|beacon_node| async move {
                    beacon_node.get_validator_duties_proposer(epoch).await
                })
                .await
                .map(|response| response.data)
                .map_err(|e| Error::FailedToPollProposers(e.to_string()));
            fetched.push((epoch, result));
        }

        self.store_proposer_duties(current_epoch, fetched)
    }

    /// Stores each successfully-fetched epoch's complete proposer view independently, prunes stale
    /// epochs, and aggregates fetch failures. Split from the async fetch so the store contract —
    /// per-epoch inserts are independent, one epoch's failure never discards another's stored view,
    /// and a failure still propagates — is unit-testable without a beacon-node HTTP seam.
    fn store_proposer_duties(
        &self,
        current_epoch: Epoch,
        fetched: Vec<(Epoch, Result<Vec<ProposerData>, Error>)>,
    ) -> Result<(), Error> {
        let mut last_err = None;
        for (epoch, result) in fetched {
            match result {
                Ok(proposer_duties) => {
                    trace!(
                        num_proposer_duties = proposer_duties.len(),
                        "Downloaded proposer duties"
                    );

                    self.duties.proposers.write().insert(epoch, proposer_duties);
                }
                // Keep the other epoch's stored view; we'll retry this one next slot.
                Err(e) => {
                    last_err = Some(e);
                }
            }
        }

        // Prune old duties.
        self.duties
            .proposers
            .write()
            .retain(|&epoch, _| epoch + HISTORICAL_DUTIES_EPOCHS >= current_epoch);

        last_err.map_or(Ok(()), Err)
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

    fn proposer_assignment_at_slot(
        &self,
        slot: Slot,
        validator_pubkey: &PublicKeyBytes,
    ) -> Option<bool> {
        let epoch = slot.epoch(self.slots_per_epoch);
        self.duties.proposers.read().get(&epoch).map(|proposers| {
            proposers
                .iter()
                .any(|d| d.slot == slot && d.pubkey == *validator_pubkey)
        })
    }
}

/// Number of epochs to wait from the start of the period before actually fetching duties.
fn epoch_offset(spec: &ChainSpec) -> u64 {
    spec.epochs_per_sync_committee_period.as_u64() / 2
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use beacon_node_fallback::{ApiTopic, BeaconNodeFallback, CandidateBeaconNode, Config};
    use bls::{Keypair, PublicKeyBytes};
    use database::NetworkDatabase;
    use eth2::{BeaconNodeHttpClient, Timeouts, types::ProposerData};
    use openssl::rsa::Rsa;
    use sensitive_url::SensitiveUrl;
    use slot_clock::{ManualSlotClock, SlotClock};
    use types::{ChainSpec, Epoch, Slot};

    use super::*;

    /// Slots per epoch used by these tests. Kept small and independent of any real fork schedule.
    const SLOTS_PER_EPOCH: u64 = 32;
    /// Genesis slot 0 anchor for the manual clock; the arm under test does not depend on wall time.
    const GENESIS_SLOT: u64 = 0;
    /// Slot duration for the manual clock; arbitrary, unused by the proposer-assignment arm.
    const SLOT_DURATION: Duration = Duration::from_secs(12);

    /// Builds a `DutiesTracker` whose `NetworkState` receiver comes from an EMPTY in-memory
    /// database (so `NetworkState::validator_indices()` is empty) and whose `BeaconNodeFallback`
    /// points at a never-contacted candidate. The tests here only exercise the read-side
    /// `DutiesProvider` methods over a directly-seeded proposers map, so the beacon node is never
    /// polled. An empty local validator set is exactly the condition under which #1142 must still
    /// serve the complete, unfiltered proposer view.
    fn tracker_with_empty_network_state() -> DutiesTracker<ManualSlotClock> {
        // Empty in-memory DB -> a `NetworkState` with no registered validators.
        let operator_pubkey = random_rsa_public_key();
        let db = NetworkDatabase::new_in_memory(&operator_pubkey, "test")
            .expect("in-memory database should be created");
        let network_state_rx = db.watch();
        // Guard the precondition this whole file relies on: no local validator indices.
        assert!(
            network_state_rx.borrow().validator_indices().is_empty(),
            "precondition: empty database must yield no local validator indices"
        );

        let slot_clock = ManualSlotClock::new(
            Slot::new(GENESIS_SLOT),
            Duration::from_secs(0),
            SLOT_DURATION,
        );

        let spec = Arc::new(ChainSpec::mainnet());
        let beacon_nodes = Arc::new(dummy_beacon_node_fallback(spec.clone()));
        let voluntary_exit_tracker = Arc::new(VoluntaryExitTracker::new());

        DutiesTracker::new(
            voluntary_exit_tracker,
            beacon_nodes,
            spec,
            SLOTS_PER_EPOCH,
            slot_clock,
            network_state_rx,
        )
    }

    /// A `BeaconNodeFallback` with a single candidate that is never actually contacted by these
    /// tests. It exists only to satisfy `DutiesTracker::new`.
    fn dummy_beacon_node_fallback(spec: Arc<ChainSpec>) -> BeaconNodeFallback<ManualSlotClock> {
        let url = SensitiveUrl::parse("http://127.0.0.1:0").expect("dummy url should parse");
        let http_client = BeaconNodeHttpClient::new(url, Timeouts::set_all(SLOT_DURATION));
        let candidate = CandidateBeaconNode::new(http_client, 0);
        BeaconNodeFallback::new(vec![candidate], Config::default(), ApiTopic::all(), spec)
    }

    /// Generates a throwaway RSA public key for the in-memory database's operator identity.
    fn random_rsa_public_key() -> Rsa<openssl::pkey::Public> {
        let private_key = Rsa::generate(2048).expect("RSA key generation should succeed");
        Rsa::from_public_components(
            private_key.n().to_owned().expect("modulus"),
            private_key.e().to_owned().expect("exponent"),
        )
        .expect("public RSA key should be reconstructable")
    }

    /// Generates a random validator public key for proposer entries.
    fn random_validator_pubkey() -> PublicKeyBytes {
        PublicKeyBytes::from(Keypair::random().pk)
    }

    /// Seeds `tracker.duties.proposers[epoch]` with `data` exactly as `poll_beacon_proposers`
    /// does (a single unfiltered `insert` of the whole `response.data`).
    fn seed_epoch(tracker: &DutiesTracker<ManualSlotClock>, epoch: Epoch, data: Vec<ProposerData>) {
        tracker.duties.proposers.write().insert(epoch, data);
    }

    /// Builds a `ProposerData` for the given slot/pubkey with an arbitrary validator index.
    fn proposer(slot: Slot, pubkey: PublicKeyBytes) -> ProposerData {
        ProposerData {
            pubkey,
            validator_index: 0,
            slot,
        }
    }

    // ==================== store_proposer_duties ====================

    #[test]
    fn test_store_proposer_duties_keeps_successful_epoch_and_propagates_other_failure() {
        // #1142: `store_proposer_duties` inserts each `Ok` epoch's proposer view independently and
        // aggregates failures via `last_err.map_or(Ok(()), Err)`. This test drives the REAL
        // store/aggregate path (not a directly-seeded map): one epoch succeeds, the other fails.
        // The contract is that the failure propagates as `Err` while the successful epoch is still
        // stored and queryable, and the failed epoch is neither stored nor fabricated.

        // Arrange: a tracker plus a fetched batch where `current_epoch` succeeds and `next_epoch`
        // fails.
        let tracker = tracker_with_empty_network_state();
        let current_epoch = Epoch::new(0);
        let next_epoch = current_epoch + 1;
        let current_slot = current_epoch.start_slot(SLOTS_PER_EPOCH);
        let pubkey = random_validator_pubkey();

        // Act: exercise the actual store/aggregate logic.
        let result = tracker.store_proposer_duties(
            current_epoch,
            vec![
                (current_epoch, Ok(vec![proposer(current_slot, pubkey)])),
                (
                    next_epoch,
                    Err(Error::FailedToPollProposers("boom".to_string())),
                ),
            ],
        );

        // Assert: the failed epoch's error propagates.
        assert!(
            result.is_err(),
            "a failed epoch in the batch must propagate as Err"
        );

        // Assert: the successful epoch was stored and survives despite the sibling failure.
        assert!(
            tracker.is_epoch_known_for_proposers(current_epoch),
            "the successfully stored epoch must be known"
        );
        assert_eq!(
            tracker.proposer_assignment_at_slot(current_slot, &pubkey),
            Some(true),
            "the successfully stored epoch's proposer must be served at its slot"
        );

        // Assert: the failed epoch was neither stored nor fabricated.
        assert!(
            !tracker.is_epoch_known_for_proposers(next_epoch),
            "the failed epoch must not be marked known"
        );
        assert_eq!(
            tracker.proposer_assignment_at_slot(next_epoch.start_slot(SLOTS_PER_EPOCH), &pubkey),
            None,
            "the failed epoch must report None (unknown), not a fabricated verdict"
        );
    }

    // ==================== proposer_assignment_at_slot ====================

    #[test]
    fn test_proposer_assignment_at_slot_returns_some_true_for_assigned_pubkey() {
        // Assigned pubkey AT its slot in a fetched epoch -> Some(true).
        let tracker = tracker_with_empty_network_state();
        let epoch = Epoch::new(0);
        let slot = epoch.start_slot(SLOTS_PER_EPOCH) + Slot::new(1);
        let assigned = random_validator_pubkey();
        seed_epoch(&tracker, epoch, vec![proposer(slot, assigned)]);

        assert_eq!(
            tracker.proposer_assignment_at_slot(slot, &assigned),
            Some(true),
            "assigned pubkey at its slot must return Some(true)"
        );
    }

    #[test]
    fn test_proposer_assignment_at_slot_returns_some_false_for_unassigned_pubkey_in_fetched_epoch()
    {
        // Different (unassigned) pubkey, same fetched epoch and slot -> Some(false).
        let tracker = tracker_with_empty_network_state();
        let epoch = Epoch::new(0);
        let slot = epoch.start_slot(SLOTS_PER_EPOCH) + Slot::new(1);
        let assigned = random_validator_pubkey();
        let other = random_validator_pubkey();
        seed_epoch(&tracker, epoch, vec![proposer(slot, assigned)]);

        assert_eq!(
            tracker.proposer_assignment_at_slot(slot, &other),
            Some(false),
            "unassigned pubkey in a fetched epoch must return Some(false)"
        );
    }

    #[test]
    fn test_proposer_assignment_at_slot_returns_some_false_for_assigned_pubkey_at_different_slot() {
        // The assignment is bound to the exact slot: the assigned pubkey queried at a DIFFERENT
        // slot within the SAME fetched epoch must return Some(false) (not Some(true)). This is the
        // slot-bind case that guards against matching on pubkey alone.
        let tracker = tracker_with_empty_network_state();
        let epoch = Epoch::new(0);
        let assigned_slot = epoch.start_slot(SLOTS_PER_EPOCH) + Slot::new(1);
        let other_slot = epoch.start_slot(SLOTS_PER_EPOCH) + Slot::new(2);
        let assigned = random_validator_pubkey();
        seed_epoch(&tracker, epoch, vec![proposer(assigned_slot, assigned)]);

        // Precondition: both slots are in the same fetched epoch.
        assert_eq!(assigned_slot.epoch(SLOTS_PER_EPOCH), epoch);
        assert_eq!(other_slot.epoch(SLOTS_PER_EPOCH), epoch);

        assert_eq!(
            tracker.proposer_assignment_at_slot(other_slot, &assigned),
            Some(false),
            "assigned pubkey queried at a different slot in the same epoch must return Some(false)"
        );
    }

    #[test]
    fn test_proposer_assignment_at_slot_returns_none_for_unfetched_epoch() {
        // A slot whose epoch has not been fetched -> None (unknown), regardless of pubkey.
        let tracker = tracker_with_empty_network_state();
        let fetched_epoch = Epoch::new(0);
        let pubkey = random_validator_pubkey();
        let fetched_slot = fetched_epoch.start_slot(SLOTS_PER_EPOCH);
        seed_epoch(
            &tracker,
            fetched_epoch,
            vec![proposer(fetched_slot, pubkey)],
        );

        // Query a slot in a DIFFERENT, unfetched epoch.
        let unfetched_slot = (fetched_epoch + 5).start_slot(SLOTS_PER_EPOCH);
        assert_eq!(
            tracker.proposer_assignment_at_slot(unfetched_slot, &pubkey),
            None,
            "a slot in an unfetched epoch must return None"
        );
    }
}
