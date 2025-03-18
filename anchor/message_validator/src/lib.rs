use database::NetworkStateService;
use gossipsub::MessageAcceptance;
use sha2::{Digest, Sha256};
use ssv_types::consensus::{QbftMessage, QbftMessageType};
use ssv_types::message::{MsgType, SSVMessage, SignedSSVMessage};
use ssv_types::msgid::{DutyExecutor, Role};
use ssv_types::partial_sig::PartialSignatureMessages;
use ssz::Decode;
use std::sync::Arc;
use tracing::{error, trace, warn};

// TODO taken from go-SSV as rough guidance. feel free to adjust as needed. https://github.com/ssvlabs/ssv/blob/e12abf7dfbbd068b99612fa2ebbe7e3372e57280/message/validation/errors.go#L55
#[derive(Debug)]
pub enum ValidationFailure {
    WrongDomain,
    NoShareMetadata,
    UnknownValidator,
    ValidatorLiquidated,
    ValidatorNotAttesting,
    EarlySlotMessage,
    LateSlotMessage,
    SlotAlreadyAdvanced,
    RoundAlreadyAdvanced,
    DecidedWithSameSigners,
    PubSubDataTooBig(usize),
    IncorrectTopic,
    NonExistentCommitteeID,
    RoundTooHigh,
    ValidatorIndexMismatch,
    TooManyDutiesPerEpoch,
    NoDuty,
    EstimatedRoundNotInAllowedSpread,
    EmptyData,
    MismatchedIdentifier { got: String, want: String },
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
    SignerNotLeader,
    SignersNotSorted,
    InconsistentSigners,
    InvalidHash,
    FullDataHash,
    UndecodableMessageData,
    EventMessage,
    UnknownSSVMessageType,
    UnknownQBFTMessageType,
    InvalidPartialSignatureType,
    PartialSignatureTypeRoleMismatch,
    NonDecidedWithMultipleSigners { got: usize, want: usize },
    DecidedNotEnoughSigners { got: usize, want: usize },
    DifferentProposalData,
    MalformedPrepareJustifications,
    UnexpectedPrepareJustifications,
    MalformedRoundChangeJustifications,
    UnexpectedRoundChangeJustifications,
    NoPartialSignatureMessages,
    NoValidators,
    NoSignatures,
    SignersAndSignaturesWithDifferentLength,
    PartialSigOneSigner,
    PrepareOrCommitWithFullData,
    FullDataNotInConsensusMessage,
    TripleValidatorIndexInPartialSignatures,
    ZeroRound,
    DuplicatedMessage,
    InvalidPartialSignatureTypeCount,
    TooManyPartialSignatureMessages,
    EncodeOperators,
    FailedToGetMaxRound,
}

