use crate::{compute_quorum_size, hash_data, ValidationFailure};
use ssv_types::consensus::{QbftMessage, QbftMessageType};
use ssv_types::message::SignedSSVMessage;
use ssv_types::msgid::Role;
use ssv_types::CommitteeInfo;
use ssv_types::VariableList;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{create_committee_info, FOUR_NODE_COMMITTEE, SINGLE_NODE_COMMITTEE};
    use crate::{validate_ssv_message, ValidatedSSVMessage};
    use bls::{Hash256, PublicKeyBytes};
    use ssv_types::consensus::{QbftMessage, QbftMessageType};
    use ssv_types::domain_type::DomainType;
    use ssv_types::message::{MsgType, SSVMessage, SignedSSVMessage, RSA_SIGNATURE_SIZE};
    use ssv_types::msgid::{DutyExecutor, MessageId, Role};
    use ssv_types::{CommitteeId, OperatorId};
    use ssz::Encode;

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

    // Helper for creating SignedSSVMessage with a QbftMessage
    fn create_signed_consensus_message(
        qbft_message: QbftMessage,
        signers: Vec<OperatorId>,
        full_data: Vec<u8>,
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
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, msg_id.into(), qbft_bytes)
            .expect("SSVMessage should be created");

        let signatures = signers
            .iter()
            .enumerate()
            .map(|(i, _)| vec![0xAA + i as u8; RSA_SIGNATURE_SIZE])
            .collect::<Vec<_>>();

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

    // ---------------------------------------------------------------------
    // validate_ssv_message tests
    // ---------------------------------------------------------------------

    #[test]
    fn test_validate_ssv_message_consensus_success() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        let qbft_message =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Proposal).build();
        let signed_msg = create_signed_consensus_message(qbft_message, vec![OperatorId(1)], vec![]);

        let result = validate_ssv_message(&signed_msg, &committee_info, Role::Committee);
        assert!(result.is_ok(), "Expected successful validation");

        match result.unwrap() {
            ValidatedSSVMessage::QbftMessage(_) => {} // success
            _ => panic!("Expected QbftMessage variant"),
        }
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

        let result = validate_ssv_message(&signed_msg, &committee_info, Role::Committee);

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
        let signed_msg =
            create_signed_consensus_message(qbft_message.clone(), vec![OperatorId(1)], vec![]);

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
            create_signed_consensus_message(qbft_message.clone(), signers.clone(), vec![]);

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
            create_signed_consensus_message(qbft_message.clone(), signers.clone(), vec![]);

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
        let signed_msg =
            create_signed_consensus_message(qbft_message.clone(), vec![OperatorId(1)], full_data);

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
        let signed_msg =
            create_signed_consensus_message(qbft_message.clone(), vec![OperatorId(1)], vec![]);

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
        let signed_msg =
            create_signed_consensus_message(qbft_message.clone(), vec![OperatorId(1)], vec![]);

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
            create_signed_consensus_message(dummy_qbft, vec![OperatorId(1)], vec![])
        };

        let qbft_message = QbftMessageBuilder::new(Role::Committee, QbftMessageType::Prepare)
            .with_prepare_justification(vec![dummy_justification])
            .build();
        let signed_msg =
            create_signed_consensus_message(qbft_message.clone(), vec![OperatorId(1)], vec![]);

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
            create_signed_consensus_message(dummy_qbft, vec![OperatorId(1)], vec![])
        };

        let qbft_message = QbftMessageBuilder::new(Role::Committee, QbftMessageType::Commit)
            .with_round_change_justification(vec![dummy_justification])
            .build();
        let signed_msg =
            create_signed_consensus_message(qbft_message.clone(), vec![OperatorId(1)], vec![]);

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
        let signed_msg =
            create_signed_consensus_message(qbft_message.clone(), signers.clone(), full_data);

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

        let signed_msg = create_signed_consensus_message(qbft_message.clone(), signers, full_data);

        let result =
            validate_consensus_message_semantics(&signed_msg, &qbft_message, &committee_info);

        assert!(
            result.is_ok(),
            "Expected successful validation with correct hash"
        );
    }
}
