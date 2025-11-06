mod consensus_message;
mod duty_state;
mod message_counts;
mod partial_signature;

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use dashmap::{DashMap, mapref::one::RefMut};
use database::NetworkState;
pub use duties_tracker::DutiesProvider;
pub use gossipsub::MessageAcceptance;
use openssl::{
    hash::MessageDigest,
    pkey::{PKey, Public},
    rsa::Rsa,
    sign::Verifier,
};
use safe_arith::SafeArith;
use sha2::{Digest, Sha256};
use slot_clock::SlotClock;
use ssv_types::{
    CommitteeInfo, IndexSet, OperatorId, ValidatorIndex,
    consensus::QbftMessage,
    message::{MsgType, SignedSSVMessage},
    msgid::{DutyExecutor, MessageId, Role},
    partial_sig::PartialSignatureMessages,
};
use ssz::{Decode, DecodeError, Encode};
use task_executor::TaskExecutor;
use tokio::{sync::watch::Receiver, time::sleep};
use tracing::trace;
use types::{Epoch, Slot};

use crate::{
    consensus_message::validate_consensus_message,
    duty_state::{DutyState, OperatorState},
    partial_signature::validate_partial_signature_message,
};

const VALIDATOR_CLEANER_NAME: &str = "validator_cleaner";

pub(crate) const FIRST_ROUND: u64 = 1;

#[derive(Debug)]
pub enum ValidationResult {
    Success(ValidatedMessage),
    PreDecodeFailure(ValidationFailure),
    PostDecodeFailure(ValidationFailure, SignedSSVMessage),
}

impl ValidationResult {
    pub fn as_result(&self) -> Result<&ValidatedMessage, &ValidationFailure> {
        match self {
            ValidationResult::Success(message) => Ok(message),
            ValidationResult::PreDecodeFailure(failure) => Err(failure),
            ValidationResult::PostDecodeFailure(failure, _) => Err(failure),
        }
    }

    pub fn signed_ssv_message(&self) -> Option<&SignedSSVMessage> {
        match self {
            ValidationResult::Success(message) => Some(&message.signed_ssv_message),
            ValidationResult::PreDecodeFailure(_) => None,
            ValidationResult::PostDecodeFailure(_, message) => Some(message),
        }
    }
}

