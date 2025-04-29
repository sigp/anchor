use std::{convert::Into, sync::Arc, time::Duration};

use slot_clock::SlotClock;
use ssv_types::{
    consensus::{QbftMessage, QbftMessageType},
    message::SignedSSVMessage,
    msgid::Role,
    CommitteeInfo, IndexSet, OperatorId, Round, Slot, ValidatorIndex, VariableList,
};
use ssz::Decode;
use ValidationFailure::EarlySlotMessage;

use crate::{
    compute_quorum_size,
    consensus_state::{ConsensusState, OperatorState},
    duties::DutiesProvider,
    hash_data, slot_start_time, sync_committee_period, verify_message_signatures,
    ValidatedSSVMessage, ValidationContext, ValidationFailure,
};

pub(crate) fn validate_consensus_message(
    validation_context: ValidationContext<impl SlotClock>,
    consensus_state: &mut ConsensusState,
    duty_provider: Arc<impl DutiesProvider>,
) -> Result<ValidatedSSVMessage, ValidationFailure> {
    // Decode message to QbftMessage
    let consensus_message = match QbftMessage::from_ssz_bytes(
        validation_context.signed_ssv_message.ssv_message().data(),
    ) {
        Ok(msg) => msg,
        Err(_) => return Err(ValidationFailure::UndecodableMessageData),
    };

    // Call the existing semantic validation
    validate_consensus_message_semantics(
        validation_context.signed_ssv_message,
        &consensus_message,
        validation_context.committee_info,
    )?;

    validate_qbft_logic(&validation_context, &consensus_message, consensus_state)?;

    validate_qbft_message_by_duty_logic(
        &validation_context,
        &consensus_message,
        consensus_state,
        duty_provider,
    )?;

    verify_message_signatures(
        validation_context.signed_ssv_message,
        validation_context.operators_pk,
    )?;

    consensus_state.update(
        validation_context.signed_ssv_message,
        &consensus_message,
        validation_context.slots_per_epoch,
    );

    // Return the validated message
    Ok(ValidatedSSVMessage::QbftMessage(consensus_message))
}

pub(crate) fn validate_consensus_message_semantics(
    signed_ssv_message: &SignedSSVMessage,
    consensus_message: &QbftMessage,
    committee_info: &CommitteeInfo,
) -> Result<(), ValidationFailure> {
    let signers = signed_ssv_message.operator_ids().len();

    let quorum_size = compute_quorum_size(committee_info.committee_members.len());
    let msg_type = consensus_message.qbft_message_type;

    if signers > 1 {
        // Rule: Decided msg with different type than Commit
        if msg_type != QbftMessageType::Commit {
            return Err(ValidationFailure::NonDecidedWithMultipleSigners {
                got: signers,
                want: 1,
            });
        }

        // Rule: Number of signers must be >= quorum size
        if signers < quorum_size {
            return Err(ValidationFailure::DecidedNotEnoughSigners {
                got: signers,
                want: quorum_size,
            });
        }
    }

    if !signed_ssv_message.full_data().is_empty() {
        // Rule: Prepare or commit messages must not have full data
        if msg_type == QbftMessageType::Prepare
            || (msg_type == QbftMessageType::Commit && signers == 1)
        {
            return Err(ValidationFailure::PrepareOrCommitWithFullData);
        }

        let hashed_full_data = hash_data(signed_ssv_message.full_data());
        // Rule: Full data hash must match root
        if hashed_full_data != consensus_message.root {
            return Err(ValidationFailure::InvalidHash);
        }
    }

    if consensus_message.round == 0 {
        return Err(ValidationFailure::ZeroRound);
    }

    // Rule: Duty role has consensus (true except for ValidatorRegistration and VoluntaryExit)
    if matches!(
        signed_ssv_message.ssv_message().msg_id().role(),
        Some(Role::ValidatorRegistration) | Some(Role::VoluntaryExit)
    ) {
        return Err(ValidationFailure::UnexpectedConsensusMessage);
    }

    let max_round = match signed_ssv_message
        .ssv_message()
        .msg_id()
        .role()
        .unwrap()
        .max_round()
    {
        Some(max_round) => max_round,
        None => return Err(ValidationFailure::FailedToGetMaxRound),
    };

    if consensus_message.round > max_round {
        return Err(ValidationFailure::RoundTooHigh);
    }

    // Rule: consensus message must have the same identifier as the ssv message's identifier
    if consensus_message.identifier != VariableList::from(signed_ssv_message.ssv_message().msg_id())
    {
        return Err(ValidationFailure::MismatchedIdentifier {
            got: hex::encode(&*consensus_message.identifier),
            want: hex::encode(signed_ssv_message.ssv_message().msg_id()),
        });
    }

    validate_justifications(consensus_message)?;

    Ok(())
}

pub(crate) fn validate_justifications(
    consensus_message: &QbftMessage,
) -> Result<(), ValidationFailure> {
    // Rule: Can only exist for Proposal messages
    let prepare_justifications = &consensus_message.prepare_justification;
    if !prepare_justifications.is_empty()
        && consensus_message.qbft_message_type != QbftMessageType::Proposal
    {
        return Err(ValidationFailure::UnexpectedPrepareJustifications);
    }

    // Rule: Can only exist for Proposal or Round-Change messages
    let round_change_justifications = &consensus_message.round_change_justification;
    if !round_change_justifications.is_empty()
        && consensus_message.qbft_message_type != QbftMessageType::Proposal
        && consensus_message.qbft_message_type != QbftMessageType::RoundChange
    {
        return Err(ValidationFailure::UnexpectedRoundChangeJustifications);
    }

    Ok(())
}