impl From<&ValidationFailure> for MessageAcceptance {
    fn from(value: &ValidationFailure) -> Self {
        match value {
            ValidationFailure::WrongDomain
            | ValidationFailure::NoShareMetadata
            | ValidationFailure::UnknownValidator
            | ValidationFailure::ValidatorLiquidated
            | ValidationFailure::ValidatorNotAttesting
            | ValidationFailure::EarlySlotMessage
            | ValidationFailure::LateSlotMessage
            | ValidationFailure::SlotAlreadyAdvanced
            | ValidationFailure::RoundAlreadyAdvanced
            | ValidationFailure::DecidedWithSameSigners
            | ValidationFailure::PubSubDataTooBig(_)
            | ValidationFailure::IncorrectTopic
            | ValidationFailure::NonExistentCommitteeID
            | ValidationFailure::RoundTooHigh
            | ValidationFailure::ValidatorIndexMismatch
            | ValidationFailure::TooManyDutiesPerEpoch
            | ValidationFailure::NoDuty
            | ValidationFailure::EstimatedRoundNotInAllowedSpread => MessageAcceptance::Ignore,
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

pub struct Validator {
    network_state_service: Arc<dyn NetworkStateService>,
}

pub trait ValidatorService: Send + Sync {
    fn validate(&self, message_data: Vec<u8>) -> Result<ValidatedMessage, ValidationFailure>;
}

impl Validator {
    pub fn new(network_state_service: Arc<dyn NetworkStateService>) -> Self {
        Self {
            network_state_service,
        }
    }

    fn validate_ssv_message(
        &self,
        signed_ssv_message: &SignedSSVMessage,
    ) -> Result<ValidatedSSVMessage, ValidationFailure> {
        let ssv_message = signed_ssv_message.ssv_message();
        match ssv_message.msg_type() {
            MsgType::SSVConsensusMsgType => {
                let consensus_message = QbftMessage::from_ssz_bytes(ssv_message.data())
                    .ok()
                    .ok_or(ValidationFailure::UndecodableMessageData)?;
                self.validate_consensus_message_semantics(signed_ssv_message, &consensus_message)?;
                Ok(ValidatedSSVMessage::QbftMessage(consensus_message))
            }
            MsgType::SSVPartialSignatureMsgType => {
                self.validate_partial_signature_message(ssv_message)
            }
        }
    }

    fn validate_partial_signature_message(
        &self,
        ssv_message: &SSVMessage,
    ) -> Result<ValidatedSSVMessage, ValidationFailure> {
        let messages = match PartialSignatureMessages::from_ssz_bytes(ssv_message.data()) {
            Ok(msgs) => msgs,
            Err(_) => return Err(ValidationFailure::UndecodableMessageData),
        };

        Ok(ValidatedSSVMessage::PartialSignatureMessages(messages))
    }

    fn validate_consensus_message_semantics(
        &self,
        signed_ssv_message: &SignedSSVMessage,
        consensus_message: &QbftMessage,
    ) -> Result<(), ValidationFailure> {
        let signers = signed_ssv_message.operator_ids().len();

        let committee_id = match signed_ssv_message.ssv_message().msg_id().duty_executor() {
            Some(DutyExecutor::Committee(id)) => id,
            _ => return Err(ValidationFailure::NonExistentCommitteeID),
        };

        let committee_members = match self
            .network_state_service
            .get_cluster_members(&committee_id)
        {
            Some(committee_members) => {
                if committee_members.is_empty() {
                    warn!(?committee_id, "Unexpected empty committee members");
                    return Err(ValidationFailure::NonExistentCommitteeID);
                }
                committee_members
            }
            None => return Err(ValidationFailure::NonExistentCommitteeID),
        };

        let quorum_size = compute_quorum_size(committee_members.len());
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

            let hashed_full_data = hash_data_root(signed_ssv_message.full_data());
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
            .and_then(Role::max_round)
        {
            Some(max_round) => max_round,
            None => return Err(ValidationFailure::FailedToGetMaxRound),
        };

        if consensus_message.round > max_round {
            return Err(ValidationFailure::RoundTooHigh);
        }

        // Rule: consensus message must have the same identifier as the ssv message's identifier
        if *consensus_message.identifier != *signed_ssv_message.ssv_message().msg_id().as_ref() {
            return Err(ValidationFailure::MismatchedIdentifier {
                got: hex::encode(&*consensus_message.identifier),
                want: hex::encode(signed_ssv_message.ssv_message().msg_id()),
            });
        }

        self.validate_justifications(consensus_message)?;

        Ok(())
    }

    fn validate_justifications(
        &self,
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
}

impl ValidatorService for Validator {
    fn validate(&self, message_data: Vec<u8>) -> Result<ValidatedMessage, ValidationFailure> {
        match SignedSSVMessage::from_ssz_bytes(&message_data) {
            Ok(deserialized_message) => {
                trace!(msg = ?deserialized_message, "SignedSSVMessage deserialized");
                self.validate_ssv_message(&deserialized_message)
                    .map(|validated| ValidatedMessage::new(deserialized_message.clone(), validated))
            }
            Err(error) => {
                trace!("error" = ?error, "Failed to deserialize SignedSSVMessage");
                Err(ValidationFailure::UndecodableMessageData)
            }
        }
    }
}

fn compute_quorum_size(committee_size: usize) -> usize {
    let f = get_f(committee_size);
    f * 2 + 1
}

// # TODO centralize this and the one in the qbft crate
fn get_f(committee_size: usize) -> usize {
    (committee_size - 1) / 3
}

fn hash_data_root(full_data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(full_data);
    let hash: [u8; 32] = hasher.finalize().into();
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use bls::{Hash256, PublicKeyBytes};
    use ssv_types::consensus::{QbftMessage, QbftMessageType};
    use ssv_types::domain_type::DomainType;
    use ssv_types::message::{MsgType, SSVMessage, SignedSSVMessage, RSA_SIGNATURE_SIZE};
    use ssv_types::msgid::{DutyExecutor, MessageId, Role};
    use ssv_types::{CommitteeId, IndexSet, OperatorId};
    use ssz::Encode;
    use std::sync::Arc;

    // Constants for committee sizes in tests to improve readability
    const SINGLE_NODE_COMMITTEE: usize = 1;
    const FOUR_NODE_COMMITTEE: usize = 4;
    const SEVEN_NODE_COMMITTEE: usize = 7;

    struct MockNetworkStateService(usize);

    impl NetworkStateService for MockNetworkStateService {
        fn get_cluster_members(&self, _cluster_id: &CommitteeId) -> Option<IndexSet<OperatorId>> {
            let mut members = IndexSet::new();
            for i in 0..self.0 {
                members.insert(OperatorId(i as u64));
            }
            Some(members)
        }
    }

    // Test fixture for setup
    struct TestFixture {
        validator: Arc<Validator>,
    }

    impl TestFixture {
        fn new(committee_size: usize) -> Self {
            let validator = Arc::new(Validator::new(Arc::new(MockNetworkStateService(
                committee_size,
            ))));
            Self { validator }
        }

        // Helper for common validation pattern
        fn validate_message(
            &self,
            signed_msg: &SignedSSVMessage,
        ) -> Result<ValidatedSSVMessage, ValidationFailure> {
            self.validator.validate_ssv_message(signed_msg)
        }
    }

    // Helper functions for message creation
    struct MessageBuilder {
        msg_id: MessageId,
        msg_type: QbftMessageType,
        round: u64,
        signers: Vec<OperatorId>,
        signatures: Vec<Vec<u8>>,
        full_data: Vec<u8>,
        prepare_justification: Vec<SignedSSVMessage>,
        round_change_justification: Vec<SignedSSVMessage>,
    }

    impl MessageBuilder {
        fn new(role: Role, msg_type: QbftMessageType) -> Self {
            Self {
                msg_id: create_message_id_for_test(role),
                msg_type,
                round: 1,
                signers: vec![OperatorId(42)],
                signatures: vec![vec![0xAA; RSA_SIGNATURE_SIZE]],
                full_data: vec![],
                prepare_justification: vec![],
                round_change_justification: vec![],
            }
        }

        fn with_round(mut self, round: u64) -> Self {
            self.round = round;
            self
        }

        fn with_signers(mut self, signers: Vec<OperatorId>) -> Self {
            // Create matching number of signatures
            self.signatures = signers
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    // Create unique signatures for each signer
                    vec![0xAA + i as u8; RSA_SIGNATURE_SIZE]
                })
                .collect();
            self.signers = signers;
            self
        }

        fn with_full_data(mut self, data: Vec<u8>) -> Self {
            self.full_data = data;
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

        fn build(self) -> SignedSSVMessage {
            let qbft_msg = QbftMessage {
                qbft_message_type: self.msg_type,
                height: 1,
                round: self.round,
                identifier: self.msg_id.clone().into(),
                root: Hash256::from([0u8; 32]),
                data_round: 1,
                round_change_justification: self.round_change_justification,
                prepare_justification: self.prepare_justification,
            };

            let qbft_bytes = qbft_msg.as_ssz_bytes();
            let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, self.msg_id, qbft_bytes)
                .expect("SSVMessage should be created");

            SignedSSVMessage::new(self.signatures, self.signers, ssv_msg, self.full_data)
                .expect("SignedSSVMessage should be created")
        }
    }

    fn create_message_id_for_test(role: Role) -> MessageId {
        let domain = DomainType([0, 0, 0, 1]);
        let duty_executor = match role {
            Role::Committee => DutyExecutor::Committee(CommitteeId([0u8; 32])),
            _ => DutyExecutor::Validator(PublicKeyBytes::empty()),
        };
        MessageId::new(&domain, role, &duty_executor)
    }

    fn dummy_signed_ssv_message_for_justification() -> SignedSSVMessage {
        MessageBuilder::new(Role::Proposer, QbftMessageType::Proposal).build()
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
    // Consensus message tests
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn test_successful_validation_of_consensus_message_with_single_signer() {
        let fixture = TestFixture::new(SINGLE_NODE_COMMITTEE);

        let signed_msg = MessageBuilder::new(Role::Committee, QbftMessageType::Prepare).build();

        let result = fixture.validate_message(&signed_msg);
        assert!(
            result.is_ok(),
            "Expected a single-signer Prepare consensus message to validate successfully"
        );

        if let Ok(ValidatedSSVMessage::QbftMessage(validated_qbft)) = result {
            assert_eq!(
                validated_qbft.round, 1,
                "Unexpected round in validated QbftMessage"
            );
            assert_eq!(
                validated_qbft.qbft_message_type,
                QbftMessageType::Prepare,
                "Unexpected QbftMessageType in validated QbftMessage"
            );
            assert_eq!(
                &*validated_qbft.identifier,
                create_message_id_for_test(Role::Committee).as_ref(),
                "Identifier mismatch after validation"
            );
        } else {
            panic!("Expected a QbftMessage variant after validation");
        }
    }

    #[tokio::test]
    async fn test_consensus_message_with_multiple_signers_but_not_commit() {
        let fixture = TestFixture::new(SINGLE_NODE_COMMITTEE);

        // Multiple signers are only allowed for Commit messages.
        let signers = vec![OperatorId(1), OperatorId(2), OperatorId(3)];
        let signed_msg = MessageBuilder::new(Role::Committee, QbftMessageType::Prepare)
            .with_signers(signers.clone())
            .build();

        let result = fixture.validate_message(&signed_msg);

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::NonDecidedWithMultipleSigners { got, want } if *got == signers.len() && *want == SINGLE_NODE_COMMITTEE),
            "NonDecidedWithMultipleSigners",
        );
    }

    #[tokio::test]
    async fn test_consensus_message_with_multiple_signers_commit_but_not_enough_signers_for_quorum()
    {
        let fixture = TestFixture::new(FOUR_NODE_COMMITTEE);

        // For Commit messages with multiple signers, the count must be >= quorum size.
        let signers = vec![OperatorId(1), OperatorId(2)]; // Quorum requires at least 3 for a committee of 4.
        let signed_msg = MessageBuilder::new(Role::Committee, QbftMessageType::Commit)
            .with_signers(signers.clone())
            .build();

        let result = fixture.validate_message(&signed_msg);

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::DecidedNotEnoughSigners { got, want } if *got == signers.len() && *want == FOUR_NODE_COMMITTEE - 1),
            "DecidedNotEnoughSigners",
        );
    }

    #[tokio::test]
    async fn test_consensus_message_full_data_mismatched_root_hash() {
        let fixture = TestFixture::new(SINGLE_NODE_COMMITTEE);

        let full_data = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let signed_msg = MessageBuilder::new(Role::Committee, QbftMessageType::Commit)
            .with_full_data(full_data)
            .build();

        let result = fixture.validate_message(&signed_msg);

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::PrepareOrCommitWithFullData),
            "PrepareOrCommitWithFullData",
        );
    }

    #[tokio::test]
    async fn test_consensus_message_zero_round_fails() {
        let fixture = TestFixture::new(SINGLE_NODE_COMMITTEE);

        let signed_msg = MessageBuilder::new(Role::Committee, QbftMessageType::Proposal)
            .with_round(0)
            .build();

        let result = fixture.validate_message(&signed_msg);

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::ZeroRound),
            "ZeroRound",
        );
    }

    #[tokio::test]
    async fn test_consensus_message_round_too_high() {
        let fixture = TestFixture::new(SINGLE_NODE_COMMITTEE);

        let signed_msg = MessageBuilder::new(Role::Committee, QbftMessageType::Proposal)
            .with_round(13) // Too high (max is 12)
            .build();

        let result = fixture.validate_message(&signed_msg);

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::RoundTooHigh),
            "RoundTooHigh",
        );
    }

    #[tokio::test]
    async fn test_consensus_message_mismatched_identifier() {
        let fixture = TestFixture::new(SINGLE_NODE_COMMITTEE);

        // Create message with mismatched identifier
        let msg_id_a = create_message_id_for_test(Role::Committee);
        let msg_id_b = create_message_id_for_test(Role::Proposer);

        let qbft_msg = QbftMessage {
            qbft_message_type: QbftMessageType::Proposal,
            height: 1,
            round: 1,
            identifier: msg_id_b.into(), // Mismatched ID
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

        let result = fixture.validate_message(&signed_msg);

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

    #[tokio::test]
    async fn test_consensus_message_decode_failure() {
        let fixture = TestFixture::new(SINGLE_NODE_COMMITTEE);

        // Provide invalid consensus data
        let msg_id = create_message_id_for_test(Role::Proposer);
        let invalid_data = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, msg_id, invalid_data)
            .expect("SSVMessage should be created");
        let signed_msg = SignedSSVMessage::new(
            vec![vec![0xAA; RSA_SIGNATURE_SIZE]],
            vec![OperatorId(42)],
            ssv_msg,
            vec![],
        )
        .expect("SignedSSVMessage should be created");

        let result = fixture.validate_message(&signed_msg);

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::UndecodableMessageData),
            "UndecodableMessageData",
        );
    }

    #[tokio::test]
    async fn test_consensus_message_multiple_signers_commit_with_full_data_and_invalid_hash() {
        let fixture = TestFixture::new(FOUR_NODE_COMMITTEE);
        let signers = vec![OperatorId(1), OperatorId(2), OperatorId(3)];
        let full_data = vec![0xFF; 16];
        let signed_msg = MessageBuilder::new(Role::Committee, QbftMessageType::Commit)
            .with_signers(signers.clone())
            .with_full_data(full_data)
            .build();
        let result = fixture.validate_message(&signed_msg);
        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::InvalidHash),
            "InvalidHash",
        );
    }

    #[tokio::test]
    async fn test_prepare_justifications_with_non_proposal_message() {
        let fixture = TestFixture::new(SINGLE_NODE_COMMITTEE);

        let signed_msg = MessageBuilder::new(Role::Committee, QbftMessageType::Prepare)
            .with_prepare_justification(vec![dummy_signed_ssv_message_for_justification()])
            .build();

        let result = fixture.validate_message(&signed_msg);

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::UnexpectedPrepareJustifications),
            "UnexpectedPrepareJustifications",
        );
    }

    #[tokio::test]
    async fn test_round_change_justifications_with_non_proposal_or_round_change() {
        let fixture = TestFixture::new(SINGLE_NODE_COMMITTEE);

        let signed_msg = MessageBuilder::new(Role::Committee, QbftMessageType::Commit)
            .with_round_change_justification(vec![dummy_signed_ssv_message_for_justification()])
            .build();

        let result = fixture.validate_message(&signed_msg);

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

    #[tokio::test]
    async fn test_compute_quorum_size() {
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
}
