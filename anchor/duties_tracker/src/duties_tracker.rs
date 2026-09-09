use std::{future::Future, sync::Arc};

use beacon_node_fallback::BeaconNodeFallback;
use bls::PublicKeyBytes;
use database::NetworkState;
use eth2::types::{DutiesResponse, ProposerData};
use safe_arith::ArithError;
use slot_clock::SlotClock;
use ssv_types::ValidatorIndex;
use task_executor::TaskExecutor;
use thiserror::Error;
use tokio::{sync::watch, time::sleep};
use tracing::{debug, error, trace, warn};
use types::{ChainSpec, Epoch, Slot};

use crate::{
    Duties, DutiesProvider, DutyAssignment, MembershipKey, ProposerSchedule, ProposerScheduleError,
    PtcSchedule, PtcScheduleError, voluntary_exit_tracker::VoluntaryExitTracker,
};

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
    #[error("Failed to poll PTC duties: {0}")]
    FailedToPollPtc(String),
    #[error("Invalid PTC duties: {0}")]
    InvalidPtcDuties(#[from] PtcScheduleError),
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

    /// Download the proposer duties for the current and next epoch, retaining only
    /// responses that validate as complete schedules.
    async fn poll_beacon_proposers(&self) -> Result<(), Error> {
        let current_slot = self.slot_clock.now().ok_or(Error::UnableToReadSlotClock)?;
        let current_epoch = current_slot.epoch(self.slots_per_epoch);

        let mut last_err = None;
        for epoch in [current_epoch, current_epoch + 1] {
            match self
                .beacon_nodes
                .first_success(|beacon_node| async move {
                    beacon_node.get_validator_duties_proposer_v2(epoch).await
                })
                .await
            {
                Ok(response) => {
                    trace!(
                        num_proposer_duties = response.data.len(),
                        "Downloaded proposer duties"
                    );

                    if let Err(e) = self.install_proposer_schedule(epoch, response) {
                        warn!(
                            %epoch,
                            error = %e,
                            "Discarding malformed proposer duties; retaining prior view"
                        );
                    }
                }
                Err(e) => last_err = Some(Error::FailedToPollProposers(e.to_string())),
            };
        }

        self.duties
            .proposers
            .write()
            .retain(|&epoch, _| epoch + HISTORICAL_DUTIES_EPOCHS >= current_epoch);

        last_err.map_or(Ok(()), Err)
    }

    /// Validate `response` and install it as the schedule for `epoch`, replacing any schedule
    /// already retained. A response that fails validation is discarded, leaving the previously
    /// retained schedule in place.
    fn install_proposer_schedule(
        &self,
        epoch: Epoch,
        response: DutiesResponse<Vec<ProposerData>>,
    ) -> Result<(), ProposerScheduleError> {
        let schedule = ProposerSchedule::from_response(epoch, self.slots_per_epoch, response)?;
        self.duties.proposers.write().insert(epoch, schedule);
        Ok(())
    }

    /// Replace the current PTC view using exactly the indices captured for this request.
    async fn poll_ptc_duties(&self) -> Result<(), Error> {
        let current_slot = self.slot_clock.now().ok_or(Error::UnableToReadSlotClock)?;
        let current_epoch = current_slot.epoch(self.slots_per_epoch);

        // Retain one previous epoch for PTC messages arriving after an epoch boundary.
        self.duties
            .ptc
            .write()
            .retain(|&epoch, _| epoch >= current_epoch.saturating_sub(1u64));

        if self
            .spec
            .gloas_fork_epoch
            .is_none_or(|gloas_epoch| current_epoch < gloas_epoch)
        {
            return Ok(());
        }

        // Release the database watch borrow before HTTP. Later additions remain unknown
        // until a subsequent request includes them.
        let validator_indices = self.network_state_rx.borrow().validator_indices();
        if validator_indices.is_empty() {
            self.duties.ptc.write().remove(&current_epoch);
            return Ok(());
        }

        let response = self
            .beacon_nodes
            .first_success(|beacon_node| {
                let indices = &validator_indices;
                async move {
                    beacon_node
                        .post_validator_duties_ptc(current_epoch, indices)
                        .await
                }
            })
            .await
            .map_err(|error| Error::FailedToPollPtc(error.to_string()))?;

        let schedule = PtcSchedule::from_response(
            current_epoch,
            self.slots_per_epoch,
            &validator_indices,
            response,
        )?;
        self.duties.ptc.write().insert(current_epoch, schedule);
        Ok(())
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

        if self.spec.gloas_fork_epoch.is_some() {
            self.clone().spawn_polling_task(
                |tracker| async move { tracker.poll_ptc_duties().await },
                "Failed to poll PTC duties",
                "ptc_tracker",
                executor.clone(),
            );
        }

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
            .map(|schedule| {
                schedule.duties().iter().any(|proposer_data| {
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
    ) -> DutyAssignment {
        let epoch = slot.epoch(self.slots_per_epoch);
        match self.duties.proposers.read().get(&epoch) {
            Some(schedule) => {
                if schedule
                    .duties()
                    .iter()
                    .any(|d| d.slot == slot && d.pubkey == *validator_pubkey)
                {
                    DutyAssignment::Assigned
                } else {
                    DutyAssignment::NotAssigned
                }
            }
            None => DutyAssignment::Unknown,
        }
    }

    fn ptc_assignment_at_slot(
        &self,
        slot: Slot,
        validator_index: ValidatorIndex,
    ) -> DutyAssignment {
        self.duties
            .ptc
            .read()
            .get(&slot.epoch(self.slots_per_epoch))
            .map_or(DutyAssignment::Unknown, |schedule| {
                schedule.assignment_at_slot(slot, validator_index.into())
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
    use bls::{FixedBytesExtended, Keypair, PublicKeyBytes};
    use database::NetworkDatabase;
    use eth2::{
        BeaconNodeHttpClient, Timeouts,
        types::{DutiesResponse, ProposerData},
    };
    use openssl::rsa::Rsa;
    use sensitive_url::SensitiveUrl;
    use slot_clock::{ManualSlotClock, SlotClock};
    use types::{ChainSpec, Epoch, Hash256, Slot};

    use super::*;
    use crate::ProposerScheduleError::{DuplicateSlot, SlotOutOfEpoch, WrongLength};

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

    /// Seeds `tracker.duties.proposers[epoch]` with `data` by wrapping the raw duties in a
    /// `ProposerSchedule` via the test-only `from_duties_unchecked` ctor. This deliberately
    /// bypasses the completeness/validity checks that `from_response` enforces, so the
    /// `proposer_assignment_at_slot` tests below can seed intentionally partial schedules that a
    /// validated response could never produce.
    fn seed_epoch(tracker: &DutiesTracker<ManualSlotClock>, epoch: Epoch, data: Vec<ProposerData>) {
        tracker
            .duties
            .proposers
            .write()
            .insert(epoch, ProposerSchedule::from_duties_unchecked(data));
    }

    /// Builds a `ProposerData` for the given slot/pubkey with an arbitrary validator index.
    fn proposer(slot: Slot, pubkey: PublicKeyBytes) -> ProposerData {
        ProposerData {
            pubkey,
            validator_index: 0,
            slot,
        }
    }

    /// Epoch used by the `from_response` matrix. Deliberately NON-ZERO: at epoch 0 the epoch's
    /// start slot is 0, so a regression that forgot to rebase slots onto the epoch
    /// (`slot.checked_sub(start_slot)`) would still pass. Epoch 3 forces `start_slot == 96`,
    /// so any such bug turns these tests RED.
    const RESPONSE_EPOCH: u64 = 3;

    /// Distinctive, clearly non-zero `dependent_root` seed for the metadata-retention test. It
    /// differs from `from_duties_unchecked`'s `Hash256::zero()` default, so a dropped-metadata
    /// regression in `from_response` is observable rather than masked by a matching default.
    const DISTINCTIVE_ROOT_SEED: u64 = 0xDEAD_BEEF;

    /// Builds a COMPLETE schedule for `epoch`: exactly one `proposer` entry for every slot in
    /// `[start_slot, start_slot + SLOTS_PER_EPOCH)`, each with a fresh random pubkey. Entries are
    /// emitted in REVERSED slot order so acceptance tests prove `from_response` is
    /// order-independent (it must not assume the input is pre-sorted).
    fn complete_epoch_duties(epoch: Epoch) -> Vec<ProposerData> {
        let start = epoch.start_slot(SLOTS_PER_EPOCH).as_u64();
        (0..SLOTS_PER_EPOCH)
            .rev()
            .map(|offset| proposer(Slot::new(start + offset), random_validator_pubkey()))
            .collect()
    }

    /// Wraps `data` in a `DutiesResponse` carrying placeholder metadata (`Hash256::zero()` /
    /// `None`). Used only by the validation-matrix cases whose result (a `WrongLength`,
    /// `SlotOutOfEpoch`, or `DuplicateSlot` error) is decided before any metadata is read.
    fn duties_response(data: Vec<ProposerData>) -> DutiesResponse<Vec<ProposerData>> {
        DutiesResponse {
            dependent_root: Hash256::zero(),
            execution_optimistic: None,
            data,
        }
    }

    /// Mid-epoch offset of the slot the install tests overwrite; the exact value is arbitrary.
    const TARGET_SLOT_OFFSET: u64 = 7;

    /// The in-epoch slot whose proposer the install tests pin.
    fn target_slot(epoch: Epoch) -> Slot {
        epoch.start_slot(SLOTS_PER_EPOCH) + Slot::new(TARGET_SLOT_OFFSET)
    }

    /// Puts `pubkey` at `slot`, leaving every slot distinct and in-epoch.
    fn assign(
        mut data: Vec<ProposerData>,
        slot: Slot,
        pubkey: PublicKeyBytes,
    ) -> Vec<ProposerData> {
        data.iter_mut()
            .find(|duty| duty.slot == slot)
            .expect("slot must be present in the schedule")
            .pubkey = pubkey;
        data
    }

    /// One entry short of complete, so `from_response` rejects it as `WrongLength`.
    fn incomplete_epoch_duties(epoch: Epoch) -> Vec<ProposerData> {
        let mut data = complete_epoch_duties(epoch);
        data.pop();
        data
    }

    // ==================== proposer_assignment_at_slot ====================

    #[test]
    fn test_proposer_assignment_at_slot_returns_some_true_for_assigned_pubkey() {
        // Assigned pubkey AT its slot in a fetched epoch -> Assigned.
        let tracker = tracker_with_empty_network_state();
        let epoch = Epoch::new(0);
        let slot = epoch.start_slot(SLOTS_PER_EPOCH) + Slot::new(1);
        let assigned = random_validator_pubkey();
        seed_epoch(&tracker, epoch, vec![proposer(slot, assigned)]);

        assert_eq!(
            tracker.proposer_assignment_at_slot(slot, &assigned),
            DutyAssignment::Assigned,
            "assigned pubkey at its slot must return Assigned"
        );
    }

    #[test]
    fn test_proposer_assignment_at_slot_returns_some_false_for_unassigned_pubkey_in_fetched_epoch()
    {
        // Different (unassigned) pubkey, same fetched epoch and slot -> NotAssigned.
        let tracker = tracker_with_empty_network_state();
        let epoch = Epoch::new(0);
        let slot = epoch.start_slot(SLOTS_PER_EPOCH) + Slot::new(1);
        let assigned = random_validator_pubkey();
        let other = random_validator_pubkey();
        seed_epoch(&tracker, epoch, vec![proposer(slot, assigned)]);

        assert_eq!(
            tracker.proposer_assignment_at_slot(slot, &other),
            DutyAssignment::NotAssigned,
            "unassigned pubkey in a fetched epoch must return NotAssigned"
        );
    }

    #[test]
    fn test_proposer_assignment_at_slot_returns_some_false_for_assigned_pubkey_at_different_slot() {
        // The assignment is bound to the exact slot: the assigned pubkey queried at a DIFFERENT
        // slot within the SAME fetched epoch must return NotAssigned (not Assigned). This is the
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
            DutyAssignment::NotAssigned,
            "assigned pubkey queried at a different slot in the same epoch must return NotAssigned"
        );
    }

    #[test]
    fn test_proposer_assignment_at_slot_returns_none_for_unfetched_epoch() {
        // A slot whose epoch has not been fetched -> Unknown, at every slot and for any pubkey.
        let tracker = tracker_with_empty_network_state();
        let fetched_epoch = Epoch::new(0);
        let pubkey = random_validator_pubkey();
        let other = random_validator_pubkey();
        let fetched_slot = fetched_epoch.start_slot(SLOTS_PER_EPOCH);
        seed_epoch(
            &tracker,
            fetched_epoch,
            vec![proposer(fetched_slot, pubkey)],
        );

        // Query every slot in a DIFFERENT, unfetched epoch.
        let unfetched_start = (fetched_epoch + 5).start_slot(SLOTS_PER_EPOCH);
        for offset in 0..SLOTS_PER_EPOCH {
            let unfetched_slot = unfetched_start + Slot::new(offset);
            for queried in [&pubkey, &other] {
                assert_eq!(
                    tracker.proposer_assignment_at_slot(unfetched_slot, queried),
                    DutyAssignment::Unknown,
                    "slot {unfetched_slot} in an unfetched epoch must return Unknown"
                );
            }
        }
    }

    // ==================== ProposerSchedule::from_response ====================

    #[test]
    fn test_from_response_accepts_complete_unsorted_schedule() {
        // regression caught: a completeness/uniqueness check that assumes the input is pre-sorted
        // (e.g. compares the i-th duty's slot to `start_slot + i`) would reject this valid
        // schedule. Arrange: a full 32-entry epoch-3 schedule supplied in REVERSED slot
        // order.
        let epoch = Epoch::new(RESPONSE_EPOCH);
        let response = duties_response(complete_epoch_duties(epoch));

        // Act
        let schedule = ProposerSchedule::from_response(epoch, SLOTS_PER_EPOCH, response)
            .expect("a complete, in-epoch, duplicate-free schedule must be accepted");

        // Assert: exactly all SLOTS_PER_EPOCH duties are retained.
        assert_eq!(
            schedule.duties().len(),
            SLOTS_PER_EPOCH as usize,
            "an accepted schedule must retain exactly SLOTS_PER_EPOCH duties"
        );
    }

    #[test]
    fn test_from_response_retains_dependent_root_and_execution_optimistic() {
        // regression caught: `from_response` dropping or defaulting the response metadata (e.g.
        // constructing `Self` with `Hash256::zero()` / `None` instead of copying
        // `response.dependent_root` / `response.execution_optimistic`). The distinctive non-zero
        // root and `Some(true)` below differ from those defaults, so a dropped-metadata bug is
        // visible rather than masked. This MUST go through `from_response` (not the unchecked
        // ctor, which would make the assertion vacuous).
        // Arrange: a complete schedule carried by a response with DISTINCTIVE non-default metadata.
        let epoch = Epoch::new(RESPONSE_EPOCH);
        let dependent_root = Hash256::from_low_u64_be(DISTINCTIVE_ROOT_SEED);
        let response = DutiesResponse {
            dependent_root,
            execution_optimistic: Some(true),
            data: complete_epoch_duties(epoch),
        };

        // Act
        let schedule = ProposerSchedule::from_response(epoch, SLOTS_PER_EPOCH, response)
            .expect("a complete schedule must be accepted");

        // Assert: both metadata fields survive verbatim.
        assert_eq!(
            schedule.dependent_root(),
            dependent_root,
            "dependent_root must be copied through from_response verbatim"
        );
        assert_eq!(
            schedule.execution_optimistic(),
            Some(true),
            "execution_optimistic must be copied through from_response verbatim"
        );
    }

    #[test]
    fn test_from_response_rejects_too_short_schedule() {
        // regression caught: a length check using `<` / `>=` instead of `!=`, or dropped entirely,
        // would let a short (incomplete) schedule through as if it were complete.
        // Arrange: 31 entries (drop the last slot of the complete set).
        let epoch = Epoch::new(RESPONSE_EPOCH);
        let mut data = complete_epoch_duties(epoch);
        data.pop();

        // Act
        let result = ProposerSchedule::from_response(epoch, SLOTS_PER_EPOCH, duties_response(data));

        // Assert: exact variant AND payload. `unwrap_err` sidesteps the `Ok` type lacking
        // `PartialEq` while still pinning the error precisely.
        assert_eq!(
            result.unwrap_err(),
            WrongLength {
                expected: 32,
                actual: 31
            },
            "31 duties must be rejected as WrongLength {{ expected: 32, actual: 31 }}"
        );
    }

    #[test]
    fn test_from_response_rejects_too_long_schedule() {
        // regression caught: a length check missing its upper bound (only `len < expected`) would
        // accept an over-long schedule. WrongLength must also preempt the duplicate scan here.
        // Arrange: 33 entries — the complete 32 plus one extra (duplicated) in-range slot.
        let epoch = Epoch::new(RESPONSE_EPOCH);
        let mut data = complete_epoch_duties(epoch);
        data.push(proposer(
            epoch.start_slot(SLOTS_PER_EPOCH),
            random_validator_pubkey(),
        ));

        // Act
        let result = ProposerSchedule::from_response(epoch, SLOTS_PER_EPOCH, duties_response(data));

        // Assert: exact variant AND payload (length checked before any per-slot scan).
        assert_eq!(
            result.unwrap_err(),
            WrongLength {
                expected: 32,
                actual: 33
            },
            "33 duties must be rejected as WrongLength {{ expected: 32, actual: 33 }}"
        );
    }

    #[test]
    fn test_from_response_rejects_empty_schedule() {
        // regression caught: treating an empty response as a valid (vacuously complete) schedule.
        // Arrange: zero entries.
        let epoch = Epoch::new(RESPONSE_EPOCH);

        // Act
        let result =
            ProposerSchedule::from_response(epoch, SLOTS_PER_EPOCH, duties_response(Vec::new()));

        // Assert: exact variant AND payload.
        assert_eq!(
            result.unwrap_err(),
            WrongLength {
                expected: 32,
                actual: 0
            },
            "an empty schedule must be rejected as WrongLength {{ expected: 32, actual: 0 }}"
        );
    }

    #[test]
    fn test_from_response_rejects_slot_from_another_epoch() {
        // regression caught: a missing/incorrect epoch-bounds check — e.g. no upper `offset <
        // slots_per_epoch` guard, or a missing `checked_sub(start_slot)` rebase — that would admit
        // a duty slot belonging to a neighbouring epoch. The non-zero RESPONSE_EPOCH is essential:
        // at epoch 0 a missing rebase would be invisible.
        // Arrange: 32 entries — 31 distinct in-range slots plus exactly ONE slot from epoch 4.
        // That foreign slot is the ONLY defect, so `SlotOutOfEpoch` is the only reachable error
        // regardless of iteration order (the remaining 31 slots are distinct and in-range).
        let epoch = Epoch::new(RESPONSE_EPOCH);
        let foreign_slot = Epoch::new(RESPONSE_EPOCH + 1).start_slot(SLOTS_PER_EPOCH);
        let mut data = complete_epoch_duties(epoch);
        data[0].slot = foreign_slot;

        // Act
        let result = ProposerSchedule::from_response(epoch, SLOTS_PER_EPOCH, duties_response(data));

        // Assert: exact out-of-epoch slot AND epoch.
        assert_eq!(
            result.unwrap_err(),
            SlotOutOfEpoch(foreign_slot, epoch),
            "a duty slot from epoch 4 must be rejected as SlotOutOfEpoch(foreign_slot, epoch 3)"
        );
    }

    #[test]
    fn test_from_response_rejects_duplicate_slot_which_also_covers_missing_slot() {
        // regression caught: dropping the per-slot uniqueness (`seen[offset]`) check, which would
        // silently accept a schedule that duplicates one slot and (by pigeonhole) omits another.
        // There is no `MissingSlot` variant, and none is needed: with `len == 32`, all slots
        // in-range, and no duplicate, the schedule is necessarily complete — so a missing slot can
        // ONLY surface as a `DuplicateSlot`. This case therefore doubles as the missing-slot test.
        // Arrange: 32 entries, all in-range; overwrite one entry's slot with another's so
        // `dup_slot` appears twice and `missing_slot` is absent. Length stays 32 so
        // WrongLength cannot preempt.
        let epoch = Epoch::new(RESPONSE_EPOCH);
        let mut data = complete_epoch_duties(epoch);
        let dup_slot = data[0].slot;
        let missing_slot = data[1].slot;
        assert_ne!(
            dup_slot, missing_slot,
            "precondition: the duplicated and omitted slots must differ"
        );
        data[1].slot = dup_slot;

        // Act
        let result = ProposerSchedule::from_response(epoch, SLOTS_PER_EPOCH, duties_response(data));

        // Assert: exact duplicated slot. `dup_slot` is identical for whichever of the two matching
        // entries is scanned second, so the payload is deterministic regardless of iteration order.
        assert_eq!(
            result.unwrap_err(),
            DuplicateSlot(dup_slot),
            "a slot appearing twice (with another absent) must be rejected as DuplicateSlot(dup_slot)"
        );
    }

    // ==================== schedule install / transitions ====================

    #[test]
    fn test_install_replaces_previous_schedule() {
        // regression caught: a refresh that merges into, or skips over, an existing epoch entry
        let tracker = tracker_with_empty_network_state();
        let epoch = Epoch::new(RESPONSE_EPOCH);
        let slot = target_slot(epoch);
        let first = random_validator_pubkey();
        let second = random_validator_pubkey();

        tracker
            .install_proposer_schedule(
                epoch,
                duties_response(assign(complete_epoch_duties(epoch), slot, first)),
            )
            .expect("a complete schedule must install");
        tracker
            .install_proposer_schedule(
                epoch,
                duties_response(assign(complete_epoch_duties(epoch), slot, second)),
            )
            .expect("a complete schedule must install");

        assert_eq!(
            tracker.proposer_assignment_at_slot(slot, &second),
            DutyAssignment::Assigned,
            "the newly installed schedule must decide the slot"
        );
        assert_eq!(
            tracker.proposer_assignment_at_slot(slot, &first),
            DutyAssignment::NotAssigned,
            "the replaced schedule must no longer assign the slot"
        );
    }

    #[test]
    fn test_malformed_refresh_preserves_prior_schedule() {
        // regression caught: a rejected refresh clobbering or erasing the retained schedule
        let tracker = tracker_with_empty_network_state();
        let epoch = Epoch::new(RESPONSE_EPOCH);
        let slot = target_slot(epoch);
        let retained = random_validator_pubkey();
        let usurper = random_validator_pubkey();

        tracker
            .install_proposer_schedule(
                epoch,
                duties_response(assign(complete_epoch_duties(epoch), slot, retained)),
            )
            .expect("a complete schedule must install");

        let result = tracker.install_proposer_schedule(
            epoch,
            duties_response(assign(incomplete_epoch_duties(epoch), slot, usurper)),
        );

        assert_eq!(
            result.unwrap_err(),
            WrongLength {
                expected: 32,
                actual: 31
            },
            "a 31-duty refresh must be rejected as WrongLength {{ expected: 32, actual: 31 }}"
        );
        assert_eq!(
            tracker.proposer_assignment_at_slot(slot, &retained),
            DutyAssignment::Assigned,
            "the prior schedule must survive a rejected refresh"
        );
    }

    #[test]
    fn test_malformed_install_without_prior_leaves_epoch_unknown() {
        // regression caught: a rejected install still marking the epoch as fetched
        let tracker = tracker_with_empty_network_state();
        let epoch = Epoch::new(RESPONSE_EPOCH);
        let slot = target_slot(epoch);
        let pubkey = random_validator_pubkey();

        let result = tracker
            .install_proposer_schedule(epoch, duties_response(incomplete_epoch_duties(epoch)));

        assert_eq!(
            result.unwrap_err(),
            WrongLength {
                expected: 32,
                actual: 31
            },
            "a 31-duty install must be rejected as WrongLength {{ expected: 32, actual: 31 }}"
        );
        assert_eq!(
            tracker.proposer_assignment_at_slot(slot, &pubkey),
            DutyAssignment::Unknown,
            "a rejected install must leave the epoch unanswerable"
        );
        assert!(
            !tracker.is_epoch_known_for_proposers(epoch),
            "a rejected install must not mark the epoch as known"
        );
    }

    #[test]
    fn test_optimistic_metadata_does_not_weaken_authority() {
        // regression caught: `execution_optimistic` downgrading a complete schedule to Unknown
        let tracker = tracker_with_empty_network_state();
        let epoch = Epoch::new(RESPONSE_EPOCH);
        let slot = target_slot(epoch);
        let assigned = random_validator_pubkey();
        let other = random_validator_pubkey();
        let response = DutiesResponse {
            dependent_root: Hash256::zero(),
            execution_optimistic: Some(true),
            data: assign(complete_epoch_duties(epoch), slot, assigned),
        };

        tracker
            .install_proposer_schedule(epoch, response)
            .expect("a complete schedule must install regardless of its metadata");

        assert_eq!(
            tracker.proposer_assignment_at_slot(slot, &assigned),
            DutyAssignment::Assigned,
            "an optimistic schedule must still assign its proposer"
        );
        assert_eq!(
            tracker.proposer_assignment_at_slot(slot, &other),
            DutyAssignment::NotAssigned,
            "an optimistic schedule must still refute a non-proposer, never answer Unknown"
        );
    }

    #[test]
    fn test_dependent_root_change_replaces_view() {
        // regression caught: a reorged schedule kept alongside, or behind, the stale one
        let tracker = tracker_with_empty_network_state();
        let epoch = Epoch::new(RESPONSE_EPOCH);
        let slot = target_slot(epoch);
        let stale_root = Hash256::from_low_u64_be(DISTINCTIVE_ROOT_SEED);
        let reorged_root = Hash256::from_low_u64_be(DISTINCTIVE_ROOT_SEED + 1);
        let stale = random_validator_pubkey();
        let reorged = random_validator_pubkey();

        tracker
            .install_proposer_schedule(
                epoch,
                DutiesResponse {
                    dependent_root: stale_root,
                    execution_optimistic: None,
                    data: assign(complete_epoch_duties(epoch), slot, stale),
                },
            )
            .expect("a complete schedule must install");
        tracker
            .install_proposer_schedule(
                epoch,
                DutiesResponse {
                    dependent_root: reorged_root,
                    execution_optimistic: None,
                    data: assign(complete_epoch_duties(epoch), slot, reorged),
                },
            )
            .expect("a complete schedule must install");

        let stored_root = tracker
            .duties
            .proposers
            .read()
            .get(&epoch)
            .expect("the epoch must be retained")
            .dependent_root();
        assert_eq!(
            stored_root, reorged_root,
            "the retained schedule must carry the newest dependent_root"
        );
        assert_eq!(
            tracker.proposer_assignment_at_slot(slot, &reorged),
            DutyAssignment::Assigned,
            "the schedule from the newest dependent_root must decide the slot"
        );
    }

    #[test]
    fn test_complete_schedule_is_authoritative_for_non_local_validators() {
        // regression caught: filtering the installed schedule by the local validator registry
        let tracker = tracker_with_empty_network_state();
        let epoch = Epoch::new(RESPONSE_EPOCH);
        let slot = target_slot(epoch);
        let scheduled = random_validator_pubkey();
        let unrelated = random_validator_pubkey();

        tracker
            .install_proposer_schedule(
                epoch,
                duties_response(assign(complete_epoch_duties(epoch), slot, scheduled)),
            )
            .expect("a complete schedule must install");

        assert_eq!(
            tracker.proposer_assignment_at_slot(slot, &scheduled),
            DutyAssignment::Assigned,
            "a scheduled non-local validator must be Assigned at its slot"
        );
        assert_eq!(
            tracker.proposer_assignment_at_slot(slot, &unrelated),
            DutyAssignment::NotAssigned,
            "an unscheduled validator must be NotAssigned, never Unknown"
        );
    }
}

#[cfg(test)]
#[path = "ptc_tests.rs"]
mod ptc_tests;
