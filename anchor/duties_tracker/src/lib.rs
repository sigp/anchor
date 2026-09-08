use std::collections::HashMap;

use bls::PublicKeyBytes;
use dashmap::DashMap;
use eth2::types::{DutiesResponse, ProposerData, PtcDuty};
use parking_lot::RwLock;
use ssv_types::ValidatorIndex;
use thiserror::Error;
use types::{Epoch, Hash256, Slot};

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
    /// Map from sync committee period and validator index to whether the validator is in the sync
    /// committee for that period.
    committee_membership: DashMap<MembershipKey, bool>,
}

#[derive(Hash, Debug, Clone, Copy, Eq, PartialEq)]
struct MembershipKey {
    committee_period: u64,
    validator_index: u64,
}

impl SyncCommitteePerPeriod {
    fn new() -> Self {
        Self {
            committee_membership: DashMap::new(),
        }
    }

    /// Check if duties are already known for all of the given validators for `committee_period`.
    fn get_missing_indices_for_period(
        &self,
        committee_period: u64,
        validator_indices: &[u64],
    ) -> Vec<u64> {
        validator_indices
            .iter()
            .copied()
            .filter(|&validator_index| {
                !self.committee_membership.contains_key(&MembershipKey {
                    committee_period,
                    validator_index,
                })
            })
            .collect()
    }

    /// Prune duties for past sync committee periods from the map.
    fn prune(&self, current_sync_committee_period: u64) {
        self.committee_membership
            .retain(|key, _| key.committee_period >= current_sync_committee_period)
    }

    pub fn is_validator_in_sync_committee(
        &self,
        committee_period: u64,
        validator_index: u64,
    ) -> bool {
        self.committee_membership
            .get(&MembershipKey {
                committee_period,
                validator_index,
            })
            .as_deref()
            .copied()
            .unwrap_or(false)
    }
}

type ProposerMap = HashMap<Epoch, ProposerSchedule>;

/// A proposer schedule validated as complete for one epoch (one duty per slot),
/// retained with the v2 response metadata used for refresh decisions and diagnostics.
#[derive(Debug, Clone)]
pub struct ProposerSchedule {
    dependent_root: Hash256,
    execution_optimistic: Option<bool>,
    duties: Vec<ProposerData>,
}

#[derive(Debug, Error, PartialEq)]
pub enum ProposerScheduleError {
    #[error("expected {expected} proposer duties, got {actual}")]
    WrongLength { expected: usize, actual: usize },
    #[error("duty slot {0} is outside epoch {1}")]
    SlotOutOfEpoch(Slot, Epoch),
    #[error("slot {0} has more than one proposer duty")]
    DuplicateSlot(Slot),
}

impl ProposerSchedule {
    /// Validate a v2 proposer-duties `response` as a complete schedule for `epoch`.
    pub fn from_response(
        epoch: Epoch,
        slots_per_epoch: u64,
        response: DutiesResponse<Vec<ProposerData>>,
    ) -> Result<Self, ProposerScheduleError> {
        let expected = slots_per_epoch as usize;
        if response.data.len() != expected {
            return Err(ProposerScheduleError::WrongLength {
                expected,
                actual: response.data.len(),
            });
        }

        let start_slot = epoch.start_slot(slots_per_epoch).as_u64();
        let mut seen = vec![false; expected];
        for duty in &response.data {
            let offset = duty
                .slot
                .as_u64()
                .checked_sub(start_slot)
                .filter(|offset| *offset < slots_per_epoch)
                .ok_or(ProposerScheduleError::SlotOutOfEpoch(duty.slot, epoch))?
                as usize;
            if seen[offset] {
                return Err(ProposerScheduleError::DuplicateSlot(duty.slot));
            }
            seen[offset] = true;
        }

        Ok(Self {
            dependent_root: response.dependent_root,
            execution_optimistic: response.execution_optimistic,
            duties: response.data,
        })
    }

    pub fn duties(&self) -> &[ProposerData] {
        &self.duties
    }

    pub fn dependent_root(&self) -> Hash256 {
        self.dependent_root
    }

    pub fn execution_optimistic(&self) -> Option<bool> {
        self.execution_optimistic
    }
}