#[allow(clippy::comparison_chain)]
pub(crate) fn validate_qbft_logic(
    validation_context: &ValidationContext<impl SlotClock>,
    consensus_message: &QbftMessage,
    consensus_state: &mut ConsensusState,
) -> Result<(), ValidationFailure> {
    let signed_ssv_message = validation_context.signed_ssv_message;

    // Rule: For proposals, signer must be the leader
    let signers = signed_ssv_message.operator_ids();
    if consensus_message.qbft_message_type == QbftMessageType::Proposal {
        let Some(&signer) = signers.first() else {
            return Err(ValidationFailure::NoSigners);
        };

        let leader = round_robin_proposer(
            consensus_message.height,
            consensus_message.round.into(),
            &validation_context.committee_info.committee_members,
        )?;

        if signer != leader {
            return Err(ValidationFailure::SignerNotLeader { signer, leader });
        }
    }

    // Create slot from height
    let msg_slot = Slot::new(consensus_message.height);

    // Check validation rules for each signer
    for signer in signers {
        // Get or create the operator state first, then check if there's a signer state
        let Some(signer_state) = consensus_state
            .get_or_create_operator(signer)
            .get_signer_state(&msg_slot)
        else {
            continue;
        };

        if signers.len() == 1 {
            // Single-signer validation rules (non-decided messages)

            // Rule: Ignore if peer already advanced to a later round
            if consensus_message.round < signer_state.round {
                // Signers aren't allowed to decrease their round.
                // If they've sent a future message due to clock error,
                // they'd have to wait for the next slot/round to be accepted.
                return Err(ValidationFailure::RoundAlreadyAdvanced {
                    got: consensus_message.round,
                    want: signer_state.round,
                });
            }

            if consensus_message.round == signer_state.round {
                // Rule: Peer must not send two proposals with different data
                if !signed_ssv_message.full_data().is_empty()
                    && signer_state
                        .proposal_data
                        .as_ref()
                        .is_some_and(|data| data != signed_ssv_message.full_data())
                {
                    return Err(ValidationFailure::DifferentProposalData);
                }

                signer_state
                    .message_counts
                    .validate_limits(signed_ssv_message, consensus_message.qbft_message_type)?;
            }
        } else if signers.len() > 1 {
            // Rule: Decided msg can't have the same signers as previously sent before for the same
            // duty
            if signer_state.has_seen_signers(signers) {
                return Err(ValidationFailure::DecidedWithSameSigners);
            }
        }
    }

    // Rule: Round must be within allowed spread from current time
    if signers.len() == 1 {
        validate_round_in_allowed_spread(consensus_message, validation_context)?;
    }

    Ok(())
}

// Define constants to match the Go implementation
const FIRST_ROUND: u64 = 1;
const MAX_ALLOWED_ROUNDS_FUTURE: u64 = 3;

/// Determines the leader for a given height and round using round robin
fn round_robin_proposer(
    height: u64,
    round: Round,
    committee: &IndexSet<OperatorId>,
) -> Result<OperatorId, ValidationFailure> {
    if committee.is_empty() {
        return Err(ValidationFailure::NonExistentCommitteeID);
    }

    let first_round_index = height % committee.len() as u64;

    let round: u64 = round.into();
    let index = (first_round_index + round - FIRST_ROUND) % committee.len() as u64;

    // Get the operator at the calculated index
    Ok(committee[index as usize])
}

/// Validate that the message round is within the allowed spread
fn validate_round_in_allowed_spread(
    consensus_message: &QbftMessage,
    validation_context: &ValidationContext<impl SlotClock>,
) -> Result<(), ValidationFailure> {
    // Get the slot
    let slot = Slot::new(consensus_message.height);
    let slot_start_time = slot_start_time(slot, validation_context.slot_clock.clone())
        .map_err(|_| ValidationFailure::SlotStartTimeNotFound { slot })?;

    let (since_slot_start, estimated_round) = if validation_context.received_at > slot_start_time {
        let duration = validation_context
            .received_at
            .duration_since(slot_start_time)
            .unwrap_or_default();
        (duration, current_estimated_round(duration))
    } else {
        (Duration::from_secs(0), FIRST_ROUND.into())
    };

    let lowest_allowed = FIRST_ROUND;
    let highest_allowed = estimated_round + MAX_ALLOWED_ROUNDS_FUTURE;

    // Check if the round is within allowed spread
    if consensus_message.round < lowest_allowed || consensus_message.round > highest_allowed.into()
    {
        return Err(ValidationFailure::EstimatedRoundNotInAllowedSpread {
            got: format!(
                "{} ({} role)",
                consensus_message.round, validation_context.role
            ),
            want: format!(
                "between {} and {} ({} role) / {:?} passed",
                lowest_allowed, highest_allowed, validation_context.role, since_slot_start
            ),
        });
    }

    Ok(())
}

