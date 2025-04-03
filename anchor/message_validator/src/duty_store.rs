use dashmap::DashMap;
use ssv_types::ValidatorIndex;
use types::{Epoch, Slot};

#[derive(Clone)]
struct ProposerDuty {
    slot: Slot,
    validator_index: ValidatorIndex,
}

#[derive(Clone)]
struct StoreDuty {
    slot: Slot,
    validator_index: ValidatorIndex,
    duty: ProposerDuty,
}

#[derive(Clone)]
struct StoreSyncCommitteeDuty {
    validator_index: ValidatorIndex,
    in_committee: bool,
}

// A comprehensive thread-safe duty store using DashMap
pub struct DutyStore {
    // Proposer duties map: epoch -> slot -> validator_index -> duty
    proposer_duties: DashMap<Epoch, DashMap<Slot, DashMap<ValidatorIndex, StoreDuty>>>,

    // Sync committee duties map: period -> validator_index -> duty
    sync_committee_duties: DashMap<Epoch, DashMap<ValidatorIndex, StoreSyncCommitteeDuty>>,
}

impl DutyStore {
    fn new() -> Self {
        Self {
            proposer_duties: DashMap::new(),
            sync_committee_duties: DashMap::new(),
        }
    }

    // Proposer duties methods
    pub(crate) fn is_epoch_set(&self, epoch: Epoch) -> bool {
        self.proposer_duties.contains_key(&epoch)
    }

    pub(crate) fn validator_has_duty_at_slot(
        &self,
        epoch: Epoch,
        slot: Slot,
        validator_index: ValidatorIndex,
    ) -> bool {
        if let Some(epoch_duties) = self.proposer_duties.get(&epoch) {
            if let Some(slot_duties) = epoch_duties.get(&slot) {
                return slot_duties.contains_key(&validator_index);
            }
        }
        false
    }

    // Sync committee methods
    pub(crate) fn validator_in_sync_committee(
        &self,
        period: Epoch,
        validator_index: ValidatorIndex,
    ) -> bool {
        if let Some(period_duties) = self.sync_committee_duties.get(&period) {
            if let Some(duty) = period_duties.get(&validator_index) {
                return duty.in_committee;
            }
        }
        false
    }

    // Method to set sync committee duties
    fn set_sync_committee_duties(&self, period: Epoch, duties: Vec<StoreSyncCommitteeDuty>) {
        let period_map = self
            .sync_committee_duties
            .entry(period)
            .or_insert_with(DashMap::new);

        for duty in duties {
            period_map.insert(duty.validator_index, duty);
        }
    }

    // Method to reset sync committee duties for a period
    fn reset_sync_committee_period(&self, period: Epoch) {
        self.sync_committee_duties.remove(&period);
    }
}
