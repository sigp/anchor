use std::{collections::HashMap, sync::Arc};

use duties_tracker::DutiesProvider;
use slot_clock::SlotClock;
use ssv_types::{
    OperatorId,
    msgid::Role,
    partial_sig::{PartialSignatureKind, PartialSignatureMessages},
};
use ssz::Decode;

use crate::{
    ValidatedSSVMessage, ValidationContext, ValidationFailure, duty_state::DutyState,
    validate_beacon_duty, validate_duty_count, validate_slot_time, verify_message_signature,
};

// Constants for validation rules
const MAX_SIGNATURES_IN_SYNC_COMMITTEE: usize = 13;

pub(crate) fn validate_partial_signature_message(
    validation_context: ValidationContext<impl SlotClock>,
    duty_state: &mut DutyState,
    duty_provider: Arc<impl DutiesProvider>,
) -> Result<ValidatedSSVMessage, ValidationFailure> {
    // Decode message directly to PartialSignatureMessages
    let messages = match PartialSignatureMessages::from_ssz_bytes(
        validation_context.signed_ssv_message.ssv_message().data(),
    ) {
        Ok(msgs) => msgs,
        Err(err) => return Err(ValidationFailure::UndecodableMessageData(err)),
    };

    // Validate basic semantics
    let signer = validate_partial_signature_message_semantics(&validation_context, &messages)?;

    // Validate duty-specific logic
    validate_partial_sig_messages_by_duty_logic(
        &validation_context,
        &messages,
        duty_state,
        duty_provider,
    )?;

    let operator_pub_keys = validation_context.operator_pub_keys.get(&signer).ok_or(
        ValidationFailure::OperatorNotFound {
            operator_id: signer,
        },
    )?;

    let signature = validation_context
        .signed_ssv_message
        .signatures()
        .first()
        .ok_or(ValidationFailure::NoSignatures)?;

    verify_message_signature(
        validation_context.signed_ssv_message,
        operator_pub_keys,
        signature,
    )?;

    // Update the duty state with information about this partial signature message
    let signer = validation_context
        .signed_ssv_message
        .operator_ids()
        .first()
        .ok_or(ValidationFailure::NoSigners)?;

    duty_state.update_for_partial_signature(
        &messages,
        signer,
        validation_context.slots_per_epoch,
    )?;

    Ok(ValidatedSSVMessage::PartialSignatureMessages(messages))
}

fn validate_partial_signature_message_semantics(
    validation_context: &ValidationContext<impl SlotClock>,
    partial_signature_messages: &PartialSignatureMessages,
) -> Result<OperatorId, ValidationFailure> {
    // Rule: Partial Signature message must have 1 signer
    let signers = validation_context.signed_ssv_message.operator_ids();
    if signers.len() != 1 {
        return Err(ValidationFailure::PartialSigOneSigner);
    }

    let signer = signers[0];

    // Rule: Partial signature message must not have full data
    if !validation_context.signed_ssv_message.full_data().is_empty() {
        return Err(ValidationFailure::FullDataNotInConsensusMessage);
    }

    // Rule: Partial signature type must match expected type for role
    if !partial_signature_type_matches_role(
        partial_signature_messages.kind,
        validation_context.role,
    ) {
        return Err(ValidationFailure::PartialSignatureTypeRoleMismatch);
    }

    // Rule: Partial signature message must have at least one signature
    if partial_signature_messages.messages.is_empty() {
        return Err(ValidationFailure::NoPartialSignatureMessages);
    }

    // Validate each individual message
    for message in &partial_signature_messages.messages {
        // Rule: Partial signature signer must be consistent
        if message.signer != signer {
            return Err(ValidationFailure::InconsistentSigners);
        }

        // Rule: (only for Validator duties) Validator index must match with validatorPK
        // For Committee duties, we don't assume that operators are synced on the validators set
        if !(validation_context.role == Role::Committee)
            && !validation_context
                .committee_info
                .validator_indices
                .is_empty()
            && !validation_context
                .committee_info
                .validator_indices
                .contains(&message.validator_index)
        {
            return Err(ValidationFailure::ValidatorIndexMismatch);
        }
    }

    Ok(signer)
}