#[cfg(test)]
impl ProposerSchedule {
    /// Build a schedule from raw duties WITHOUT the completeness/validity checks that
    /// `from_response` enforces. Test-only: lets `proposer_assignment_at_slot` tests seed
    /// deliberately partial schedules that could never come from a validated response.
    pub(crate) fn from_duties_unchecked(duties: Vec<ProposerData>) -> Self {
        // Brings `Hash256::zero()` (a `FixedBytesExtended` method) into scope for this
        // test-only constructor without adding a non-test import to the crate.
        use bls::FixedBytesExtended;
        Self {
            dependent_root: Hash256::zero(),
            execution_optimistic: None,
            duties,
        }
    }
}

#[derive(Debug)]
pub struct Duties {
    /// Maps an epoch to its validated complete proposer schedule (the full schedule for
    /// every validator, not filtered by the local registry).
    pub proposers: RwLock<ProposerMap>,
    /// Map from validator index to sync committee duties.
    pub sync_duties: SyncCommitteePerPeriod,
    /// PTC snapshots include every queried index, including validators with no duty.
    pub(crate) ptc: RwLock<HashMap<Epoch, PtcSchedule>>,
}

impl Duties {
    pub fn new() -> Self {
        Self {
            proposers: RwLock::new(HashMap::new()),
            sync_duties: SyncCommitteePerPeriod::new(),
            ptc: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for Duties {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a validator holds a duty at a slot, as one atomic verdict over the stored duty view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DutyAssignment {
    /// A fetched view covering this validator assigns it at this slot.
    Assigned,
    /// A fetched view covering this validator proves it is not assigned at this slot.
    NotAssigned,
    /// No fetched view covers this validator for the slot's epoch.
    Unknown,
}

/// A sparse PTC response together with its exact requested-index coverage.
#[derive(Debug)]
pub(crate) struct PtcSchedule {
    assignments: HashMap<u64, Option<Slot>>,
}

#[derive(Debug, Error, PartialEq)]
pub enum PtcScheduleError {
    #[error("PTC duty slot {0} is outside epoch {1}")]
    SlotOutOfEpoch(Slot, Epoch),
    #[error("PTC response includes unrequested validator {0}")]
    UnrequestedValidator(u64),
    #[error("PTC response includes multiple duties for validator {0}")]
    DuplicateValidator(u64),
}

impl PtcSchedule {
    fn from_response(
        epoch: Epoch,
        slots_per_epoch: u64,
        requested_indices: &[u64],
        response: DutiesResponse<Vec<PtcDuty>>,
    ) -> Result<Self, PtcScheduleError> {
        let mut assignments: HashMap<_, _> = requested_indices
            .iter()
            .map(|&index| (index, None))
            .collect();
        for duty in response.data {
            if duty.slot.epoch(slots_per_epoch) != epoch {
                return Err(PtcScheduleError::SlotOutOfEpoch(duty.slot, epoch));
            }
            let assignment = assignments
                .get_mut(&duty.validator_index)
                .ok_or(PtcScheduleError::UnrequestedValidator(duty.validator_index))?;
            if assignment.replace(duty.slot).is_some() {
                return Err(PtcScheduleError::DuplicateValidator(duty.validator_index));
            }
        }
        Ok(Self { assignments })
    }

    fn assignment_at_slot(&self, slot: Slot, validator_index: u64) -> DutyAssignment {
        match self.assignments.get(&validator_index) {
            Some(Some(assigned_slot)) if *assigned_slot == slot => DutyAssignment::Assigned,
            Some(_) => DutyAssignment::NotAssigned,
            None => DutyAssignment::Unknown,
        }
    }
}

pub trait DutiesProvider: Sync + Send + 'static {
    fn is_validator_in_sync_committee(
        &self,
        committee_period: u64,
        validator_index: ValidatorIndex,
    ) -> bool;

    /// Whether a complete proposer schedule is retained for `epoch`.
    fn is_epoch_known_for_proposers(&self, epoch: Epoch) -> bool;

    fn is_validator_proposer_at_slot(&self, slot: Slot, validator_index: ValidatorIndex) -> bool;

    fn get_voluntary_exit_duty_count(&self, slot: Slot, pubkey: &PublicKeyBytes) -> u64;

    /// A retained complete proposer view is not revoked by a failed or malformed refresh,
    /// local registry changes, `execution_optimistic`, or a reorg. Only a complete replacement
    /// changes its assignments.
    fn proposer_assignment_at_slot(
        &self,
        slot: Slot,
        validator_pubkey: &PublicKeyBytes,
    ) -> DutyAssignment;

    /// Unknown unless a completed PTC fetch covered this validator in the slot's epoch.
    fn ptc_assignment_at_slot(&self, slot: Slot, validator_index: ValidatorIndex)
    -> DutyAssignment;
}
