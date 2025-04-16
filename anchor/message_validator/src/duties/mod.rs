use std::collections::{HashMap, HashSet};

use eth2::types::ProposerData;
use parking_lot::RwLock;
use ssv_types::ValidatorIndex;
use types::{Epoch, Hash256, Slot};

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
#[derive(Debug)]
pub struct SyncCommitteePerPeriod {
    /// Map from sync committee period to members of that sync committee.
    committees: RwLock<HashMap<u64, HashSet<u64>>>,
}

impl SyncCommitteePerPeriod {
    fn new() -> Self {
        Self {
            committees: RwLock::new(HashMap::new()),
        }
    }

    /// Check if duties are already known for all of the given validators for `committee_period`.
    fn all_duties_known(&self, committee_period: u64, validator_indices: &[u64]) -> bool {
        self.committees
            .read()
            .get(&committee_period)
            .is_some_and(|validators| {
                validator_indices
                    .iter()
                    .all(|index| validators.contains(index))
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
            .is_some_and(|validator_indices| validator_indices.contains(&validator_index))
    }
}

/// To assist with readability, the dependent root for attester/proposer duties.
type DependentRoot = Hash256;

type ProposerMap = HashMap<Epoch, (DependentRoot, Vec<ProposerData>)>;

#[derive(Debug)]
pub struct Duties {
    /// Maps an epoch to all *local* proposers in this epoch. Notably, this does not contain
    /// proposals for any validators which are not registered locally.
    pub proposers: RwLock<ProposerMap>,
    /// Map from validator index to sync committee duties.
    pub sync_duties: SyncCommitteePerPeriod,
}

impl Duties {
    pub fn new() -> Self {
        Self {
            proposers: RwLock::new(HashMap::new()),
            sync_duties: SyncCommitteePerPeriod::new(),
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