impl From<&ValidationResult> for MessageAcceptance {
    fn from(value: &ValidationResult) -> Self {
        match value.as_result() {
            Ok(_) => MessageAcceptance::Accept,
            Err(failure) => failure.into(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum ValidationFailure {
    WrongDomain,
    NoShareMetadata,
    UnknownValidator,
    ValidatorLiquidated,
    ValidatorNotAttesting,
    EarlySlotMessage {
        got: String,
    },
    LateSlotMessage {
        got: String,
    },
    SlotAlreadyAdvanced {
        got: u64,
        want: u64,
    },
    RoundAlreadyAdvanced {
        got: u64,
        want: u64,
    },
    DecidedWithSameSigners,
    PubSubDataTooBig(usize),
    IncorrectTopic,
    NonExistentCommitteeID,
    RoundTooHigh,
    ValidatorIndexMismatch,
    TooManyDutiesPerEpoch,
    NoDuty,
    EstimatedRoundNotInAllowedSpread {
        got: String,
        want: String,
    },
    EmptyData,
    MismatchedIdentifier {
        got: String,
        want: String,
    },
    SignatureVerification,
    PubSubMessageHasNoData,
    MalformedPubSubMessage,
    NilSignedSSVMessage,
    NilSSVMessage,
    SSVDataTooBig,
    InvalidRole,
    UnexpectedConsensusMessage,
    NoSigners,
    WrongRSASignatureSize,
    ZeroSigner,
    SignerNotInCommittee,
    DuplicatedSigner,
    SignerNotLeader {
        signer: OperatorId,
        leader: OperatorId,
    },
    SignersNotSorted,
    InconsistentSigners,
    InvalidHash,
    FullDataHash,
    UndecodableMessageData(DecodeError),
    EventMessage,
    UnknownSSVMessageType,
    UnknownQBFTMessageType,
    InvalidPartialSignatureType,
    PartialSignatureTypeRoleMismatch,
    NonDecidedWithMultipleSigners {
        got: usize,
        want: usize,
    },
    DecidedNotEnoughSigners {
        got: usize,
        want: usize,
    },
    DifferentProposalData,
    MalformedJustifications,
    UnexpectedPrepareJustifications,
    UnexpectedRoundChangeJustifications,
    NoPartialSignatureMessages,
    NoValidators,
    NoSignatures,
    OperatorNotFound {
        operator_id: OperatorId,
    },
    SignersAndSignaturesWithDifferentLength,
    PartialSigOneSigner,
    PrepareOrCommitWithFullData,
    FullDataNotInConsensusMessage,
    TripleValidatorIndexInPartialSignatures,
    ZeroRound,
    RoundOverflow,
    DuplicatedMessage {
        got: String,
    }, // Updated to include context
    InvalidPartialSignatureTypeCount {
        got: String,
    },
    TooManyPartialSignatureMessages {
        got: usize,
        limit: usize,
    },
    EncodeOperators,
    FailedToGetMaxRound,
    SlotStartTimeNotFound {
        slot: Slot,
    },
    SignatureVerificationFailed {
        reason: String,
    },
    ExcessiveDutyCount {
        got: u64,
        limit: u64,
        role: Role,
    },
    SyncCommitteePeriodCalculationFailure,
    UnexpectedFailure {
        msg: String,
    },
}

impl From<&ValidationFailure> for MessageAcceptance {
    fn from(value: &ValidationFailure) -> Self {
        match value {
            ValidationFailure::WrongDomain
            | ValidationFailure::NoShareMetadata
            | ValidationFailure::UnknownValidator
            | ValidationFailure::ValidatorLiquidated
            | ValidationFailure::ValidatorNotAttesting
            | ValidationFailure::EarlySlotMessage { .. }
            | ValidationFailure::LateSlotMessage { .. }
            | ValidationFailure::SlotAlreadyAdvanced { .. }
            | ValidationFailure::RoundAlreadyAdvanced { .. }
            | ValidationFailure::DecidedWithSameSigners
            | ValidationFailure::PubSubDataTooBig(_)
            | ValidationFailure::IncorrectTopic
            | ValidationFailure::NonExistentCommitteeID
            | ValidationFailure::RoundTooHigh
            | ValidationFailure::RoundOverflow
            | ValidationFailure::ValidatorIndexMismatch
            | ValidationFailure::TooManyDutiesPerEpoch
            | ValidationFailure::NoDuty
            | ValidationFailure::EstimatedRoundNotInAllowedSpread { .. } => {
                MessageAcceptance::Ignore
            }
            _ => MessageAcceptance::Reject,
        }
    }
}

#[derive(Debug)]
pub enum ValidatedSSVMessage {
    QbftMessage(QbftMessage),
    PartialSignatureMessages(PartialSignatureMessages),
}

#[derive(Debug)]
pub struct ValidatedMessage {
    pub signed_ssv_message: SignedSSVMessage,
    pub ssv_message: ValidatedSSVMessage,
}

impl ValidatedMessage {
    pub fn new(signed_ssv_message: SignedSSVMessage, ssv_message: ValidatedSSVMessage) -> Self {
        Self {
            signed_ssv_message,
            ssv_message,
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Processor error: {0}")]
    Processor(#[from] ::processor::Error),
}

struct ValidationContext<'a, S> {
    pub signed_ssv_message: &'a SignedSSVMessage,
    pub role: Role, // Small value type can remain owned
    pub committee_info: &'a CommitteeInfo,
    pub received_at: SystemTime, // Small value type
    pub slots_per_epoch: u64,
    pub epochs_per_sync_committee_period: u64,
    pub sync_committee_size: usize,
    pub slot_clock: S,
    pub operator_pub_keys: &'a HashMap<OperatorId, Rsa<Public>>,
}

pub struct Validator<S: SlotClock, D: DutiesProvider> {
    network_state_rx: Receiver<NetworkState>,
    duty_state_map: DashMap<MessageId, DutyState>,
    slots_per_epoch: u64,
    epochs_per_sync_committee_period: u64,
    sync_committee_size: usize,
    duties_provider: Arc<D>,
    slot_clock: S,
}

impl<S: SlotClock + 'static, D: DutiesProvider> Validator<S, D> {
    pub fn new(
        network_state_rx: Receiver<NetworkState>,
        slots_per_epoch: u64,
        epochs_per_sync_committee_period: u64,
        sync_committee_size: usize,
        duties_provider: Arc<D>,
        slot_clock: S,
        task_executor: &TaskExecutor,
    ) -> Arc<Self> {
        let validator = Arc::new(Self {
            network_state_rx,
            duty_state_map: DashMap::new(),
            slots_per_epoch,
            epochs_per_sync_committee_period,
            sync_committee_size,
            duties_provider,
            slot_clock,
        });

        task_executor.spawn(Arc::clone(&validator).cleaner(), VALIDATOR_CLEANER_NAME);

        validator
    }

    pub fn validate(&self, message_data: &[u8]) -> ValidationResult {
        match SignedSSVMessage::from_ssz_bytes(message_data) {
            Ok(signed_ssv_message) => {
                trace!(msg = ?signed_ssv_message, "SignedSSVMessage deserialized");
                match self.validate_decoded_message(&signed_ssv_message) {
                    Ok(validated_message) => ValidationResult::Success(validated_message),
                    Err(failure) => {
                        ValidationResult::PostDecodeFailure(failure, signed_ssv_message)
                    }
                }
            }
            Err(error) => {
                ValidationResult::PreDecodeFailure(ValidationFailure::UndecodableMessageData(error))
            }
        }
    }

    fn validate_decoded_message(
        &self,
        signed_ssv_message: &SignedSSVMessage,
    ) -> Result<ValidatedMessage, ValidationFailure> {
        // Get the role from message ID
        let ssv_message = signed_ssv_message.ssv_message();
        let role = ssv_message
            .msg_id()
            .role()
            .ok_or(ValidationFailure::InvalidRole)?;

        // Get committee info based on role and duty executor
        let network_state = self.network_state_rx.borrow();
        let committee_info = match role {
            Role::Committee => {
                let committee_id = match ssv_message.msg_id().duty_executor() {
                    Some(DutyExecutor::Committee(id)) => id,
                    _ => return Err(ValidationFailure::NonExistentCommitteeID),
                };
                network_state
                    .get_committee_info_by_committee_id(&committee_id)
                    .ok_or(ValidationFailure::NonExistentCommitteeID)?
            }
            _ => {
                let validator_pk = match ssv_message.msg_id().duty_executor() {
                    Some(DutyExecutor::Validator(pk)) => pk,
                    _ => return Err(ValidationFailure::UnknownValidator),
                };

                network_state
                    .get_committee_info_by_validator_pk(&validator_pk)
                    .ok_or(ValidationFailure::UnknownValidator)?
            }
        };
        let operator_pub_keys =
            &get_operator_pub_keys(&network_state, &committee_info.committee_members);

        drop(network_state);

        let mut duty_state = self.get_duty_state(ssv_message.msg_id(), self.slots_per_epoch);

        let validation_context = ValidationContext {
            signed_ssv_message,
            role,
            committee_info: &committee_info,
            received_at: SystemTime::now(),
            slots_per_epoch: self.slots_per_epoch,
            epochs_per_sync_committee_period: self.epochs_per_sync_committee_period,
            sync_committee_size: self.sync_committee_size,
            slot_clock: self.slot_clock.clone(),
            operator_pub_keys,
        };

        validate_ssv_message(
            validation_context,
            duty_state.value_mut(),
            self.duties_provider.clone(),
        )
        .map(|validated| ValidatedMessage::new(signed_ssv_message.clone(), validated))
    }

    /// Gets the duty state for a message ID, creating a new one if it doesn't exist
    fn get_duty_state(
        &self,
        message_id: &MessageId,
        slots_per_epoch: u64,
    ) -> RefMut<'_, MessageId, DutyState> {
        self.duty_state_map
            .entry(message_id.clone())
            .or_insert_with(|| {
                let stored_slot_count = slots_per_epoch * 2; // Store last two epochs

                DutyState::new(stored_slot_count as usize)
            })
    }

    async fn cleaner(self: Arc<Self>) {
        let slot_clock = self.slot_clock.clone();
        let slots_per_epoch = self.slots_per_epoch;

        // Use a weak reference to exit when the other `Arc` are dropped.
        let weak_self = Arc::downgrade(&self);
        loop {
            // Try to get the time to the next slot.
            let Some(until_next_epoch) = slot_clock.duration_to_next_epoch(slots_per_epoch) else {
                sleep(slot_clock.slot_duration()).await;
                continue;
            };

            // Wait until 5/6ths into the slot. Then, all proposal and attestation duties should be
            // done, so we can lock the map without risking message delays for time-critical
            // messages.
            let sleep_for = until_next_epoch + slot_clock.slot_duration() * 5 / 6;
            sleep(sleep_for).await;

            let Some(validator) = weak_self.upgrade() else {
                // No validator to clean anymore, exit.
                break;
            };
            let Some(now) = slot_clock.now() else {
                // Very weird, let's try again later.
                continue;
            };

            validator
                .duty_state_map
                .retain(|_, duty_state| !duty_state.outdated(now));
        }
    }
}

fn validate_ssv_message(
    validation_context: ValidationContext<impl SlotClock>,
    duty_state: &mut DutyState,
    duty_provider: Arc<impl DutiesProvider>,
) -> Result<ValidatedSSVMessage, ValidationFailure> {
    let ssv_message = validation_context.signed_ssv_message.ssv_message();

    match ssv_message.msg_type() {
        MsgType::SSVConsensusMsgType => {
            validate_consensus_message(validation_context, duty_state, duty_provider)
        }
        MsgType::SSVPartialSignatureMsgType => {
            validate_partial_signature_message(validation_context, duty_state, duty_provider)
        }
    }
}

fn verify_message_signature(
    signed_message: &SignedSSVMessage,
    operator_pk: &Rsa<Public>,
    signature: &[u8],
) -> Result<(), ValidationFailure> {
    let p_key = PKey::from_rsa(operator_pk.clone()).map_err(|e| {
        ValidationFailure::SignatureVerificationFailed {
            reason: format!("Failed to create PKey: {e}"),
        }
    })?;

    let mut verifier = Verifier::new(MessageDigest::sha256(), &p_key).map_err(|e| {
        ValidationFailure::SignatureVerificationFailed {
            reason: format!("Failed to create verifier: {e}"),
        }
    })?;

    verifier
        .update(&signed_message.ssv_message().as_ssz_bytes())
        .map_err(|e| ValidationFailure::SignatureVerificationFailed {
            reason: format!("Failed to update verifier: {e}"),
        })?;

    match verifier.verify(signature) {
        Ok(true) => Ok(()),
        Ok(false) => Err(ValidationFailure::SignatureVerificationFailed {
            reason: "Signature verification failed".to_string(),
        }),
        Err(e) => Err(ValidationFailure::SignatureVerificationFailed {
            reason: format!("Signature verification error: {e}"),
        }),
    }
}

/// Verifies all signatures in a signed SSV message
fn verify_message_signatures(
    signed_message: &SignedSSVMessage,
    operator_pub_keys: &HashMap<OperatorId, Rsa<Public>>,
) -> Result<(), ValidationFailure> {
    let signatures = signed_message.signatures();

    let operators_pks = signed_message
        .operator_ids()
        .iter()
        .map(|operator_id| {
            operator_pub_keys
                .get(operator_id)
                .ok_or(ValidationFailure::OperatorNotFound {
                    operator_id: *operator_id,
                })
        })
        .collect::<Result<Vec<&Rsa<Public>>, ValidationFailure>>()?;

    // Basic validation for signature/operator count matching
    if signatures.len() != operators_pks.len() {
        return Err(ValidationFailure::SignatureVerificationFailed {
            reason: "Signature count doesn't match operator count".to_string(),
        });
    }

    for (signature, operator_pk) in signatures.iter().zip(operators_pks.iter()) {
        verify_message_signature(signed_message, operator_pk, signature)?
    }

    Ok(())
}

/// Validates if a validator is assigned to a specific duty
pub(crate) fn validate_beacon_duty(
    validation_context: &ValidationContext<impl SlotClock>,
    slot: Slot,
    randao_msg: bool,
    duty_provider: Arc<impl DutiesProvider>,
) -> Result<(), ValidationFailure> {
    let role = validation_context.role;
    let epoch = slot.epoch(validation_context.slots_per_epoch);
    // Rule: For a proposal duty message, check if the validator is assigned to it
    if role == Role::Proposer {
        // Tolerate missing duties for RANDAO signatures during the first slot of an epoch,
        // while duties are still being fetched from the Beacon node.

        let is_first_slot_of_epoch = epoch.start_slot(validation_context.slots_per_epoch) == slot;

        if randao_msg
            && is_first_slot_of_epoch
            && validation_context
                .slot_clock
                .now()
                .ok_or(ValidationFailure::UnexpectedFailure {
                    msg: "Failed to get current time".to_string(),
                })?
                <= slot
            && !duty_provider.is_epoch_known_for_proposers(epoch)
        {
            return Ok(());
        }

        // Non-committee roles always have one validator index
        let validator_index = validation_context
            .committee_info
            .validator_indices
            .first()
            .copied()
            .ok_or(ValidationFailure::UnexpectedFailure {
                msg: "Unexpected error when getting first validator index".to_string(),
            })?;

        if !duty_provider.is_validator_proposer_at_slot(slot, validator_index) {
            return Err(ValidationFailure::NoDuty);
        }
    }

    // Rule: For a sync committee duty message, check if the validator is assigned
    if role == Role::SyncCommittee {
        let period =
            sync_committee_period(epoch, validation_context.epochs_per_sync_committee_period)?;
        let validator_index = validation_context
            .committee_info
            .validator_indices
            .first()
            .copied()
            .ok_or(ValidationFailure::UnexpectedFailure {
                msg: "Unexpected error when getting first validator index".to_string(),
            })?;

        if !duty_provider.is_validator_in_sync_committee(period, validator_index) {
            return Err(ValidationFailure::NoDuty);
        }
    }

    Ok(())
}

/// clockErrorTolerance is the maximum amount of clock error we expect to see between nodes.
const CLOCK_ERROR_TOLERANCE: Duration = Duration::from_millis(50);
/// lateMessageMargin is the duration past a message's TTL in which it is still considered valid.
///
/// This margin is added to the deadline calculation after converting slot-based TTL to time.
/// The full message acceptance window is: (ttl_slots × slot_duration) + LATE_MESSAGE_MARGIN
pub const LATE_MESSAGE_MARGIN: Duration = Duration::from_secs(3);
/// Number of slots added to TTL windows for late message acceptance
///
/// Used in calculating message acceptance deadlines for Committee and Aggregator roles.
/// The actual TTL is: slots_per_epoch + LATE_SLOT_ALLOWANCE
pub const LATE_SLOT_ALLOWANCE: u64 = 2;

/// Validates that the message's slot timing is correct
pub(crate) fn validate_slot_time(
    msg_slot: Slot,
    validation_context: &ValidationContext<impl SlotClock>,
) -> Result<(), ValidationFailure> {
    // Check if the message is too early
    let earliness = message_earliness(msg_slot, validation_context)?;
    if earliness > CLOCK_ERROR_TOLERANCE {
        return Err(ValidationFailure::EarlySlotMessage {
            got: format!("early by {earliness:?}"),
        });
    }

    // Check if the message is too late
    let lateness = message_lateness(msg_slot, validation_context)?;
    if lateness > CLOCK_ERROR_TOLERANCE {
        return Err(ValidationFailure::LateSlotMessage {
            got: format!("late by {lateness:?}"),
        });
    }

    Ok(())
}

/// Returns how early a message is compared to its slot start time.
/// Returns a zero duration if the message is on time or late.
fn message_earliness(
    slot: Slot,
    validation_context: &ValidationContext<impl SlotClock>,
) -> Result<Duration, ValidationFailure> {
    let slot_start = slot_start_time(slot, validation_context.slot_clock.clone())
        .map_err(|_| ValidationFailure::SlotStartTimeNotFound { slot })?;
    Ok(slot_start
        .duration_since(validation_context.received_at)
        .unwrap_or_default())
}

/// Returns how late a message is compared to its deadline based on role.
/// If the message was received before the deadline, it returns 0.
/// If the message was received after the deadline, it returns the duration by which it was late.
fn message_lateness(
    slot: Slot,
    validation_context: &ValidationContext<impl SlotClock>,
) -> Result<Duration, ValidationFailure> {
    let ttl = match validation_context.role {
        Role::Proposer | Role::SyncCommittee => 1 + LATE_SLOT_ALLOWANCE,
        Role::Committee | Role::Aggregator | Role::ValidatorRegistration | Role::VoluntaryExit => {
            validation_context.slots_per_epoch + LATE_SLOT_ALLOWANCE
        }
    };

    let deadline = slot_start_time(slot + ttl, validation_context.slot_clock.clone())
        .map_err(|_| ValidationFailure::SlotStartTimeNotFound { slot })?
        .checked_add(LATE_MESSAGE_MARGIN)
        .ok_or(ValidationFailure::UnexpectedFailure {
            msg: "Unexpected overflow calculating message deadline".to_string(),
        })?;

    Ok(validation_context
        .received_at
        .duration_since(deadline)
        .unwrap_or_default())
}

/// Validates the duty count for a specific message and operator
pub(crate) fn validate_duty_count(
    validation_context: &ValidationContext<impl SlotClock>,
    slot: Slot,
    signer_state: &mut OperatorState,
    duty_provider: Arc<impl DutiesProvider>,
) -> Result<(), ValidationFailure> {
    if let Some(limit) = duty_limit(
        validation_context,
        slot,
        &validation_context.committee_info.validator_indices,
        duty_provider,
    )? {
        // Get current duty count for this signer
        let epoch = slot.epoch(validation_context.slots_per_epoch);
        let duty_count = signer_state.get_duty_count(epoch);

        // Error if this validator has already been assigned at least as many duties
        // as allowed for the target epoch. We perform this check *before* incrementing
        // the in-memory count (so the very first duty will see count==0), hence the
        // inclusive “>=” comparison.
        // We only want to check the limit if this is the first message of that duty, as otherwise
        // the check will fail for non-first messages of the last allowed duty. We do this by
        // checking if there is a signer state already set for that slot. If so, we have already
        // processed a message for this duty and the counter will not be increased further in
        // `OperatorState::update`, so we skip the limit check here also.
        if signer_state.is_first_message_for_duty(slot) && duty_count >= limit {
            return Err(ValidationFailure::ExcessiveDutyCount {
                got: duty_count,
                limit,
                role: validation_context.role,
            });
        }
    }

    Ok(())
}

/// Determines duty limit based on role and validator indices
fn duty_limit(
    validation_context: &ValidationContext<impl SlotClock>,
    slot: Slot,
    validator_indices: &[ValidatorIndex],
    duty_provider: Arc<impl DutiesProvider>,
) -> Result<Option<u64>, ValidationFailure> {
    match validation_context.role {
        Role::VoluntaryExit => {
            // Extract the validator public key from the message ID
            let pubkey = match validation_context
                .signed_ssv_message
                .ssv_message()
                .msg_id()
                .duty_executor()
            {
                Some(DutyExecutor::Validator(pubkey)) => pubkey,
                _ => return Err(ValidationFailure::UnknownValidator),
            };
            // Get the current voluntary exit duty count for this validator
            Ok(Some(
                duty_provider.get_voluntary_exit_duty_count(slot, &pubkey),
            ))
        }
        Role::Aggregator | Role::ValidatorRegistration => Ok(Some(2)),
        Role::Committee => {
            let validator_index_count = validator_indices.len() as u64;
            let slots_per_epoch_val = validation_context.slots_per_epoch;

            // Skip duty search if validators * 2 exceeds slots per epoch
            if validator_index_count < slots_per_epoch_val / 2 {
                let epoch = slot.epoch(validation_context.slots_per_epoch);
                let period = sync_committee_period(
                    epoch,
                    validation_context.epochs_per_sync_committee_period,
                )?;

                // Check if at least one validator is in the sync committee
                for &index in validator_indices {
                    if duty_provider.is_validator_in_sync_committee(period, index) {
                        return Ok(Some(slots_per_epoch_val));
                    }
                }
            }
            Ok(Some(std::cmp::min(
                slots_per_epoch_val,
                2 * validator_index_count,
            )))
        }
        _ => Ok(None),
    }
}

#[derive(thiserror::Error, Debug)]
pub enum TimeError {
    #[error("clock start-of-slot overflow for slot {0}")]
    Overflow(Slot),
}

pub fn slot_start_time(slot: Slot, slot_clock: impl SlotClock) -> Result<SystemTime, TimeError> {
    let dur = slot_clock.start_of(slot).ok_or(TimeError::Overflow(slot))?;
    Ok(UNIX_EPOCH + dur)
}

/// Compute the sync committee period for an epoch.
pub fn sync_committee_period(
    epoch: Epoch,
    epochs_per_sync_committee_period: u64,
) -> Result<u64, ValidationFailure> {
    Ok(epoch
        .safe_div(epochs_per_sync_committee_period)
        .map_err(|_| ValidationFailure::SyncCommitteePeriodCalculationFailure)?
        .as_u64())
}

pub(crate) fn compute_quorum_size(committee_size: usize) -> usize {
    let f = get_f(committee_size);
    f * 2 + 1
}

fn get_operator_pub_keys(
    network_state: &NetworkState,
    operator_ids: &IndexSet<OperatorId>,
) -> HashMap<OperatorId, Rsa<Public>> {
    operator_ids
        .iter()
        .flat_map(|id| {
            network_state
                .get_operator(id)
                .map(|operator| (*id, operator.rsa_pubkey))
        })
        .collect()
}

// # TODO centralize this and the one in the qbft crate
pub(crate) fn get_f(committee_size: usize) -> usize {
    (committee_size - 1) / 3
}

pub(crate) fn hash_data(full_data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(full_data);
    let hash: [u8; 32] = hasher.finalize().into();
    hash
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use bls::{Hash256, PublicKeyBytes};
    use duties_tracker::DutiesProvider;
    use openssl::{
        hash::MessageDigest,
        pkey::{PKey, Private, Public},
        rsa::Rsa,
        sign::Signer,
    };
    use ssv_types::{
        CommitteeId, CommitteeInfo, IndexSet, OperatorId, RSA_SIGNATURE_SIZE, ValidatorIndex,
        VariableList,
        consensus::{QbftMessage, QbftMessageType},
        domain_type::DomainType,
        message::{MsgType, SSVMessage, SignedSSVMessage},
        msgid::{DutyExecutor, MessageId, Role},
    };
    use ssz::Encode;
    use types::{Epoch, Slot};

    use crate::{ValidationFailure, compute_quorum_size, hash_data};

    // Constants for committee sizes in tests to improve readability
    pub(crate) const SINGLE_NODE_COMMITTEE: usize = 1;
    pub(crate) const FOUR_NODE_COMMITTEE: usize = 4;
    pub(crate) const SEVEN_NODE_COMMITTEE: usize = 7;

    // Helper struct for directly creating consensus messages for tests
    pub(crate) struct QbftMessageBuilder {
        msg_type: QbftMessageType,
        round: u64,
        identifier: MessageId,
        prepare_justification: Vec<SignedSSVMessage>,
        round_change_justification: Vec<SignedSSVMessage>,
    }

    impl QbftMessageBuilder {
        pub(crate) fn new(role: Role, msg_type: QbftMessageType) -> Self {
            Self {
                msg_type,
                round: 1,
                identifier: create_message_id_for_test(role),
                prepare_justification: vec![],
                round_change_justification: vec![],
            }
        }

        pub(crate) fn with_round(mut self, round: u64) -> Self {
            self.round = round;
            self
        }

        pub(crate) fn with_identifier(mut self, identifier: MessageId) -> Self {
            self.identifier = identifier;
            self
        }

        pub(crate) fn with_prepare_justification(
            mut self,
            justifications: Vec<SignedSSVMessage>,
        ) -> Self {
            self.prepare_justification = justifications;
            self
        }

        pub(crate) fn with_round_change_justification(
            mut self,
            justifications: Vec<SignedSSVMessage>,
        ) -> Self {
            self.round_change_justification = justifications;
            self
        }

        pub(crate) fn build(self) -> QbftMessage {
            // This is a test builder, so using expect() is acceptable here
            // Convert Vec<SignedSSVMessage> to VariableList<VariableList<u8, _>, U13>
            let round_change_justification_vec: Vec<_> = self
                .round_change_justification
                .into_iter()
                .map(|msg| msg.without_full_data())
                .map(|msg| {
                    let bytes = msg.as_ssz_bytes();
                    VariableList::new(bytes).unwrap() // Test data should fit
                })
                .collect();
            let round_change_justification =
                VariableList::new(round_change_justification_vec).unwrap(); // Test data should fit

            let prepare_justification_vec: Vec<_> = self
                .prepare_justification
                .into_iter()
                .map(|msg| msg.without_full_data())
                .map(|msg| {
                    let bytes = msg.as_ssz_bytes();
                    VariableList::new(bytes).unwrap() // Test data should fit
                })
                .collect();
            let prepare_justification = VariableList::new(prepare_justification_vec).unwrap(); // Test data should fit

            QbftMessage {
                qbft_message_type: self.msg_type,
                height: 1,
                round: self.round,
                identifier: (&self.identifier).into(),
                root: Hash256::from([0u8; 32]),
                data_round: 1,
                round_change_justification,
                prepare_justification,
            }
        }
    }

    // Helper for creating SignedSSVMessage with a QbftMessage
    pub(crate) fn create_signed_consensus_message(
        qbft_message: QbftMessage,
        signers: Vec<OperatorId>,
        full_data: Vec<u8>,
        pks: Vec<Rsa<Private>>,
    ) -> SignedSSVMessage {
        // Validate that we don't have any zero signers
        assert!(!signers.is_empty(), "Must provide at least one signer");
        assert!(
            signers.iter().all(|s| s.0 > 0),
            "OperatorId(0) is not allowed as it causes ZeroSigner error"
        );

        let qbft_bytes = qbft_message.as_ssz_bytes();
        let slice: &[u8] = qbft_message.identifier.as_ref();
        let msg_id: [u8; 56] = slice
            .try_into()
            .expect("VariableList does not contain exactly 56 bytes");
        let ssv_msg = SSVMessage::new(
            MsgType::SSVConsensusMsgType,
            msg_id.into(),
            qbft_bytes.clone(),
        )
        .expect("SSVMessage should be created");

        let signatures = if pks.is_empty() {
            signers
                .iter()
                .enumerate()
                .map(|(i, _)| [0xAA + i as u8; RSA_SIGNATURE_SIZE])
                .collect::<Vec<_>>()
        } else {
            pks.iter()
                .map(|pk| {
                    let p_key = PKey::from_rsa(pk.clone()).unwrap();
                    let mut signer = Signer::new(MessageDigest::sha256(), &p_key).unwrap();
                    signer.update(&ssv_msg.as_ssz_bytes()).unwrap();
                    signer
                        .sign_to_vec()
                        .expect("Failed to sign message")
                        .try_into()
                        .expect("Signature should be 256 bytes")
                })
                .collect::<Vec<_>>()
        };

        SignedSSVMessage::new(signatures, signers, ssv_msg, full_data)
            .expect("SignedSSVMessage should be created")
    }

    pub(crate) fn generate_random_rsa_public_keys(count: usize) -> Vec<Rsa<Public>> {
        (0..count)
            .map(|_| {
                // 1) Generate a full private key
                let private_key = Rsa::generate(2048).expect("Failed to generate RSA private key");

                // 2) Extract the public part
                Rsa::from_public_components(
                    private_key.n().to_owned().expect("Failed to get modulus"),
                    private_key.e().to_owned().expect("Failed to get exponent"),
                )
                .expect("Failed to create Rsa<Public> from components")
            })
            .collect()
    }

    // Create a committee info object for tests
    pub(crate) fn create_committee_info(committee_size: usize) -> CommitteeInfo {
        let mut members = IndexSet::new();
        for i in 0..committee_size {
            // Start from 1 to avoid zero values
            members.insert(OperatorId(i as u64 + 1));
        }

        CommitteeInfo {
            committee_members: members,
            validator_indices: vec![ValidatorIndex(0), ValidatorIndex(123)],
        }
    }

    // Helper to create a message ID for tests
    pub(crate) fn create_message_id_for_test(role: Role) -> MessageId {
        let domain = DomainType([0, 0, 0, 1]);
        let duty_executor = match role {
            Role::Committee => DutyExecutor::Committee(CommitteeId([0u8; 32])),
            _ => DutyExecutor::Validator(PublicKeyBytes::empty()),
        };
        MessageId::new(&domain, role, &duty_executor)
    }

    // Helper to create a HashMap of CommitteeId -> PublicKey for tests
    pub(crate) fn create_operator_pub_keys(
        committee_members: IndexSet<OperatorId>,
        public_keys: Vec<Rsa<Public>>,
    ) -> HashMap<OperatorId, Rsa<Public>> {
        committee_members.into_iter().zip(public_keys).collect()
    }

    // Assert helpers for common validation patterns
    pub fn assert_validation_error<T, F>(
        result: Result<T, ValidationFailure>,
        expected_error: F,
        error_name: &str,
    ) where
        F: Fn(&ValidationFailure) -> bool,
    {
        match result {
            Ok(_) => panic!("Expected validation to fail with {error_name}"),
            Err(failure) => {
                assert!(
                    expected_error(&failure),
                    "Expected {error_name} error, got: {failure:?}"
                );
            }
        }
    }

    #[derive(Default)]
    pub struct MockDutiesProvider {
        pub(crate) voluntary_exit_duty_count: u64,
    }
    impl DutiesProvider for MockDutiesProvider {
        fn is_validator_in_sync_committee(
            &self,
            _committee_period: u64,
            _validator_index: ValidatorIndex,
        ) -> bool {
            true
        }

        fn is_epoch_known_for_proposers(&self, _epoch: Epoch) -> bool {
            true
        }

        fn is_validator_proposer_at_slot(
            &self,
            _slot: Slot,
            _validator_index: ValidatorIndex,
        ) -> bool {
            true
        }

        fn get_voluntary_exit_duty_count(&self, _slot: Slot, _pubkey: &PublicKeyBytes) -> u64 {
            self.voluntary_exit_duty_count
        }
    }

    // ---------------------------------------------------------------------
    // Utility function tests
    // ---------------------------------------------------------------------

    #[test]
    fn test_compute_quorum_size() {
        // For committee_size=4 -> f=1 -> quorum=3.
        assert_eq!(
            compute_quorum_size(FOUR_NODE_COMMITTEE),
            3,
            "Expected quorum=3 for committee of 4"
        );
        // For committee_size=7 -> f=2 -> quorum=5.
        assert_eq!(
            compute_quorum_size(SEVEN_NODE_COMMITTEE),
            5,
            "Expected quorum=5 for committee of 7"
        );
        // For committee_size=1 -> f=0 -> quorum=1.
        assert_eq!(
            compute_quorum_size(SINGLE_NODE_COMMITTEE),
            1,
            "Expected quorum=1 for committee of 1"
        );
    }

    #[test]
    fn test_hash_data_root() {
        let data1 = vec![1, 2, 3, 4];
        let data2 = vec![1, 2, 3, 5]; // One byte different

        let hash1 = hash_data(&data1);
        let hash2 = hash_data(&data2);

        assert_ne!(
            hash1, hash2,
            "Different data should produce different hashes"
        );
        assert_eq!(
            hash1,
            hash_data(&data1),
            "Same data should produce the same hash"
        );
    }
}
