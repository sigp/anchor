use database::NetworkStateService;
use libp2p::gossipsub::MessageAcceptance::{Accept, Reject};
use libp2p::gossipsub::{MessageAcceptance, MessageId};
use libp2p::PeerId;
use processor::Senders;
use sha2::{Digest, Sha256};
use ssv_types::consensus::{QbftMessage, QbftMessageType};
use ssv_types::message::{MsgType, SSVMessage, SignedSSVMessage};
use ssv_types::msgid::DutyExecutor;
use ssv_types::partial_sig::{
    PartialSignatureKind, PartialSignatureMessage, PartialSignatureMessages,
};
use ssz::Decode;
use std::sync::Arc;
use tokio::sync::mpsc::error::TrySendError::{Closed, Full};
use tokio::sync::mpsc::Sender;
use tracing::{error, trace, warn};
use types::Slot;

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

pub enum ValidatedSSVMessage {
    QbftMessage(QbftMessage),
    PartialSignatureMessages(PartialSignatureMessages),
}

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

pub struct Outcome {
    pub message_id: MessageId,
    pub propagation_source: PeerId,
    pub message: Option<ValidatedMessage>,
    pub action: MessageAcceptance,
}

impl Outcome {
    pub fn new(
        message_id: MessageId,
        propagation_success: PeerId,
        message: Option<ValidatedMessage>,
        action: MessageAcceptance,
    ) -> Self {
        Self {
            message_id,
            propagation_source: propagation_success,
            message,
            action,
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Processor error: {0}")]
    Processor(#[from] ::processor::Error),
}

pub struct Validator {
    processor: Senders,
    result_tx: Sender<Outcome>,
    network_state_service: Arc<dyn NetworkStateService>,
}

pub trait ValidatorService {
    fn send_for_validation(
        self: Arc<Self>,
        message_id: MessageId,
        propagation_source: PeerId,
        message_data: Vec<u8>,
    ) -> Result<(), Error>;
}

impl Validator {
    pub fn new(
        processor: Senders,
        result_tx: Sender<Outcome>,
        network_state_service: Arc<dyn NetworkStateService>,
    ) -> Self {
        Self {
            processor,
            result_tx,
            network_state_service,
        }
    }

    fn do_validate(&self, _message: &SignedSSVMessage) -> Result<(), ValidationFailure> {
        Ok(())
    }

    fn validate_ssv_message(
        &self,
        signed_ssv_message: &SignedSSVMessage,
        ssv_message: &SSVMessage,
    ) -> Result<ValidatedSSVMessage, ValidationFailure> {
        match ssv_message.msg_type() {
            MsgType::SSVConsensusMsgType => {
                let consensus_message = QbftMessage::from_ssz_bytes(ssv_message.data())
                    .ok()
                    .ok_or(ValidationFailure::UndecodableMessageData)?;
                self.validate_consensus_message_semantics(signed_ssv_message, &consensus_message)?;
                Ok(ValidatedSSVMessage::QbftMessage(consensus_message))
            }
            MsgType::SSVPartialSignatureMsgType => {
                PartialSignatureMessage::from_ssz_bytes(ssv_message.data())
                    .ok()
                    .map(|m| {
                        let p = PartialSignatureMessages {
                            kind: PartialSignatureKind::RandaoPartialSig,
                            slot: Slot::new(1),
                            messages: vec![m],
                        };
                        ValidatedSSVMessage::PartialSignatureMessages(p)
                    })
                    .ok_or(ValidationFailure::UndecodableMessageData)
            }
        }
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

        let màx_round = match consensus_message.max_round() {
            Some(max_round) => max_round,
            None => return Err(ValidationFailure::FailedToGetMaxRound),
        };

        if consensus_message.round > màx_round {
            return Err(ValidationFailure::RoundTooHigh);
        }

        // Rule: consensus message must have the same identifier as the ssv message's identifier
        if consensus_message.identifier != *signed_ssv_message.ssv_message().msg_id() {
            return Err(ValidationFailure::MismatchedIdentifier {
                got: hex::encode(&consensus_message.identifier),
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
    fn send_for_validation(
        self: Arc<Self>,
        message_id: MessageId,
        propagation_source: PeerId,
        message_data: Vec<u8>,
    ) -> Result<(), Error> {
        let validator = self.clone();
        Ok(self.processor.urgent_consensus.send_blocking(
            move || {
                let (outcome, validated_message) =
                    match SignedSSVMessage::from_ssz_bytes(&message_data) {
                        Ok(deserialized_message) => {
                            trace!(msg = ?deserialized_message, "SignedSSVMessage deserialized");
                            match validator.do_validate(&deserialized_message) {
                                Ok(()) => {
                                    match validator.validate_ssv_message(
                                        &deserialized_message,
                                        deserialized_message.ssv_message(),
                                    ) {
                                        Ok(validated_ssv_message) => (
                                            Accept,
                                            Some(ValidatedMessage::new(
                                                deserialized_message.clone(),
                                                validated_ssv_message,
                                            )),
                                        ),
                                        Err(failure) => {
                                            trace!(
                                                ?failure,
                                                ?message_id,
                                                ?propagation_source,
                                                "Validation failure"
                                            );
                                            ((&failure).into(), None)
                                        }
                                    }
                                }
                                Err(failure) => {
                                    trace!(
                                        ?failure,
                                        ?message_id,
                                        ?propagation_source,
                                        "Validation failure"
                                    );
                                    ((&failure).into(), None)
                                }
                            }
                        }
                        Err(error) => {
                            trace!("error" = ?error, "Failed to deserialize SignedSSVMessage");
                            (Reject, None)
                        }
                    };
                match validator.result_tx.try_send(Outcome::new(
                    message_id,
                    propagation_source,
                    validated_message,
                    outcome,
                )) {
                    Ok(()) => (),
                    Err(Closed(_)) => {
                        error!("Validation result receiver dropped");
                    }
                    Err(Full(_)) => {
                        error!("Validation result receiver full");
                        // metrics::inc_counter_vec(
                        //     &metrics::VALIDATOR_RESULT_TIMEOUTS,
                        //     &["validator_service"],
                        // );
                    }
                }
            },
            "validator",
        )?)
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
    use once_cell::sync::Lazy;
    use ssv_types::{CommitteeId, IndexSet, OperatorId};
    use std::sync::Arc;
    use task_executor::TaskExecutor;
    use tokio::sync::mpsc;
    // Import real types from your modules.
    use ssv_types::consensus::{QbftMessage, QbftMessageType};
    use ssv_types::domain_type::DomainType;
    use ssv_types::message::{MsgType, SSVMessage, SignedSSVMessage, RSA_SIGNATURE_SIZE};
    use ssv_types::msgid::{DutyExecutor, MessageId, Role};

    // Create a global task executor once for all tests.
    static GLOBAL_EXECUTOR: Lazy<TaskExecutor> = Lazy::new(|| {
        let handle = tokio::runtime::Handle::current();
        let (_signal, exit) = async_channel::bounded(1);
        let (shutdown, _) = libp2p::futures::channel::mpsc::channel(1);
        TaskExecutor::new(handle, exit, shutdown)
    });

    // Create a global processor once for all tests.
    static GLOBAL_PROCESSOR: Lazy<Senders> = Lazy::new(|| {
        let config = processor::Config::default();
        processor::spawn(config, GLOBAL_EXECUTOR.clone())
    });

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

    /// Real processor setup using the provided executor.
    fn build_validator_and_outcome_channel(
        _executor: TaskExecutor,
        num_operators: usize,
    ) -> (Arc<Validator>, mpsc::Sender<Outcome>) {
        let (outcome_tx, _outcome_rx) = mpsc::channel(10);
        let validator = Arc::new(Validator::new(
            GLOBAL_PROCESSOR.clone(),
            outcome_tx.clone(),
            Arc::new(MockNetworkStateService(num_operators)),
        ));
        (validator, outcome_tx)
    }

    /// Helper: Create a valid MessageId for testing.
    fn create_message_id_for_test(role: Role) -> MessageId {
        let domain = DomainType([0, 0, 0, 1]);
        let duty_executor = match role {
            Role::Committee => DutyExecutor::Committee(CommitteeId([0u8; 32])),
            _ => DutyExecutor::Validator(PublicKeyBytes::empty()),
        };
        MessageId::new(&domain, role, &duty_executor)
    }

    /// Helper functions for creating SSV messages.
    mod test_utils {
        use super::*;
        use ssz::Encode;
        /// Create a consensus SSVMessage from a given QbftMessage and message identifier.
        pub fn create_consensus_ssv_message(
            qbft_msg: QbftMessage,
            msg_id: MessageId,
        ) -> SSVMessage {
            let qbft_bytes = qbft_msg.as_ssz_bytes();
            // The constructor now expects a MessageId (not a Vec<u8>).
            SSVMessage::new(MsgType::SSVConsensusMsgType, msg_id, qbft_bytes)
                .expect("SSVMessage should be created")
        }
    }

    /// Convenience function to build a SignedSSVMessage.
    fn create_signed_ssv_message(
        signatures: Vec<Vec<u8>>,
        operator_ids: Vec<OperatorId>,
        ssv_message: SSVMessage,
        full_data: Vec<u8>,
    ) -> SignedSSVMessage {
        SignedSSVMessage::new(signatures, operator_ids, ssv_message, full_data)
            .expect("SignedSSVMessage should be created")
    }

    /// Helper: Create a dummy SignedSSVMessage for justifications.
    fn dummy_signed_ssv_message_for_justification() -> SignedSSVMessage {
        let msg_id = create_message_id_for_test(Role::Proposer);
        // Create a dummy consensus message; its content isn’t used.

        let dummy_qbft = QbftMessage {
            qbft_message_type: QbftMessageType::Proposal,
            height: 1,
            round: 1,
            identifier: msg_id.clone(),
            root: Hash256::from([0u8; 32]),
            data_round: 1,
            round_change_justification: vec![],
            prepare_justification: vec![],
        };
        let dummy_ssv = test_utils::create_consensus_ssv_message(dummy_qbft, msg_id);
        create_signed_ssv_message(
            vec![vec![0xAA; RSA_SIGNATURE_SIZE]],
            vec![OperatorId(42)],
            dummy_ssv,
            vec![],
        )
    }

    /// Convenience: Quick SHA256 hash.
    fn quick_hash(data: &[u8]) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher.finalize().into()
    }

    // ---------------------------------------------------------------------
    // Consensus message tests
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn test_successful_validation_of_consensus_message_with_single_signer() {
        let (validator, _outcome_tx) =
            build_validator_and_outcome_channel(GLOBAL_EXECUTOR.clone(), 1);

        let msg_id = create_message_id_for_test(Role::Committee);
        let round = 1;
        let qbft_msg = QbftMessage {
            qbft_message_type: QbftMessageType::Prepare,
            height: 1,
            round,
            identifier: msg_id.clone(),
            root: Hash256::from([0u8; 32]),
            data_round: 1,
            round_change_justification: vec![],
            prepare_justification: vec![],
        };

        let ssv_msg = test_utils::create_consensus_ssv_message(qbft_msg, msg_id.clone());
        let signed_msg = create_signed_ssv_message(
            vec![vec![0xAA; RSA_SIGNATURE_SIZE]],
            vec![OperatorId(42)],
            ssv_msg,
            vec![],
        );

        let result = validator.validate_ssv_message(&signed_msg, signed_msg.ssv_message());
        assert!(
            result.is_ok(),
            "Expected a single-signer Prepare consensus message to validate successfully"
        );
        if let Ok(ValidatedSSVMessage::QbftMessage(validated_qbft)) = result {
            assert_eq!(
                validated_qbft.round, round,
                "Unexpected round in validated QbftMessage"
            );
            assert_eq!(
                validated_qbft.qbft_message_type,
                QbftMessageType::Prepare,
                "Unexpected QbftMessageType in validated QbftMessage"
            );
            assert_eq!(
                validated_qbft.identifier, msg_id,
                "Identifier mismatch after validation"
            );
        } else {
            panic!("Expected a QbftMessage variant after validation");
        }
    }

    #[tokio::test]
    async fn test_consensus_message_with_multiple_signers_but_not_commit() {
        let (validator, _outcome_tx) =
            build_validator_and_outcome_channel(GLOBAL_EXECUTOR.clone(), 1);

        // Multiple signers are only allowed for Commit messages.
        let signers = vec![OperatorId(1), OperatorId(2), OperatorId(3)];
        let msg_id = create_message_id_for_test(Role::Committee);
        let qbft_msg = QbftMessage {
            qbft_message_type: QbftMessageType::Prepare, // Non-Commit type.
            height: 1,
            round: 1,
            identifier: msg_id.clone(),
            root: Hash256::from([0u8; 32]),
            data_round: 1,
            round_change_justification: vec![],
            prepare_justification: vec![],
        };

        let ssv_msg = test_utils::create_consensus_ssv_message(qbft_msg, msg_id);
        let signed_msg = create_signed_ssv_message(
            vec![
                vec![0xAA; RSA_SIGNATURE_SIZE],
                vec![0xBB; RSA_SIGNATURE_SIZE],
                vec![0xCC; RSA_SIGNATURE_SIZE],
            ],
            signers.clone(),
            ssv_msg,
            vec![],
        );

        let result = validator.validate_ssv_message(&signed_msg, signed_msg.ssv_message());
        assert!(
            result.is_err(),
            "Expected multiple signers with non-Commit type to fail validation"
        );
        match result.err().unwrap() {
            ValidationFailure::NonDecidedWithMultipleSigners { got, want } => {
                assert_eq!(got, signers.len(), "Unexpected number of signers in error");
                assert_eq!(want, 1, "Expected only one signer for non-Commit messages");
            }
            other => panic!(
                "Expected NonDecidedWithMultipleSigners error, got: {:?}",
                other
            ),
        }
    }

    #[tokio::test]
    async fn test_consensus_message_with_multiple_signers_commit_but_not_enough_signers_for_quorum()
    {
        let (validator, _outcome_tx) =
            build_validator_and_outcome_channel(GLOBAL_EXECUTOR.clone(), 4);

        // For Commit messages with multiple signers, the count must be >= quorum size.
        let signers = vec![OperatorId(1), OperatorId(2)]; // Assume quorum requires at least 3.
        let msg_id = create_message_id_for_test(Role::Committee);
        let qbft_msg = QbftMessage {
            qbft_message_type: QbftMessageType::Commit,
            height: 1,
            round: 1,
            identifier: msg_id.clone(),
            root: Hash256::from([0u8; 32]),
            data_round: 1,
            round_change_justification: vec![],
            prepare_justification: vec![],
        };

        let ssv_msg = test_utils::create_consensus_ssv_message(qbft_msg, msg_id);
        let signed_msg = create_signed_ssv_message(
            vec![
                vec![0xAA; RSA_SIGNATURE_SIZE],
                vec![0xBB; RSA_SIGNATURE_SIZE],
            ],
            signers.clone(),
            ssv_msg,
            vec![],
        );

        let result = validator.validate_ssv_message(&signed_msg, signed_msg.ssv_message());
        assert!(
            result.is_err(),
            "Expected Commit message with insufficient signers to fail validation"
        );
        match result.err().unwrap() {
            ValidationFailure::DecidedNotEnoughSigners { got, want } => {
                assert_eq!(got, signers.len(), "Mismatch in signer count reported");
                assert!(got < want, "Got should be less than required quorum");
            }
            other => panic!("Expected DecidedNotEnoughSigners error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_consensus_message_full_data_mismatched_root_hash() {
        let (validator, _outcome_tx) =
            build_validator_and_outcome_channel(GLOBAL_EXECUTOR.clone(), 1);

        // For a Commit message with full_data (single-signer) the full data hash must match.
        let signers = vec![OperatorId(42)];
        let full_data = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let msg_id = create_message_id_for_test(Role::Committee);
        let qbft_msg = QbftMessage {
            qbft_message_type: QbftMessageType::Commit,
            height: 1,
            round: 1,
            identifier: msg_id.clone(),
            // Set root to the hash of an empty slice (mismatched)
            root: Hash256::from([0u8; 32]),
            data_round: 1,
            round_change_justification: vec![],
            prepare_justification: vec![],
        };
        let ssv_msg = test_utils::create_consensus_ssv_message(qbft_msg, msg_id);
        let signed_msg = create_signed_ssv_message(
            vec![vec![0xAA; RSA_SIGNATURE_SIZE]],
            signers.clone(),
            ssv_msg,
            full_data,
        );

        let result = validator.validate_ssv_message(&signed_msg, signed_msg.ssv_message());
        assert!(
            result.is_err(),
            "Expected validation failure due to full data hash mismatch"
        );
        match result.err().unwrap() {
            ValidationFailure::PrepareOrCommitWithFullData => { /* Expected */ }
            other => panic!(
                "Expected PrepareOrCommitWithFullData error, got: {:?}",
                other
            ),
        }
    }

    #[tokio::test]
    async fn test_consensus_message_zero_round_fails() {
        let (validator, _outcome_tx) =
            build_validator_and_outcome_channel(GLOBAL_EXECUTOR.clone(), 1);

        let signers = vec![OperatorId(42)];
        let msg_id = create_message_id_for_test(Role::Committee);
        let qbft_msg = QbftMessage {
            qbft_message_type: QbftMessageType::Proposal,
            height: 1,
            round: 0, // Invalid round.
            identifier: msg_id.clone(),
            root: Hash256::from([0u8; 32]),
            data_round: 1,
            round_change_justification: vec![],
            prepare_justification: vec![],
        };
        let ssv_msg = test_utils::create_consensus_ssv_message(qbft_msg, msg_id);
        let signed_msg = create_signed_ssv_message(
            vec![vec![0xAA; RSA_SIGNATURE_SIZE]],
            signers,
            ssv_msg,
            vec![],
        );

        let result = validator.validate_ssv_message(&signed_msg, signed_msg.ssv_message());
        assert!(result.is_err(), "Expected round=0 to fail validation");
        match result.err().unwrap() {
            ValidationFailure::ZeroRound => (),
            other => panic!("Expected ZeroRound error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_consensus_message_round_too_high() {
        let (validator, _outcome_tx) =
            build_validator_and_outcome_channel(GLOBAL_EXECUTOR.clone(), 1);

        // For a proposer, max_round is Some(6). Set round = 7 to trigger an error.
        let signers = vec![OperatorId(42)];
        let msg_id = create_message_id_for_test(Role::Committee);
        let qbft_msg = QbftMessage {
            qbft_message_type: QbftMessageType::Proposal,
            height: 1,
            round: 13, // Invalid round.
            identifier: msg_id.clone(),
            root: Hash256::from([0u8; 32]),
            data_round: 1,
            round_change_justification: vec![],
            prepare_justification: vec![],
        };
        let ssv_msg = test_utils::create_consensus_ssv_message(qbft_msg, msg_id);
        let signed_msg = create_signed_ssv_message(
            vec![vec![0xAA; RSA_SIGNATURE_SIZE]],
            signers,
            ssv_msg,
            vec![],
        );

        let result = validator.validate_ssv_message(&signed_msg, signed_msg.ssv_message());
        assert!(
            result.is_err(),
            "Expected round > max_round to fail validation"
        );
        match result.err().unwrap() {
            ValidationFailure::RoundTooHigh => (),
            other => panic!("Expected RoundTooHigh error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_consensus_message_mismatched_identifier() {
        let (validator, _outcome_tx) =
            build_validator_and_outcome_channel(GLOBAL_EXECUTOR.clone(), 1);

        let signers = vec![OperatorId(42)];
        // Create two different MessageIds.
        let msg_id_a = create_message_id_for_test(Role::Committee);
        let msg_id_b = create_message_id_for_test(Role::Proposer);
        let qbft_msg = QbftMessage {
            qbft_message_type: QbftMessageType::Proposal,
            height: 1,
            round: 1,
            identifier: msg_id_b.clone(),
            root: Hash256::from([0u8; 32]),
            data_round: 1,
            round_change_justification: vec![],
            prepare_justification: vec![],
        };
        let ssv_msg = test_utils::create_consensus_ssv_message(qbft_msg, msg_id_a);
        let signed_msg = create_signed_ssv_message(
            vec![vec![0xAA; RSA_SIGNATURE_SIZE]],
            signers,
            ssv_msg,
            vec![],
        );

        let result = validator.validate_ssv_message(&signed_msg, signed_msg.ssv_message());
        assert!(
            result.is_err(),
            "Expected mismatched identifier to fail validation"
        );
        match result.err().unwrap() {
            ValidationFailure::MismatchedIdentifier { got, want } => {
                // Expect hexadecimal strings representing the differing ids.
                // Adjust these expectations as appropriate.
                assert_ne!(got, want, "Expected identifiers to differ");
            }
            other => panic!("Expected MismatchedIdentifier error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_consensus_message_decode_failure() {
        let (validator, _outcome_tx) =
            build_validator_and_outcome_channel(GLOBAL_EXECUTOR.clone(), 1);

        let signers = vec![OperatorId(42)];
        // Provide invalid consensus data.
        let msg_id = create_message_id_for_test(Role::Proposer);
        let invalid_data = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, msg_id, invalid_data)
            .expect("SSVMessage should be created");
        let signed_msg = create_signed_ssv_message(
            vec![vec![0xAA; RSA_SIGNATURE_SIZE]],
            signers,
            ssv_msg,
            vec![],
        );

        let result = validator.validate_ssv_message(&signed_msg, signed_msg.ssv_message());
        assert!(
            result.is_err(),
            "Expected decode failure for consensus message data"
        );
        match result.err().unwrap() {
            ValidationFailure::UndecodableMessageData => (),
            other => panic!("Expected UndecodableMessageData error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_prepare_justifications_with_non_proposal_message() {
        let (validator, _outcome_tx) =
            build_validator_and_outcome_channel(GLOBAL_EXECUTOR.clone(), 1);

        let signers = vec![OperatorId(42)];
        let msg_id = create_message_id_for_test(Role::Committee);
        // Create a Prepare message (non-Proposal) with a non-empty prepare justification.
        let qbft_msg = QbftMessage {
            qbft_message_type: QbftMessageType::Prepare,
            height: 1,
            round: 1,
            identifier: msg_id.clone(),
            root: Hash256::from([0u8; 32]),
            data_round: 1,
            round_change_justification: vec![],
            prepare_justification: vec![dummy_signed_ssv_message_for_justification()],
        };
        let ssv_msg = test_utils::create_consensus_ssv_message(qbft_msg, msg_id);
        let signed_msg = create_signed_ssv_message(
            vec![vec![0xAA; RSA_SIGNATURE_SIZE]],
            signers,
            ssv_msg,
            vec![],
        );

        let result = validator.validate_ssv_message(&signed_msg, signed_msg.ssv_message());
        assert!(
            result.is_err(),
            "Expected non-empty prepare_justifications in a non-Proposal to fail"
        );
        match result.err().unwrap() {
            ValidationFailure::UnexpectedPrepareJustifications => (),
            other => panic!(
                "Expected UnexpectedPrepareJustifications error, got: {:?}",
                other
            ),
        }
    }

    #[tokio::test]
    async fn test_round_change_justifications_with_non_proposal_or_roundchange() {
        let (validator, _outcome_tx) =
            build_validator_and_outcome_channel(GLOBAL_EXECUTOR.clone(), 1);

        let signers = vec![OperatorId(42)];
        let msg_id = create_message_id_for_test(Role::Committee);
        // Create a Commit message with non-empty round_change_justification.
        let qbft_msg = QbftMessage {
            qbft_message_type: QbftMessageType::Commit,
            height: 1,
            round: 1,
            identifier: msg_id.clone(),
            root: Hash256::from([0u8; 32]),
            data_round: 1,
            round_change_justification: vec![dummy_signed_ssv_message_for_justification()],
            prepare_justification: vec![],
        };
        let ssv_msg = test_utils::create_consensus_ssv_message(qbft_msg, msg_id);
        let signed_msg = create_signed_ssv_message(
            vec![vec![0xAA; RSA_SIGNATURE_SIZE]],
            signers,
            ssv_msg,
            vec![],
        );

        let result = validator.validate_ssv_message(&signed_msg, signed_msg.ssv_message());
        assert!(
            result.is_err(),
            "Expected non-empty round_change_justifications in a Commit to fail"
        );
        match result.err().unwrap() {
            ValidationFailure::UnexpectedRoundChangeJustifications => (),
            other => panic!(
                "Expected UnexpectedRoundChangeJustifications error, got: {:?}",
                other
            ),
        }
    }

    #[tokio::test]
    async fn test_compute_quorum_size() {
        // For committee_size=4 -> f=1 -> quorum=3.
        assert_eq!(
            compute_quorum_size(4),
            3,
            "Expected quorum=3 for committee of 4"
        );
        // For committee_size=7 -> f=2 -> quorum=5.
        assert_eq!(
            compute_quorum_size(7),
            5,
            "Expected quorum=5 for committee of 7"
        );
        // For committee_size=1 -> f=0 -> quorum=1.
        assert_eq!(
            compute_quorum_size(1),
            1,
            "Expected quorum=1 for committee of 1"
        );
    }
    //
    // #[tokio::test]
    // async fn test_hash_data_root() {
    //     let data = b"hello world";
    //     let hash_of_data = hash_data_root(data);
    //     let expected_hash = quick_hash(data);
    //     assert_eq!(
    //         hash_of_data, expected_hash,
    //         "hash_data_root should match the SHA256 hash for the given input"
    //     );
    // }
}
