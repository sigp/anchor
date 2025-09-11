use std::{collections::HashMap, convert::Into, sync::Arc, time::Duration};

use duties_tracker::DutiesProvider;
use openssl::{pkey::Public, rsa::Rsa};
use slot_clock::SlotClock;
use ssv_types::{
    CommitteeInfo, IndexSet, OperatorId, Round, Slot, VariableList,
    consensus::{QbftMessage, QbftMessageType},
    message::SignedSSVMessage,
    msgid::Role,
};
use ssz::Decode;

use crate::{
    FIRST_ROUND, ValidatedSSVMessage, ValidationContext, ValidationFailure, compute_quorum_size,
    duty_state::DutyState, hash_data, slot_start_time, validate_beacon_duty, validate_duty_count,
    validate_slot_time, verify_message_signatures,
};

pub(crate) fn validate_consensus_message(
    validation_context: ValidationContext<impl SlotClock>,
    duty_state: &mut DutyState,
    duty_provider: Arc<impl DutiesProvider>,
) -> Result<ValidatedSSVMessage, ValidationFailure> {
    // Decode message to QbftMessage
    let consensus_message = match QbftMessage::from_ssz_bytes(
        validation_context.signed_ssv_message.ssv_message().data(),
    ) {
        Ok(msg) => msg,
        Err(err) => return Err(ValidationFailure::UndecodableMessageData(err)),
    };

    // Call the existing semantic validation
    validate_consensus_message_semantics(
        validation_context.signed_ssv_message,
        &consensus_message,
        validation_context.committee_info,
        validation_context.operator_pub_keys,
    )?;

    validate_qbft_logic(&validation_context, &consensus_message, duty_state)?;

    validate_qbft_message_by_duty_logic(
        &validation_context,
        &consensus_message,
        duty_state,
        duty_provider,
    )?;

    verify_message_signatures(
        validation_context.signed_ssv_message,
        validation_context.operator_pub_keys,
    )?;

    duty_state.update_for_consensus_message(
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
    operator_pub_keys: &HashMap<OperatorId, Rsa<Public>>,
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

    validate_justifications(consensus_message, operator_pub_keys)?;

    Ok(())
}

pub(crate) fn validate_justifications(
    consensus_message: &QbftMessage,
    operator_pub_keys: &HashMap<OperatorId, Rsa<Public>>,
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

    prepare_justifications
        .iter()
        .chain(round_change_justifications.iter())
        .try_for_each(|signed_message| {
            verify_message_signatures(signed_message, operator_pub_keys)
        })?;

    Ok(())
}

#[allow(clippy::comparison_chain)]
pub(crate) fn validate_qbft_logic(
    validation_context: &ValidationContext<impl SlotClock>,
    consensus_message: &QbftMessage,
    duty_state: &mut DutyState,
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
        let Some(signer_state) = duty_state
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
                // Rule: Peer must not send two proposals with different data.
                // We separately verify that the root in the message matches the data.
                if !signed_ssv_message.full_data().is_empty()
                    && signer_state
                        .proposal_hash
                        .as_ref()
                        .is_some_and(|hash| hash != consensus_message.root)
                {
                    return Err(ValidationFailure::DifferentProposalData);
                }

                signer_state
                    .message_counts
                    .validate_consensus_message_limits(
                        signed_ssv_message,
                        consensus_message.qbft_message_type,
                    )?;
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
    let highest_allowed =
        (estimated_round + MAX_ALLOWED_ROUNDS_FUTURE).ok_or(ValidationFailure::RoundOverflow)?;

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

/// Validates QBFT messages based on beacon chain duties
pub(crate) fn validate_qbft_message_by_duty_logic(
    validation_context: &ValidationContext<impl SlotClock>,
    consensus_message: &QbftMessage,
    duty_state: &mut DutyState,
    duty_provider: Arc<impl DutiesProvider>,
) -> Result<(), ValidationFailure> {
    let role = validation_context.role;
    let signed_ssv_message = validation_context.signed_ssv_message;

    // Rule: Height must not be "old". I.e., signer must not have already advanced to a later slot.
    if role != Role::Committee {
        for &signer in signed_ssv_message.operator_ids() {
            let signer_state = duty_state.get_or_create_operator(&signer);
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
        let signer_state = duty_state.get_or_create_operator(&signer);
        validate_duty_count(
            validation_context,
            msg_slot,
            signer_state,
            duty_provider.clone(),
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use bls::{Hash256, PublicKeyBytes};
    use openssl::hash::MessageDigest;
    use ssv_types::{
        OperatorId,
        consensus::{QbftMessage, QbftMessageType},
        domain_type::DomainType,
        message::{MsgType, RSA_SIGNATURE_SIZE, SSVMessage, SignedSSVMessage},
        msgid::{DutyExecutor, MessageId, Role},
    };
    use ssz::Encode;

    use super::*;
    use crate::{
        LATE_MESSAGE_MARGIN, LATE_SLOT_ALLOWANCE, ValidatedSSVMessage, duty_limit,
        tests::{
            FOUR_NODE_COMMITTEE, SINGLE_NODE_COMMITTEE, create_committee_info,
            create_operator_pub_keys, generate_random_rsa_public_keys,
        },
        validate_ssv_message,
    };

    // Assert helpers for common validation patterns
    fn assert_validation_error<T, F>(
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
        let (_private_key, public_key) = generate_test_key_pair();
        let (private_key2, public_key2) = generate_test_key_pair();
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let map = create_operator_pub_keys(
            committee_info.committee_members.clone(),
            vec![public_key, public_key2],
        );

        let qbft_message =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Proposal).build();
        let signed_msg = create_signed_consensus_message(
            qbft_message,
            vec![OperatorId(2)],
            vec![],
            vec![private_key2],
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
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock,
            operator_pub_keys: &map,
        };

        let expected_duty_count = 5;
        let result = validate_ssv_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: expected_duty_count,
            }),
        );

        match result {
            Ok(ValidatedSSVMessage::QbftMessage(_)) => {} // success
            Err(e) => panic!("Expected successful validation, got: {e:?}"),
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
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock,
            operator_pub_keys: &HashMap::new(),
        };

        let result = validate_ssv_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
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
            received_at: now
                .checked_add(Duration::from_secs(1 + LATE_SLOT_ALLOWANCE + 2))
                .unwrap()
                .checked_add(LATE_MESSAGE_MARGIN)
                .unwrap(),
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock,
            operator_pub_keys: &HashMap::new(),
        };

        let result = validate_ssv_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
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

        let public_keys = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), public_keys);

        let validation_context = ValidationContext {
            signed_ssv_message: &signed_msg,
            committee_info: &committee_info,
            role: Role::Committee,
            received_at: SystemTime::now(),
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock: ManualSlotClock::new(
                Slot::new(0),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap(),
                Duration::from_secs(1),
            ),
            operator_pub_keys: &map,
        };

        let result = validate_ssv_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::UndecodableMessageData(_)),
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

        let map = create_operator_pub_keys(committee_info.committee_members.clone(), vec![]);

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info, &map);

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

        let map = create_operator_pub_keys(committee_info.committee_members.clone(), vec![]);

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info, &map);

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

        let map = create_operator_pub_keys(committee_info.committee_members.clone(), vec![]);

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info, &map);

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

        let map = create_operator_pub_keys(committee_info.committee_members.clone(), vec![]);

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info, &map);

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

        let map = create_operator_pub_keys(committee_info.committee_members.clone(), vec![]);

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info, &map);

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

        let map = create_operator_pub_keys(committee_info.committee_members.clone(), vec![]);

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info, &map);

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

        let map = create_operator_pub_keys(committee_info.committee_members.clone(), vec![]);

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_msg, &committee_info, &map);

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

        let map = create_operator_pub_keys(committee_info.committee_members.clone(), vec![]);

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info, &map);

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

        let map = create_operator_pub_keys(committee_info.committee_members.clone(), vec![]);

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info, &map);

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

        let map = create_operator_pub_keys(committee_info.committee_members.clone(), vec![]);

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info, &map);

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

        let map = create_operator_pub_keys(committee_info.committee_members.clone(), vec![]);

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info, &map);

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

        let map = create_operator_pub_keys(committee_info.committee_members.clone(), vec![]);

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info, &map);

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

    use crate::{
        ValidationFailure::{EarlySlotMessage, LateSlotMessage},
        tests::{
            MockDutiesProvider, QbftMessageBuilder, create_message_id_for_test,
            create_signed_consensus_message,
        },
    };

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

        let mut committee = IndexSet::new();
        committee.insert(OperatorId(1));
        let map = create_operator_pub_keys(committee, vec![public_key]);

        // Verify signatures
        let result = verify_message_signatures(&signed_msg, &map);
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

        let mut committee = IndexSet::new();
        committee.insert(OperatorId(1));
        committee.insert(OperatorId(2));
        let map = create_operator_pub_keys(committee, rsa_keys);

        let result = verify_message_signatures(&signed_msg, &map);

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

        let mut committee = IndexSet::new();
        committee.insert(OperatorId(1));
        let map = create_operator_pub_keys(committee, vec![public_key]);

        // Verify should fail
        let result = verify_message_signatures(&signed_msg, &map);

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

        let mut committee = IndexSet::new();
        committee.insert(OperatorId(1));
        let map = create_operator_pub_keys(committee, vec![invalid_key]);

        let result = verify_message_signatures(&signed_msg, &map);

        assert!(result.is_err(), "Expected PKey creation to fail");
    }

    #[test]
    fn test_duty_limit_voluntary_exit() {
        // Create a mock SlotClock implementation
        let now = SystemTime::now();
        let slot_clock = ManualSlotClock::new(
            Slot::new(100),
            now.duration_since(UNIX_EPOCH).unwrap(),
            Duration::from_secs(1),
        );

        // Create a validator public key to test with
        let validator_pubkey = PublicKeyBytes::empty();

        // Create a message ID with the validator as duty executor
        let msg_id = MessageId::new(
            &DomainType([0, 0, 0, 1]),
            Role::VoluntaryExit,
            &DutyExecutor::Validator(validator_pubkey),
        );

        // Create an SSV message with this message ID
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, msg_id, vec![1, 2, 3])
            .expect("SSVMessage should be created");

        // Create a signed SSV message
        let signed_msg = SignedSSVMessage::new(
            vec![vec![0xAA; RSA_SIGNATURE_SIZE]],
            vec![OperatorId(1)],
            ssv_msg,
            vec![],
        )
        .expect("SignedSSVMessage should be created");

        // Create committee info
        let committee_info = create_committee_info(SINGLE_NODE_COMMITTEE);

        // Create a mock DutiesProvider that returns a fixed value for voluntary exits
        let expected_duty_count = 5;
        let mock_duties_provider = Arc::new(MockDutiesProvider {
            voluntary_exit_duty_count: expected_duty_count,
        });

        let map = create_operator_pub_keys(committee_info.committee_members.clone(), vec![]);

        // Create the validation context with voluntary exit role
        let validation_context = ValidationContext {
            signed_ssv_message: &signed_msg,
            committee_info: &committee_info,
            role: Role::VoluntaryExit,
            received_at: now,
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock: slot_clock.clone(),
            operator_pub_keys: &map,
        };

        let slot = slot_clock.now().unwrap();

        let result = duty_limit(&validation_context, slot, &[], mock_duties_provider);

        assert_eq!(result, Ok(Some(expected_duty_count)));
    }
}