fn partial_signature_type_matches_role(kind: PartialSignatureKind, role: Role) -> bool {
    match role {
        Role::Committee => kind == PartialSignatureKind::PostConsensus,
        Role::Aggregator => {
            kind == PartialSignatureKind::PostConsensus
                || kind == PartialSignatureKind::SelectionProofPartialSig
        }
        Role::Proposer => {
            kind == PartialSignatureKind::PostConsensus
                || kind == PartialSignatureKind::RandaoPartialSig
        }
        Role::SyncCommittee => {
            kind == PartialSignatureKind::PostConsensus
                || kind == PartialSignatureKind::ContributionProofs
        }
        Role::ValidatorRegistration => kind == PartialSignatureKind::ValidatorRegistration,
        Role::VoluntaryExit => kind == PartialSignatureKind::VoluntaryExit,
    }
}

/// Validates partial signature messages based on duty logic.
fn validate_partial_sig_messages_by_duty_logic(
    validation_context: &ValidationContext<impl SlotClock>,
    partial_signature_messages: &PartialSignatureMessages,
    duty_state: &mut DutyState,
    duty_provider: Arc<impl DutiesProvider>,
) -> Result<(), ValidationFailure> {
    let role = validation_context.role;
    let message_slot = partial_signature_messages.slot;
    let signed_message = validation_context.signed_ssv_message;

    // Get the operator ID (signer)
    let signer = signed_message
        .operator_ids()
        .first()
        .ok_or(ValidationFailure::NoSigners)?;

    // Get duty state for this signer
    let operator_state = duty_state.get_or_create_operator(signer);

    // Rule: Slot must not be "old" - signer must not have already advanced to a later slot
    // Skip for committee role
    if role != Role::Committee {
        let max_slot = operator_state.max_slot();
        if max_slot.as_u64() != 0 && max_slot > message_slot {
            return Err(ValidationFailure::SlotAlreadyAdvanced {
                got: message_slot.as_u64(),
                want: max_slot.as_u64(),
            });
        }
    }

    let is_randao_msg = partial_signature_messages.kind == PartialSignatureKind::RandaoPartialSig;
    validate_beacon_duty(
        validation_context,
        message_slot,
        is_randao_msg,
        duty_provider.clone(),
    )?;

    // Check if we've seen messages for this slot already
    if let Some(signer_state) = operator_state.get_signer_state(&message_slot) {
        // Rule: peer must send only:
        // - 1 PostConsensusPartialSig, for Committee duty
        // - 1 RandaoPartialSig and 1 PostConsensusPartialSig for Proposer
        // - 1 SelectionProofPartialSig and 1 PostConsensusPartialSig for Aggregator
        // - 1 SelectionProofPartialSig and 1 PostConsensusPartialSig for Sync committee
        //   contribution
        // - 1 ValidatorRegistrationPartialSig for Validator Registration
        // - 1 VoluntaryExitPartialSig for Voluntary Exit
        signer_state
            .message_counts
            .validate_partial_signature_message(partial_signature_messages)?;
    }

    // Check timing constraints
    validate_slot_time(message_slot, validation_context)?;

    // Validate duty count
    validate_duty_count(
        validation_context,
        message_slot,
        operator_state,
        duty_provider.clone(),
    )?;

    // Process role-specific message count constraints
    let validator_count = validation_context.committee_info.validator_indices.len();
    let message_count = partial_signature_messages.messages.len();

    match role {
        Role::Committee => {
            // Rule: Number of signatures must be <= min(2*V, V + SYNC_COMMITTEE_SIZE)
            let max_allowed = std::cmp::min(
                2 * validator_count,
                validator_count + validation_context.sync_committee_size,
            );

            if message_count > max_allowed {
                return Err(ValidationFailure::TooManyPartialSignatureMessages {
                    got: message_count,
                    limit: max_allowed,
                });
            }

            // Rule: A validator index can't appear more than 2 times
            let mut validator_index_count = HashMap::new();
            for message in &partial_signature_messages.messages {
                let count = validator_index_count
                    .entry(message.validator_index)
                    .or_insert(0);
                *count += 1;
                if *count > 2 {
                    return Err(ValidationFailure::TripleValidatorIndexInPartialSignatures);
                }
            }
        }
        Role::SyncCommittee if message_count > MAX_SIGNATURES_IN_SYNC_COMMITTEE => {
            // Rule: Number of signatures must be <= MAX_SIGNATURES_IN_SYNC_COMMITTEE
            return Err(ValidationFailure::TooManyPartialSignatureMessages {
                got: message_count,
                limit: MAX_SIGNATURES_IN_SYNC_COMMITTEE,
            });
        }
        _ if message_count > 1 => {
            // Rule: For other duties, only one signature is allowed
            return Err(ValidationFailure::TooManyPartialSignatureMessages {
                got: message_count,
                limit: 1,
            });
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use bls::{Hash256, Signature};
    use openssl::{
        hash::MessageDigest,
        pkey::{PKey, Private, Public},
        rsa::Rsa,
        sign::Signer,
    };
    use slot_clock::{ManualSlotClock, SlotClock};
    use ssv_types::{
        OperatorId, RSA_SIGNATURE_SIZE, ValidatorIndex,
        message::{MsgType, SSVMessage, SignedSSVMessage},
        partial_sig::PartialSignatureMessage,
    };
    use ssz::Encode;
    use types::Slot;

    use super::*;
    use crate::tests::{
        FOUR_NODE_COMMITTEE, MockDutiesProvider, assert_validation_error, create_committee_info,
        create_message_id_for_test, create_operator_pub_keys, generate_random_rsa_public_keys,
    };

    // Options for creating test partial signature messages
    #[derive(Default)]
    pub struct PartialSigTestOptions {
        pub add_full_data: bool,
        pub different_message_signer: Option<OperatorId>,
        pub empty_messages: bool,
        pub validator_index: Option<ValidatorIndex>,
    }

    // Helper to create a partial signature message for testing
    pub fn create_test_partial_signature(
        role: Role,
        kind: PartialSignatureKind,
        signer: OperatorId,
        options: PartialSigTestOptions,
        operator_pk: Option<Rsa<Private>>,
    ) -> (PartialSignatureMessages, SignedSSVMessage) {
        let message_signer = options.different_message_signer.unwrap_or(signer);

        let messages = if options.empty_messages {
            vec![]
        } else {
            vec![PartialSignatureMessage {
                partial_signature: Signature::empty(),
                signing_root: Hash256::from([0u8; 32]),
                signer: message_signer,
                validator_index: options.validator_index.unwrap_or(ValidatorIndex(0)),
            }]
        };

        let partial_sig_messages = PartialSignatureMessages {
            kind,
            slot: Slot::new(0),
            messages: messages.into(),
        };

        let msg_id = create_message_id_for_test(role);
        let ssv_msg_data = partial_sig_messages.as_ssz_bytes();
        let ssv_msg = SSVMessage::new(MsgType::SSVPartialSignatureMsgType, msg_id, ssv_msg_data)
            .expect("SSVMessage should be created");

        let full_data = if options.add_full_data {
            vec![0xCC; 32]
        } else {
            vec![]
        };

        let signature = if let Some(pk) = operator_pk {
            let p_key = PKey::from_rsa(pk.clone()).unwrap();
            let mut signer = Signer::new(MessageDigest::sha256(), &p_key).unwrap();
            signer.update(&ssv_msg.as_ssz_bytes()).unwrap();
            vec![
                signer
                    .sign_to_vec()
                    .expect("Failed to sign message")
                    .try_into()
                    .expect("Signature should be 256 bytes"),
            ]
        } else {
            vec![[0xAA; RSA_SIGNATURE_SIZE]]
        };

        let signed_msg = SignedSSVMessage::new(signature, vec![signer], ssv_msg, full_data)
            .expect("SignedSSVMessage should be created");

        (partial_sig_messages, signed_msg)
    }

    // Import helper function from consensus_message tests or redefine here
    fn generate_test_key_pair() -> (Rsa<Private>, Rsa<Public>) {
        let private_key = Rsa::generate(2048).expect("Failed to generate RSA key");
        let public_key = Rsa::from_public_components(
            private_key.n().to_owned().unwrap(),
            private_key.e().to_owned().unwrap(),
        )
        .expect("Failed to extract public key");
        (private_key, public_key)
    }

    // Helper function to create a ValidationContext for testing
    fn create_test_validation_context<'a>(
        signed_msg: &'a SignedSSVMessage,
        committee_info: &'a crate::CommitteeInfo,
        role: Role,
        operator_pub_keys: &'a HashMap<OperatorId, Rsa<Public>>,
    ) -> ValidationContext<'a, ManualSlotClock> {
        ValidationContext {
            signed_ssv_message: signed_msg,
            committee_info,
            role,
            received_at: SystemTime::now(),
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock: ManualSlotClock::new(
                Slot::new(0),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap(),
                Duration::from_secs(1),
            ),
            operator_pub_keys,
        }
    }

    #[test]
    fn test_partial_signature_message_with_invalid_type_for_role() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        let (_, signed_msg) = create_test_partial_signature(
            Role::Committee,
            PartialSignatureKind::RandaoPartialSig, // Invalid for Committee role
            OperatorId(1),
            PartialSigTestOptions::default(),
            None,
        );

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);

        let validation_context =
            create_test_validation_context(&signed_msg, &committee_info, Role::Committee, &map);

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::PartialSignatureTypeRoleMismatch),
            "PartialSignatureTypeRoleMismatch",
        );
    }

    #[test]
    fn test_partial_signature_message_with_multiple_signers() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        let (messages, _) = create_test_partial_signature(
            Role::Proposer,
            PartialSignatureKind::RandaoPartialSig,
            OperatorId(1),
            PartialSigTestOptions::default(),
            None,
        );

        // Create a new SignedSSVMessage with multiple signers
        let ssv_msg_data = messages.as_ssz_bytes();
        let msg_id = create_message_id_for_test(Role::Proposer);
        let ssv_msg = SSVMessage::new(MsgType::SSVPartialSignatureMsgType, msg_id, ssv_msg_data)
            .expect("SSVMessage should be created");

        // Multiple signers - this should fail
        let signers = vec![OperatorId(1), OperatorId(2)];
        let signatures = vec![[0xAA; RSA_SIGNATURE_SIZE], [0xBB; RSA_SIGNATURE_SIZE]];

        let signed_msg = SignedSSVMessage::new(signatures, signers, ssv_msg, vec![])
            .expect("SignedSSVMessage should be created");

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);

        let validation_context =
            create_test_validation_context(&signed_msg, &committee_info, Role::Proposer, &map);

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::PartialSigOneSigner),
            "PartialSigOneSigner",
        );
    }

    #[test]
    fn test_partial_signature_message_with_full_data() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        let (_, signed_msg) = create_test_partial_signature(
            Role::Proposer,
            PartialSignatureKind::RandaoPartialSig,
            OperatorId(1),
            PartialSigTestOptions {
                add_full_data: true,
                ..Default::default()
            },
            None,
        );

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);

        let validation_context =
            create_test_validation_context(&signed_msg, &committee_info, Role::Proposer, &map);

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::FullDataNotInConsensusMessage),
            "FullDataNotInConsensusMessage",
        );
    }

    #[test]
    fn test_partial_signature_message_inconsistent_signers() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        let (_, signed_msg) = create_test_partial_signature(
            Role::Proposer,
            PartialSignatureKind::RandaoPartialSig,
            OperatorId(1),
            PartialSigTestOptions {
                different_message_signer: Some(OperatorId(42)),
                ..Default::default()
            },
            None,
        );

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);

        let validation_context =
            create_test_validation_context(&signed_msg, &committee_info, Role::Proposer, &map);

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::InconsistentSigners),
            "InconsistentSigners",
        );
    }

    #[test]
    fn test_partial_signature_message_no_messages() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        let (_, signed_msg) = create_test_partial_signature(
            Role::Proposer,
            PartialSignatureKind::RandaoPartialSig,
            OperatorId(1),
            PartialSigTestOptions {
                empty_messages: true,
                ..Default::default()
            },
            None,
        );

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);

        let validation_context =
            create_test_validation_context(&signed_msg, &committee_info, Role::Proposer, &map);

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::NoPartialSignatureMessages),
            "NoPartialSignatureMessages",
        );
    }

    #[test]
    fn test_partial_signature_message_successful() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();

        let (_, signed_msg) = create_test_partial_signature(
            Role::Proposer,
            PartialSignatureKind::RandaoPartialSig,
            OperatorId(1),
            PartialSigTestOptions::default(),
            Some(private_key),
        );

        let binding = [public_key];
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), binding.to_vec());

        let validation_context =
            create_test_validation_context(&signed_msg, &committee_info, Role::Proposer, &map);

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert!(
            result.is_ok(),
            "{}",
            format!("Expected successful validation but got: {result:?}")
        );

        if let Ok(ValidatedSSVMessage::PartialSignatureMessages(messages)) = result {
            assert_eq!(messages.kind, PartialSignatureKind::RandaoPartialSig);
            assert_eq!(messages.messages.len(), 1);
            assert_eq!(messages.messages[0].signer, OperatorId(1));
        } else {
            panic!("Expected PartialSignatureMessages in successful validation");
        }
    }

    #[test]
    fn test_validator_index_mismatch() {
        // Create committee info with specific validator indices
        let mut committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        committee_info.validator_indices = vec![ValidatorIndex(10), ValidatorIndex(20)];

        let (_, signed_msg) = create_test_partial_signature(
            Role::Proposer,
            PartialSignatureKind::RandaoPartialSig,
            OperatorId(1),
            PartialSigTestOptions {
                validator_index: Some(ValidatorIndex(30)), // Not in committee
                ..Default::default()
            },
            None,
        );

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);

        let validation_context = create_test_validation_context(
            &signed_msg,
            &committee_info,
            Role::Proposer, // Not a committee role, so validator index is checked
            &map,
        );

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::ValidatorIndexMismatch),
            "ValidatorIndexMismatch",
        );
    }

    #[test]
    fn test_committee_role_skips_validator_index_check() {
        // Create committee info with specific validator indices
        let mut committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        committee_info.validator_indices = vec![ValidatorIndex(10), ValidatorIndex(20)];

        let (private_key, public_key) = generate_test_key_pair();

        let (_, signed_msg) = create_test_partial_signature(
            Role::Committee,
            PartialSignatureKind::PostConsensus, // Valid for Committee role
            OperatorId(1),
            PartialSigTestOptions {
                validator_index: Some(ValidatorIndex(30)), /* Not in committee, but ignored for
                                                            * Committee role */
                ..Default::default()
            },
            Some(private_key),
        );

        let binding = [public_key];
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), binding.to_vec());

        let validation_context = create_test_validation_context(
            &signed_msg,
            &committee_info,
            Role::Committee, // Committee role, so validator index is not checked
            &map,
        );

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert!(
            result.is_ok(),
            "{}",
            format!("Expected successful validation for Committee role, but got: {result:?}")
        );
    }

    fn create_partial_signature_messages() -> Vec<PartialSignatureMessage> {
        let mut messages = vec![];
        for _ in 0..3 {
            messages.push(PartialSignatureMessage {
                partial_signature: Signature::empty(),
                signing_root: Hash256::from([0u8; 32]),
                signer: OperatorId(1),
                validator_index: ValidatorIndex(0),
            });
        }
        messages
    }

    #[test]
    fn test_too_many_partial_signature_messages() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        // Create messages with more than allowed count
        let messages = create_partial_signature_messages();

        let partial_sig_messages = PartialSignatureMessages {
            kind: PartialSignatureKind::PostConsensus,
            slot: Slot::new(0),
            messages: messages.into(),
        };

        let msg_id = create_message_id_for_test(Role::Proposer); // Not committee role
        let ssv_msg_data = partial_sig_messages.as_ssz_bytes();
        let ssv_msg = SSVMessage::new(MsgType::SSVPartialSignatureMsgType, msg_id, ssv_msg_data)
            .expect("SSVMessage should be created");

        let signed_msg = SignedSSVMessage::new(
            vec![[0xAA; RSA_SIGNATURE_SIZE]],
            vec![OperatorId(1)],
            ssv_msg,
            vec![],
        )
        .expect("SignedSSVMessage should be created");

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);

        let validation_context =
            create_test_validation_context(&signed_msg, &committee_info, Role::Proposer, &map);

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::TooManyPartialSignatureMessages { .. }
                )
            },
            "TooManyPartialSignatureMessages",
        );
    }

    #[test]
    fn test_triple_validator_index_fails() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        // Create messages with a validator index that appears 3 times
        let messages = create_partial_signature_messages();

        let partial_sig_messages = PartialSignatureMessages {
            kind: PartialSignatureKind::PostConsensus,
            slot: Slot::new(0),
            messages: messages.into(),
        };

        let msg_id = create_message_id_for_test(Role::Committee);
        let ssv_msg_data = partial_sig_messages.as_ssz_bytes();
        let ssv_msg = SSVMessage::new(MsgType::SSVPartialSignatureMsgType, msg_id, ssv_msg_data)
            .expect("SSVMessage should be created");

        let signed_msg = SignedSSVMessage::new(
            vec![[0xAA; RSA_SIGNATURE_SIZE]],
            vec![OperatorId(1)],
            ssv_msg,
            vec![],
        )
        .expect("SignedSSVMessage should be created");

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);

        let validation_context =
            create_test_validation_context(&signed_msg, &committee_info, Role::Committee, &map);

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::TripleValidatorIndexInPartialSignatures
                )
            },
            "TripleValidatorIndexInPartialSignatures",
        );
    }
}