/// Constants for round timeouts
const QUICK_TIMEOUT_THRESHOLD: u64 = 8;
const QUICK_TIMEOUT: Duration = Duration::from_secs(2);
const SLOW_TIMEOUT: Duration = Duration::from_secs(120);

/// Calculates the current estimated round based on time since slot start,
/// using quick timeouts for early rounds and slow timeouts for later rounds
fn current_estimated_round(since_slot_start: Duration) -> Round {
    // Calculate quick round delta
    let delta_quick = since_slot_start.as_secs() / QUICK_TIMEOUT.as_secs();

    // Calculate the current round assuming quick timeouts
    let current_quick_round = FIRST_ROUND + delta_quick;

    // If we're in the quick timeout phase, return the quick round
    if current_quick_round <= QUICK_TIMEOUT_THRESHOLD {
        return current_quick_round.into();
    }

    // Otherwise we're in the slow phase
    // Calculate how much time has passed since we entered the slow phase
    let time_in_quick_phase = QUICK_TIMEOUT * QUICK_TIMEOUT_THRESHOLD as u32;
    let since_first_slow_round = since_slot_start.saturating_sub(time_in_quick_phase);

    // Calculate how many slow rounds have passed
    let delta_slow = since_first_slow_round.as_secs() / SLOW_TIMEOUT.as_secs();

    // In the Go code:
    // estimatedRound := roundtimer.QuickTimeoutThreshold + specqbft.FirstRound +
    // specqbft.Round(delta)
    (QUICK_TIMEOUT_THRESHOLD + FIRST_ROUND + delta_slow).into()
}

/// clockErrorTolerance is the maximum amount of clock error we expect to see between nodes.
const CLOCK_ERROR_TOLERANCE: Duration = Duration::from_millis(50);
/// lateMessageMargin is the duration past a message's TTL in which it is still considered valid.
const LATE_MESSAGE_MARGIN: Duration = Duration::from_secs(3);
const LATE_SLOT_ALLOWANCE: u64 = 2;

/// Validates QBFT messages based on beacon chain duties
pub(crate) fn validate_qbft_message_by_duty_logic(
    validation_context: &ValidationContext<impl SlotClock>,
    consensus_message: &QbftMessage,
    consensus_state: &mut ConsensusState,
    duty_provider: Arc<impl DutiesProvider>,
) -> Result<(), ValidationFailure> {
    let role = validation_context.role;
    let signed_ssv_message = validation_context.signed_ssv_message;

    // Rule: Height must not be "old". I.e., signer must not have already advanced to a later slot.
    if role != Role::Committee {
        for &signer in signed_ssv_message.operator_ids() {
            let signer_state = consensus_state.get_or_create_operator(&signer);
            let max_slot = signer_state.max_slot();
            if max_slot > consensus_message.height {
                return Err(ValidationFailure::SlotAlreadyAdvanced {
                    got: consensus_message.height,
                    want: max_slot.as_u64(),
                });
            }
        }
    }

    let msg_slot = Slot::new(consensus_message.height);
    let randao_msg = false; // Default to false as in the Go code

    validate_beacon_duty(
        validation_context,
        msg_slot,
        randao_msg,
        duty_provider.clone(),
    )?;

    // Rule: current slot(height) must be between duty's starting slot and:
    // - duty's starting slot + 34 (committee and aggregation)
    // - duty's starting slot + 3 (other types)
    validate_slot_time(msg_slot, validation_context)?;

    // Rule: valid number of duties per epoch
    for &signer in signed_ssv_message.operator_ids() {
        let signer_state = consensus_state.get_or_create_operator(&signer);
        validate_duty_count(
            validation_context,
            msg_slot,
            signer_state,
            duty_provider.clone(),
        )?;
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
            && validation_context.slot_clock.now().unwrap_or_default() <= slot
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
            .unwrap_or_default();
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
            .unwrap_or_default();
        if !duty_provider.is_validator_in_sync_committee(period, validator_index) {
            return Err(ValidationFailure::NoDuty);
        }
    }

    Ok(())
}

