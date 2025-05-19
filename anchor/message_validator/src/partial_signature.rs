use slot_clock::SlotClock;
use ssv_types::{
    msgid::Role,
    partial_sig::{PartialSignatureKind, PartialSignatureMessages},
};
use ssz::Decode;

use crate::{ValidatedSSVMessage, ValidationContext, ValidationFailure, verify_message_signature};

pub(crate) fn validate_partial_signature_message(
    validation_context: ValidationContext<impl SlotClock>,
) -> Result<ValidatedSSVMessage, ValidationFailure> {
    // Decode message directly to PartialSignatureMessages
    let messages = match PartialSignatureMessages::from_ssz_bytes(
        validation_context.signed_ssv_message.ssv_message().data(),
    ) {
        Ok(msgs) => msgs,
        Err(_) => return Err(ValidationFailure::UndecodableMessageData),
    };

    // Validate basic semantics
    validate_partial_signature_message_semantics(&validation_context, &messages)?;

    // we still need to validate by duty logic

    let operator_pk = validation_context
        .operators_pk
        .first()
        .ok_or(ValidationFailure::NoSigners)?;

    let signature = validation_context
        .signed_ssv_message
        .signatures()
        .first()
        .ok_or(ValidationFailure::NoSignatures)?;

    verify_message_signature(
        validation_context.signed_ssv_message,
        operator_pk,
        signature,
    )?;

    Ok(ValidatedSSVMessage::PartialSignatureMessages(messages))
}

fn validate_partial_signature_message_semantics(
    validation_context: &ValidationContext<impl SlotClock>,
    partial_signature_messages: &PartialSignatureMessages,
) -> Result<(), ValidationFailure> {
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

    Ok(())
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
        OperatorId, ValidatorIndex,
        message::{MsgType, RSA_SIGNATURE_SIZE, SSVMessage, SignedSSVMessage},
        partial_sig::PartialSignatureMessage,
    };
    use ssz::Encode;
    use types::Slot;

    use super::*;
    use crate::tests::{
        FOUR_NODE_COMMITTEE, assert_validation_error, create_committee_info,
        create_message_id_for_test, generate_random_rsa_public_keys,
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
            slot: Slot::new(1),
            messages,
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
            vec![signer.sign_to_vec().expect("Failed to sign message")]
        } else {
            vec![vec![0xAA; RSA_SIGNATURE_SIZE]]
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
        operators_pk: &'a [Rsa<Public>],
    ) -> ValidationContext<'a, ManualSlotClock> {
        ValidationContext {
            signed_ssv_message: signed_msg,
            committee_info,
            role,
            received_at: SystemTime::now(),
            operators_pk,
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            slot_clock: ManualSlotClock::new(
                Slot::new(0),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap(),
                Duration::from_secs(1),
            ),
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
        let validation_context =
            create_test_validation_context(&signed_msg, &committee_info, Role::Committee, &binding);

        let result = validate_partial_signature_message(validation_context);

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
        let signatures = vec![
            vec![0xAA; RSA_SIGNATURE_SIZE],
            vec![0xBB; RSA_SIGNATURE_SIZE],
        ];

        let signed_msg = SignedSSVMessage::new(signatures, signers, ssv_msg, vec![])
            .expect("SignedSSVMessage should be created");

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let validation_context =
            create_test_validation_context(&signed_msg, &committee_info, Role::Proposer, &binding);

        let result = validate_partial_signature_message(validation_context);

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
        let validation_context =
            create_test_validation_context(&signed_msg, &committee_info, Role::Proposer, &binding);

        let result = validate_partial_signature_message(validation_context);

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
        let validation_context =
            create_test_validation_context(&signed_msg, &committee_info, Role::Proposer, &binding);

        let result = validate_partial_signature_message(validation_context);

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
        let validation_context =
            create_test_validation_context(&signed_msg, &committee_info, Role::Proposer, &binding);

        let result = validate_partial_signature_message(validation_context);

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
        let validation_context =
            create_test_validation_context(&signed_msg, &committee_info, Role::Proposer, &binding);

        let result = validate_partial_signature_message(validation_context);

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
        let validation_context = create_test_validation_context(
            &signed_msg,
            &committee_info,
            Role::Proposer, // Not a committee role, so validator index is checked
            &binding,
        );

        let result = validate_partial_signature_message(validation_context);

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
        let validation_context = create_test_validation_context(
            &signed_msg,
            &committee_info,
            Role::Committee, // Committee role, so validator index is not checked
            &binding,
        );

        let result = validate_partial_signature_message(validation_context);

        assert!(
            result.is_ok(),
            "{}",
            format!("Expected successful validation for Committee role, but got: {result:?}")
        );
    }
}
