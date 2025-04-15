use std::collections::HashMap;

use bls::PublicKeyBytes;
use eth2::types::ProposerData;
use parking_lot::{MappedRwLockReadGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use ssv_types::ValidatorIndex;
use types::{Epoch, Hash256, Slot, SyncDuty};

pub mod duties_tracker;

/// Top-level data-structure containing sync duty information.
///
/// This data is structured as a series of nested `HashMap`s wrapped in `RwLock`s. Fine-grained
/// locking is used to provide maximum concurrency for the different services reading and writing.
///
/// Deadlocks are prevented by:
///
/// 1. Hierarchical locking. It is impossible to lock an inner lock (e.g. `validators`) without
///    first locking its parent.
/// 2. One-at-a-time locking. For the innermost locks on the aggregator duties, all of the functions
///    in this file take care to only lock one validator at a time. We never hold a lock while
///    trying to obtain another one (hence no lock ordering issues).
pub struct SyncDutiesMap {
    /// Map from sync committee period to duties for members of that sync committee.
    committees: RwLock<HashMap<u64, CommitteeDuties>>,
}

impl SyncDutiesMap {
    fn new() -> Self {
        Self {
            committees: RwLock::new(HashMap::new()),
        }
    }

    fn get_or_create_committee_duties<'a, 'b>(
        &'a self,
        committee_period: u64,
        validator_indices: impl IntoIterator<Item = &'b u64>,
    ) -> MappedRwLockReadGuard<'a, CommitteeDuties> {
        let mut committees_writer = self.committees.write();

        committees_writer
            .entry(committee_period)
            .or_default()
            .init(validator_indices);

        // Return shared reference
        RwLockReadGuard::map(
            RwLockWriteGuard::downgrade(committees_writer),
            |committees_reader| &committees_reader[&committee_period],
        )
    }

    /// Check if duties are already known for all of the given validators for `committee_period`.
    fn all_duties_known(&self, committee_period: u64, validator_indices: &[u64]) -> bool {
        self.committees
            .read()
            .get(&committee_period)
            .is_some_and(|committee_duties| {
                let validator_duties = committee_duties.validators.read();
                validator_indices
                    .iter()
                    .all(|index| validator_duties.contains_key(index))
            })
    }

    /// Prune duties for past sync committee periods from the map.
    fn prune(&self, current_sync_committee_period: u64) {
        self.committees
            .write()
            .retain(|period, _| *period >= current_sync_committee_period)
    }

    pub fn is_validator_in_sync_committee(
        &self,
        committee_period: u64,
        validator_index: u64,
    ) -> bool {
        self.committees
            .read()
            .get(&committee_period)
            .is_some_and(|committee_duties| {
                committee_duties
                    .validators
                    .read()
                    .get(&validator_index)
                    .is_some_and(|duty| duty.is_some())
            })
    }
}

/// Duties for a single sync committee period.
#[derive(Default)]
pub struct CommitteeDuties {
    /// Map from validator index to validator duties.
    ///
    /// A `None` value indicates that the validator index is known *not* to be a member of the sync
    /// committee, while a `Some` indicates a known member. An absent value indicates that the
    /// validator index was not part of the set of local validators when the duties were fetched.
    /// This allows us to track changes to the set of local validators.
    validators: RwLock<HashMap<u64, Option<ValidatorDuties>>>,
}

impl CommitteeDuties {
    fn init<'b>(&mut self, validator_indices: impl IntoIterator<Item = &'b u64>) {
        validator_indices.into_iter().for_each(|validator_index| {
            self.validators
                .get_mut()
                .entry(*validator_index)
                .or_insert(None);
        })
    }
}

/// Duties for a single validator.
pub struct ValidatorDuties {
    /// The sync duty: including validator sync committee indices & pubkey.
    duty: SyncDuty,
}

impl ValidatorDuties {
    fn new(duty: SyncDuty) -> Self {
        Self { duty }
    }
}

/// To assist with readability, the dependent root for attester/proposer duties.
type DependentRoot = Hash256;

type AttesterMap = HashMap<PublicKeyBytes, HashMap<Epoch, DependentRoot>>;
type ProposerMap = HashMap<Epoch, (DependentRoot, Vec<ProposerData>)>;

pub struct Duties {
    /// Maps a validator public key to their duties for each epoch.
    pub attesters: RwLock<AttesterMap>,
    /// Maps an epoch to all *local* proposers in this epoch. Notably, this does not contain
    /// proposals for any validators which are not registered locally.
    pub proposers: RwLock<ProposerMap>,
    /// Map from validator index to sync committee duties.
    pub sync_duties: SyncDutiesMap,
}

impl Duties {
    pub fn new() -> Self {
        Self {
            attesters: RwLock::new(HashMap::new()),
            proposers: RwLock::new(HashMap::new()),
            sync_duties: SyncDutiesMap::new(),
        }
    }
}

impl Default for Duties {
    fn default() -> Self {
        Self::new()
    }
}

pub trait DutiesProvider: Sync + Send + 'static {
    fn is_validator_in_sync_committee(
        &self,
        committee_period: u64,
        validator_index: ValidatorIndex,
    ) -> bool;

    fn is_epoch_known_for_proposers(&self, epoch: Epoch) -> bool;

    fn is_validator_proposer_at_slot(&self, slot: Slot, validator_index: ValidatorIndex) -> bool;
}
