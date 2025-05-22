use std::collections::{HashMap, HashSet};

use bls::PublicKeyBytes;
use dashmap::DashMap;
use eth2::types::ProposerData;
use parking_lot::RwLock;
use ssv_types::ValidatorIndex;
use types::{Epoch, Slot};

pub mod duties_tracker;
pub mod voluntary_exit_tracker;

/// Top-level data-structure containing sync duty information.
///
/// This data is structured using a `DashMap` which provides concurrent read/write access
/// with fine-grained locking at the entry level. This allows multiple threads to access
/// different entries without blocking each other.
///
/// Key benefits of using DashMap over RwLock<HashMap>:
/// 1. Fine-grained locking at the individual entry level rather than the entire map
/// 2. Better performance in concurrent scenarios with many readers and occasional writers
/// 3. Simpler code that doesn't require explicit lock acquisition
///
/// The structure only stores validators that actually have sync committee duties, which
/// helps reduce memory usage compared to storing all validators and marking some as not
/// having duties.
#[derive(Debug)]
pub struct SyncCommitteePerPeriod {
    /// Map from sync committee period to validators that are members of that sync committee.
    /// Only validators with actual duties are stored in the HashSet for each period.
    committees: DashMap<u64, HashSet<u64>>,
}

impl SyncCommitteePerPeriod {
    fn new() -> Self {
        Self {
            committees: DashMap::new(),
        }
    }

    /// Check if duties are already known for all of the given validators for `committee_period`.
    fn all_duties_known(&self, committee_period: u64, validator_indices: &[u64]) -> bool {
        self.committees
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
            .retain(|period, _| *period >= current_sync_committee_period)
    }

    pub fn is_validator_in_sync_committee(
        &self,
        committee_period: u64,
        validator_index: u64,
    ) -> bool {
        self.committees
            .get(&committee_period)
            .is_some_and(|validator_indices| validator_indices.contains(&validator_index))
    }
}

type ProposerMap = HashMap<Epoch, Vec<ProposerData>>;

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

    fn get_voluntary_exit_duty_count(&self, slot: Slot, pubkey: &PublicKeyBytes) -> u64;
}