/// Validates that the message's slot timing is correct
pub(crate) fn validate_slot_time(
    msg_slot: Slot,
    validation_context: &ValidationContext<impl SlotClock>,
) -> Result<(), ValidationFailure> {
    // Check if the message is too early
    let earliness = message_earliness(msg_slot, validation_context)?;
    if earliness > CLOCK_ERROR_TOLERANCE {
        return Err(EarlySlotMessage {
            got: format!("early by {:?}", earliness),
        });
    }

    // Check if the message is too late
    let lateness = message_lateness(msg_slot, validation_context)?;
    if lateness > CLOCK_ERROR_TOLERANCE {
        return Err(ValidationFailure::LateSlotMessage {
            got: format!("late by {:?}", lateness),
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
        Role::Committee | Role::Aggregator => {
            validation_context.slots_per_epoch + LATE_SLOT_ALLOWANCE
        }
        // No lateness check for these roles
        Role::ValidatorRegistration | Role::VoluntaryExit => return Ok(Duration::from_secs(0)),
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

        if duty_count > limit {
            return Err(ValidationFailure::ExcessiveDutyCount {
                got: duty_count,
                limit,
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
            // TODO For voluntary exit, check the stored duties https://github.com/sigp/anchor/issues/277
            // This would need to be adapted to use the actual duty store
            Ok(Some(2))
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

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use bls::{Hash256, PublicKeyBytes};
    use openssl::hash::MessageDigest;
    use ssv_types::{
        consensus::{QbftMessage, QbftMessageType},
        domain_type::DomainType,
        message::{MsgType, SSVMessage, SignedSSVMessage, RSA_SIGNATURE_SIZE},
        msgid::{DutyExecutor, MessageId, Role},
        CommitteeId, OperatorId,
    };
    use ssz::Encode;

    use super::*;
    use crate::{
        tests::{
            create_committee_info, generate_random_rsa_public_keys, FOUR_NODE_COMMITTEE,
            SINGLE_NODE_COMMITTEE,
        },
        validate_ssv_message, ValidatedSSVMessage,
    };

    // Helper struct for directly creating consensus messages for tests
    struct QbftMessageBuilder {
        msg_type: QbftMessageType,
        round: u64,
        identifier: MessageId,
        prepare_justification: Vec<SignedSSVMessage>,
        round_change_justification: Vec<SignedSSVMessage>,
    }

    impl QbftMessageBuilder {
        fn new(role: Role, msg_type: QbftMessageType) -> Self {
            Self {
                msg_type,
                round: 1,
                identifier: create_message_id_for_test(role),
                prepare_justification: vec![],
                round_change_justification: vec![],
            }
        }

        fn with_round(mut self, round: u64) -> Self {
            self.round = round;
            self
        }

        fn with_identifier(mut self, identifier: MessageId) -> Self {
            self.identifier = identifier;
            self
        }

        fn with_prepare_justification(mut self, justifications: Vec<SignedSSVMessage>) -> Self {
            self.prepare_justification = justifications;
            self
        }

        fn with_round_change_justification(
            mut self,
            justifications: Vec<SignedSSVMessage>,
        ) -> Self {
            self.round_change_justification = justifications;
            self
        }

        fn build(self) -> QbftMessage {
            QbftMessage {
                qbft_message_type: self.msg_type,
                height: 1,
                round: self.round,
                identifier: (&self.identifier).into(),
                root: Hash256::from([0u8; 32]),
                data_round: 1,
                round_change_justification: self.round_change_justification,
                prepare_justification: self.prepare_justification,
            }
        }
    }

    struct MockDutiesProvider {}
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
    }

    // Helper for creating SignedSSVMessage with a QbftMessage
    fn create_signed_consensus_message(
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
                .map(|(i, _)| vec![0xAA + i as u8; RSA_SIGNATURE_SIZE])
                .collect::<Vec<_>>()
        } else {
            pks.iter()
                .map(|pk| {
                    let p_key = PKey::from_rsa(pk.clone()).unwrap();
                    let mut signer = Signer::new(MessageDigest::sha256(), &p_key).unwrap();
                    signer.update(&ssv_msg.as_ssz_bytes()).unwrap();
                    signer.sign_to_vec().expect("Failed to sign message")
                })
                .collect::<Vec<_>>()
        };

        SignedSSVMessage::new(signatures, signers, ssv_msg, full_data)
            .expect("SignedSSVMessage should be created")
    }

    fn create_message_id_for_test(role: Role) -> MessageId {
        let domain = DomainType([0, 0, 0, 1]);
        let duty_executor = match role {
            Role::Committee => DutyExecutor::Committee(CommitteeId([0u8; 32])),
            _ => DutyExecutor::Validator(PublicKeyBytes::empty()),
        };
        MessageId::new(&domain, role, &duty_executor)
    }

    // Assert helpers for common validation patterns
    fn assert_validation_error<T, F>(
        result: Result<T, ValidationFailure>,
        expected_error: F,
        error_name: &str,
    ) where
        F: Fn(&ValidationFailure) -> bool,
    {
        match result {
            Ok(_) => panic!("Expected validation to fail with {}", error_name),
            Err(failure) => {
                assert!(
                    expected_error(&failure),
                    "Expected {} error, got: {:?}",
                    error_name,
                    failure
                );
            }
        }
    }

    // Extract common key generation into a helper
    fn generate_test_key_pair() -> (Rsa<Private>, Rsa<Public>) {
        let private_key = Rsa::generate(2048).expect("Failed to generate RSA key");
        let public_key = Rsa::from_public_components(
            private_key.n().to_owned().unwrap(),
            private_key.e().to_owned().unwrap(),
        )
        .expect("Failed to extract public key");
        (private_key, public_key)
    }

    // ---------------------------------------------------------------------
    // validate_ssv_message tests
    // ---------------------------------------------------------------------

    #[test]
    fn test_validate_ssv_message_consensus_success() {
        // Generate a key pair
        let (private_key, public_key) = generate_test_key_pair();
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        let qbft_message =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Proposal).build();
        let signed_msg = create_signed_consensus_message(
            qbft_message,
            vec![OperatorId(2)],
            vec![],
            vec![private_key],
        );

        let now = SystemTime::now();
        let slot_duration = Duration::from_secs(1);
        let slot_clock = ManualSlotClock::new(
            Slot::new(0),
            now.duration_since(UNIX_EPOCH).unwrap(),
            Duration::from_secs(1),
        );
        slot_clock.advance_slot();
        slot_clock.advance_time(slot_duration);

        let validation_context = ValidationContext {
            signed_ssv_message: &signed_msg,
            committee_info: &committee_info,
            role: Role::Committee,
            received_at: now + slot_duration,
            operators_pk: &[public_key],
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            slot_clock,
        };

        let result = validate_ssv_message(
            validation_context,
            &mut ConsensusState::new(2),
            Arc::new(MockDutiesProvider {}),
        );

        match result {
            Ok(ValidatedSSVMessage::QbftMessage(_)) => {} // success
            Err(e) => panic!("Expected successful validation, got: {:?}", e),
            _ => {}
        }

        assert!(result.is_ok(), "Expected successful validation");

        match result.unwrap() {
            ValidatedSSVMessage::QbftMessage(_) => {} // success
            _ => panic!("Expected QbftMessage variant"),
        }
    }

    #[test]
    fn test_early_message_fails_validation() {
        // Generate a key pair
        let (private_key, _) = generate_test_key_pair();
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        let qbft_message =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Proposal).build();
        let signed_msg = create_signed_consensus_message(
            qbft_message,
            vec![OperatorId(2)],
            vec![],
            vec![private_key],
        );

        // Set up slot clock where current time is before slot start time (message too early)
        let now = SystemTime::now();
        let slot_clock = ManualSlotClock::new(
            Slot::new(0),
            // Slot 1 starts in 1 second from now
            now.duration_since(UNIX_EPOCH).unwrap(),
            Duration::from_secs(1),
        );

        let validation_context = ValidationContext {
            signed_ssv_message: &signed_msg,
            committee_info: &committee_info,
            role: Role::Committee,
            received_at: now,
            operators_pk: &[],
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            slot_clock,
        };

        let result = validate_ssv_message(
            validation_context,
            &mut ConsensusState::new(2),
            Arc::new(MockDutiesProvider {}),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, EarlySlotMessage { got: _ }),
            "EarlySlotMessage",
        );
    }

    #[test]
    fn test_late_message_fails_validation() {
        // Generate a key pair
        let (private_key, _) = generate_test_key_pair();
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        let qbft_message =
            QbftMessageBuilder::new(Role::Proposer, QbftMessageType::Proposal).build();
        let signed_msg = create_signed_consensus_message(
            qbft_message,
            vec![OperatorId(2)],
            vec![],
            vec![private_key],
        );

        let now = SystemTime::now();
        let slot_duration = Duration::from_secs(1);
        let slot_clock = ManualSlotClock::new(
            Slot::new(0),
            now.duration_since(UNIX_EPOCH).unwrap(),
            slot_duration,
        );

        let validation_context = ValidationContext {
            signed_ssv_message: &signed_msg,
            committee_info: &committee_info,
            role: Role::Proposer, // Proposer role has TTL = 1 + LATE_SLOT_ALLOWANCE + LATE_MESSAGE_MARGIN. To be late for slot 1, we need to add more 2 seconds (2 * slot duration).
            received_at: now.checked_add(Duration::from_secs(1 + LATE_SLOT_ALLOWANCE + 2)).unwrap().checked_add(LATE_MESSAGE_MARGIN).unwrap(),
            operators_pk: &[],
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            slot_clock,
        };

        let result = validate_ssv_message(
            validation_context,
            &mut ConsensusState::new(2),
            Arc::new(MockDutiesProvider {}),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, LateSlotMessage { got: _ }),
            "LateSlotMessage",
        );
    }

    #[test]
    fn test_validate_ssv_message_invalid_consensus_data() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        // Create invalid consensus message data
        let msg_id = create_message_id_for_test(Role::Committee);
        let invalid_data = vec![0xDE, 0xAD, 0xBE, 0xEF]; // Not valid QBFT data
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, msg_id, invalid_data)
            .expect("SSVMessage should be created");
        let signed_msg = SignedSSVMessage::new(
            vec![vec![0xAA; RSA_SIGNATURE_SIZE]],
            vec![OperatorId(1)],
            ssv_msg,
            vec![],
        )
        .expect("SignedSSVMessage should be created");

        let validation_context = ValidationContext {
            signed_ssv_message: &signed_msg,
            committee_info: &committee_info,
            role: Role::Committee,
            received_at: SystemTime::now(),
            operators_pk: &generate_random_rsa_public_keys(signed_msg.operator_ids().len()),
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            slot_clock: ManualSlotClock::new(
                Slot::new(0),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap(),
                Duration::from_secs(1),
            ),
        };

        let result = validate_ssv_message(
            validation_context,
            &mut ConsensusState::new(2),
            Arc::new(MockDutiesProvider {}),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::UndecodableMessageData),
            "UndecodableMessageData",
        );
    }

    // ---------------------------------------------------------------------
    // Consensus message semantic validation tests
    // ---------------------------------------------------------------------

    #[test]
    fn test_successful_validation_of_consensus_message_with_single_signer() {
        let committee_info = create_committee_info(SINGLE_NODE_COMMITTEE);

        let qbft_message =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Prepare).build();
        let signed_msg = create_signed_consensus_message(
            qbft_message.clone(),
            vec![OperatorId(1)],
            vec![],
            vec![],
        );

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info);

        assert!(
            result.is_ok(),
            "Expected a single-signer Prepare consensus message to validate successfully"
        );
    }

    #[test]
    fn test_consensus_message_with_multiple_signers_but_not_commit() {
        let committee_info = create_committee_info(SINGLE_NODE_COMMITTEE);

        // Multiple signers are only allowed for Commit messages.
        let signers = vec![OperatorId(1), OperatorId(2), OperatorId(3)];
        let qbft_message =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Prepare).build();
        let signed_msg =
            create_signed_consensus_message(qbft_message.clone(), signers.clone(), vec![], vec![]);

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info);

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::NonDecidedWithMultipleSigners { got, want } if *got == signers.len() && *want == SINGLE_NODE_COMMITTEE),
            "NonDecidedWithMultipleSigners",
        );
    }

    #[test]
    fn test_consensus_message_with_multiple_signers_commit_but_not_enough_signers_for_quorum() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        // For Commit messages with multiple signers, the count must be >= quorum size.
        let signers = vec![OperatorId(1), OperatorId(2)]; // Quorum requires at least 3 for a committee of 4.
        let qbft_message =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Commit).build();
        let signed_msg =
            create_signed_consensus_message(qbft_message.clone(), signers.clone(), vec![], vec![]);

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info);

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::DecidedNotEnoughSigners { got, want } if *got == signers.len() && *want == FOUR_NODE_COMMITTEE - 1),
            "DecidedNotEnoughSigners",
        );
    }

    #[test]
    fn test_consensus_message_full_data_mismatched_root_hash() {
        let committee_info = create_committee_info(SINGLE_NODE_COMMITTEE);

        let full_data = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let qbft_message =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Commit).build();
        let signed_msg = create_signed_consensus_message(
            qbft_message.clone(),
            vec![OperatorId(1)],
            full_data,
            vec![],
        );

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info);

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::PrepareOrCommitWithFullData),
            "PrepareOrCommitWithFullData",
        );
    }

    #[test]
    fn test_consensus_message_zero_round_fails() {
        let committee_info = create_committee_info(SINGLE_NODE_COMMITTEE);

        let qbft_message = QbftMessageBuilder::new(Role::Committee, QbftMessageType::Proposal)
            .with_round(0)
            .build();
        let signed_msg = create_signed_consensus_message(
            qbft_message.clone(),
            vec![OperatorId(1)],
            vec![],
            vec![],
        );

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info);

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::ZeroRound),
            "ZeroRound",
        );
    }

    #[test]
    fn test_consensus_message_round_too_high() {
        let committee_info = create_committee_info(SINGLE_NODE_COMMITTEE);

        let qbft_message = QbftMessageBuilder::new(Role::Committee, QbftMessageType::Proposal)
            .with_round(13) // Too high (max is 12)
            .build();
        let signed_msg = create_signed_consensus_message(
            qbft_message.clone(),
            vec![OperatorId(1)],
            vec![],
            vec![],
        );

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info);

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::RoundTooHigh),
            "RoundTooHigh",
        );
    }

    #[test]
    fn test_consensus_message_mismatched_identifier() {
        let committee_info = create_committee_info(SINGLE_NODE_COMMITTEE);

        // Create message with mismatched identifier
        let msg_id_a = create_message_id_for_test(Role::Committee);
        let msg_id_b = create_message_id_for_test(Role::Proposer);

        let qbft_msg = QbftMessage {
            qbft_message_type: QbftMessageType::Proposal,
            height: 1,
            round: 1,
            identifier: (&msg_id_b).into(), // Mismatched ID
            root: Hash256::from([0u8; 32]),
            data_round: 1,
            round_change_justification: vec![],
            prepare_justification: vec![],
        };

        let qbft_bytes = qbft_msg.as_ssz_bytes();
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, msg_id_a, qbft_bytes)
            .expect("SSVMessage should be created");
        let signed_msg = SignedSSVMessage::new(
            vec![vec![0xAA; RSA_SIGNATURE_SIZE]],
            vec![OperatorId(42)],
            ssv_msg,
            vec![],
        )
        .expect("SignedSSVMessage should be created");

        let result = validate_consensus_message_semantics(&signed_msg, &qbft_msg, &committee_info);

        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::MismatchedIdentifier { got: _, want: _ }
                )
            },
            "MismatchedIdentifier",
        );
    }

    #[test]
    fn test_consensus_message_for_non_consensus_role() {
        let committee_info = create_committee_info(SINGLE_NODE_COMMITTEE);

        // Create a consensus message for a non-consensus role (ValidatorRegistration)
        let msg_id = create_message_id_for_test(Role::ValidatorRegistration);
        let qbft_message =
            QbftMessageBuilder::new(Role::ValidatorRegistration, QbftMessageType::Proposal)
                .with_identifier(msg_id.clone())
                .build();

        let qbft_bytes = qbft_message.as_ssz_bytes();
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, msg_id, qbft_bytes)
            .expect("SSVMessage should be created");
        let signed_msg = SignedSSVMessage::new(
            vec![vec![0xAA; RSA_SIGNATURE_SIZE]],
            vec![OperatorId(1)],
            ssv_msg,
            vec![],
        )
        .expect("SignedSSVMessage should be created");

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info);

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::UnexpectedConsensusMessage),
            "UnexpectedConsensusMessage",
        );
    }

    #[test]
    fn test_prepare_justifications_with_non_proposal_message() {
        let committee_info = create_committee_info(SINGLE_NODE_COMMITTEE);

        // Create dummy justification
        let dummy_justification = {
            let dummy_qbft =
                QbftMessageBuilder::new(Role::Committee, QbftMessageType::Prepare).build();
            create_signed_consensus_message(dummy_qbft, vec![OperatorId(1)], vec![], vec![])
        };

        let qbft_message = QbftMessageBuilder::new(Role::Committee, QbftMessageType::Prepare)
            .with_prepare_justification(vec![dummy_justification])
            .build();
        let signed_msg = create_signed_consensus_message(
            qbft_message.clone(),
            vec![OperatorId(1)],
            vec![],
            vec![],
        );

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info);

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::UnexpectedPrepareJustifications),
            "UnexpectedPrepareJustifications",
        );
    }

    #[test]
    fn test_round_change_justifications_with_non_proposal_or_roundchange() {
        let committee_info = create_committee_info(SINGLE_NODE_COMMITTEE);

        // Create dummy justification
        let dummy_justification = {
            let dummy_qbft =
                QbftMessageBuilder::new(Role::Committee, QbftMessageType::RoundChange).build();
            create_signed_consensus_message(dummy_qbft, vec![OperatorId(1)], vec![], vec![])
        };

        let qbft_message = QbftMessageBuilder::new(Role::Committee, QbftMessageType::Commit)
            .with_round_change_justification(vec![dummy_justification])
            .build();
        let signed_msg = create_signed_consensus_message(
            qbft_message.clone(),
            vec![OperatorId(1)],
            vec![],
            vec![],
        );

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info);

        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::UnexpectedRoundChangeJustifications
                )
            },
            "UnexpectedRoundChangeJustifications",
        );
    }

    #[test]
    fn test_consensus_message_multiple_signers_commit_with_full_data_and_invalid_hash() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        // Create a full commit message with quorum signers
        let signers = vec![OperatorId(1), OperatorId(2), OperatorId(3)]; // 3 signers meets quorum for committee of 4
        let full_data = vec![0xFF; 16]; // Some sample full data

        // Root hash doesn't match the actual hash of full_data
        let qbft_message =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Commit).build();
        let signed_msg = create_signed_consensus_message(
            qbft_message.clone(),
            signers.clone(),
            full_data,
            vec![],
        );

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info);

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::InvalidHash),
            "InvalidHash",
        );
    }

    #[test]
    fn test_full_commit_with_matching_hash() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        // Create some data that we'll hash
        let full_data = vec![0xAA, 0xBB, 0xCC, 0xDD];

        // Hash the data to create the root
        let root = hash_data(&full_data);

        // Create a message with the correct root hash
        let signers = vec![OperatorId(1), OperatorId(2), OperatorId(3)]; // 3 signers meets quorum for committee of 4
        let mut qbft_message =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Commit).build();

        // Convert the [u8; 32] hash to Hash256
        qbft_message.root = Hash256::from(root);

        let signed_msg =
            create_signed_consensus_message(qbft_message.clone(), signers, full_data, vec![]);

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info);

        assert!(
            result.is_ok(),
            "Expected successful validation with correct hash"
        );
    }

    #[test]
    fn test_round_robin_proposer() {
        let committee: IndexSet<OperatorId> = vec![OperatorId(1), OperatorId(2), OperatorId(3)]
            .into_iter()
            .collect();

        // Test basic round robin
        assert_eq!(
            round_robin_proposer(0, FIRST_ROUND.into(), &committee).unwrap(),
            OperatorId(1)
        );
        assert_eq!(
            round_robin_proposer(0, (FIRST_ROUND + 1).into(), &committee).unwrap(),
            OperatorId(2)
        );
        assert_eq!(
            round_robin_proposer(0, (FIRST_ROUND + 2).into(), &committee).unwrap(),
            OperatorId(3)
        );
        assert_eq!(
            round_robin_proposer(0, (FIRST_ROUND + 3).into(), &committee).unwrap(),
            OperatorId(1)
        ); // Wraps around

        // Test with different heights
        assert_eq!(
            round_robin_proposer(1, FIRST_ROUND.into(), &committee).unwrap(),
            OperatorId(2)
        );
        assert_eq!(
            round_robin_proposer(2, FIRST_ROUND.into(), &committee).unwrap(),
            OperatorId(3)
        );
    }

    #[test]
    fn test_current_estimated_round() {
        // Test early rounds (quick timeout)
        assert_eq!(current_estimated_round(Duration::from_secs(0)), 1.into());
        assert_eq!(
            current_estimated_round(Duration::from_millis(1999)),
            1.into()
        );
        assert_eq!(current_estimated_round(Duration::from_secs(2)), 2.into());
        assert_eq!(
            current_estimated_round(Duration::from_millis(3999)),
            2.into()
        );

        // Test transition from quick to slow rounds
        // Fix: Calculate the quick phase duration directly
        let quick_phase_time =
            Duration::from_millis(QUICK_TIMEOUT.as_millis() as u64 * QUICK_TIMEOUT_THRESHOLD);

        assert_eq!(
            current_estimated_round(quick_phase_time - Duration::from_millis(1)),
            QUICK_TIMEOUT_THRESHOLD.into()
        );
        assert_eq!(
            current_estimated_round(quick_phase_time),
            (QUICK_TIMEOUT_THRESHOLD + 1).into()
        );

        // Test slow rounds
        assert_eq!(
            current_estimated_round(quick_phase_time + SLOW_TIMEOUT - Duration::from_millis(1)),
            (QUICK_TIMEOUT_THRESHOLD + 1).into()
        );
        assert_eq!(
            current_estimated_round(quick_phase_time + SLOW_TIMEOUT),
            (QUICK_TIMEOUT_THRESHOLD + 2).into()
        );
    }

    // ---------------------------------------------------------------------
    // Signature verification tests
    // ---------------------------------------------------------------------

    use openssl::{
        pkey::{PKey, Private, Public},
        rsa::Rsa,
        sign::Signer,
    };
    use slot_clock::ManualSlotClock;
    use types::Epoch;

    use crate::ValidationFailure::LateSlotMessage;

    #[test]
    fn test_verify_message_signatures_success() {
        // Generate a key pair
        let (private_key, public_key) = generate_test_key_pair();

        // Create a message
        let qbft_message =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Proposal).build();
        let msg_id = create_message_id_for_test(Role::Committee);
        let qbft_bytes = qbft_message.as_ssz_bytes();
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, msg_id, qbft_bytes)
            .expect("SSVMessage should be created");

        // Sign the message
        let p_key = PKey::from_rsa(private_key).expect("Failed to create PKey");
        let mut signer =
            Signer::new(MessageDigest::sha256(), &p_key).expect("Failed to create signer");
        signer
            .update(&ssv_msg.as_ssz_bytes())
            .expect("Failed to update signer");
        let signature = signer.sign_to_vec().expect("Failed to create signature");

        // Pad signature to RSA_SIGNATURE_SIZE if needed
        let padded_signature = if signature.len() < RSA_SIGNATURE_SIZE {
            let mut padded = vec![0; RSA_SIGNATURE_SIZE];
            padded[..signature.len()].copy_from_slice(&signature);
            padded
        } else {
            signature
        };

        // Create signed message
        let signed_msg =
            SignedSSVMessage::new(vec![padded_signature], vec![OperatorId(1)], ssv_msg, vec![])
                .expect("SignedSSVMessage should be created");

        // Verify signatures
        let result = verify_message_signatures(&signed_msg, &[public_key]);
        assert!(result.is_ok(), "Expected successful signature verification");
    }

    #[test]
    fn test_verify_message_signatures_count_mismatch() {
        let qbft_message =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Proposal).build();
        let signed_msg = create_signed_consensus_message(
            qbft_message,
            vec![OperatorId(1), OperatorId(2)],
            vec![],
            vec![],
        );

        // Provide only one key when we have two signatures
        let rsa_keys = generate_random_rsa_public_keys(1);

        let result = verify_message_signatures(&signed_msg, &rsa_keys);

        assert_validation_error(
            result,
            |failure| {
                if let ValidationFailure::SignatureVerificationFailed { reason } = failure {
                    reason.contains("Signature count doesn't match operator count")
                } else {
                    false
                }
            },
            "SignatureVerificationFailed: count mismatch",
        );
    }

    #[test]
    fn test_verify_message_signatures_invalid_signature() {
        // Generate a key pair
        let (_, public_key) = generate_test_key_pair();

        // Create a message
        let qbft_message =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Proposal).build();
        let msg_id = create_message_id_for_test(Role::Committee);
        let qbft_bytes = qbft_message.as_ssz_bytes();
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, msg_id, qbft_bytes)
            .expect("SSVMessage should be created");

        // Create an invalid signature (just random bytes)
        let invalid_signature = vec![0xBB; RSA_SIGNATURE_SIZE];

        // Create signed message with invalid signature
        let signed_msg = SignedSSVMessage::new(
            vec![invalid_signature],
            vec![OperatorId(1)],
            ssv_msg,
            vec![],
        )
        .expect("SignedSSVMessage should be created");

        // Verify should fail
        let result = verify_message_signatures(&signed_msg, &[public_key]);

        assert!(result.is_err(), "Expected signature verification to fail");
        assert_validation_error(
            result,
            |failure| {
                if let ValidationFailure::SignatureVerificationFailed { reason } = failure {
                    reason.contains("Signature verification failed")
                } else {
                    false
                }
            },
            "SignatureVerificationFailed: invalid signature",
        );
    }

    #[test]
    fn test_verify_message_signatures_pkey_creation_error() {
        let qbft_message =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Proposal).build();
        let signed_msg =
            create_signed_consensus_message(qbft_message, vec![OperatorId(1)], vec![], vec![]);

        // Create an invalid RSA key that will fail when creating PKey
        let invalid_rsa = Rsa::generate(512).expect("Failed to generate RSA key");
        let invalid_key = Rsa::from_public_components(
            invalid_rsa.n().to_owned().unwrap(),
            // Using n as e will make the key invalid
            invalid_rsa.n().to_owned().unwrap(),
        )
        .expect("Failed to create invalid key");

        let result = verify_message_signatures(&signed_msg, &[invalid_key]);

        assert!(result.is_err(), "Expected PKey creation to fail");
    }
}
