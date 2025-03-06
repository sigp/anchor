use database::NetworkState;
use libp2p::gossipsub::MessageAcceptance::{Accept, Reject};
use libp2p::gossipsub::{MessageAcceptance, MessageId};
use libp2p::PeerId;
use processor::Senders;
use sha2::{Digest, Sha256};
use ssv_types::consensus::{QbftMessage, QbftMessageType};
use ssv_types::message::{MsgType, SSVMessage, SignedSSVMessage};
use ssv_types::msgid::DutyExecutor;
use ssv_types::partial_sig::PartialSignatureMessages;
use ssz::Decode;
use std::sync::Arc;
use tokio::sync::mpsc::error::TrySendError::{Closed, Full};
use tokio::sync::mpsc::Sender;
use tokio::sync::watch;
use tracing::{error, trace};

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
    network_state_rxx: watch::Receiver<NetworkState>,
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
        network_state_rxx: watch::Receiver<NetworkState>,
    ) -> Self {
        Self {
            processor,
            result_tx,
            network_state_rxx,
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
                PartialSignatureMessages::from_ssz_bytes(ssv_message.data())
                    .ok()
                    .map(ValidatedSSVMessage::PartialSignatureMessages)
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

        let db = self.network_state_rxx.borrow();
        let committee_id = match signed_ssv_message.ssv_message().msg_id().duty_executor() {
            Some(DutyExecutor::Committee(id)) => id,
            _ => return Err(ValidationFailure::NonExistentCommitteeID),
        };

        let committee_members = match db.get_cluster_members(&committee_id) {
            Some(committee_members) => {
                if committee_members.is_empty() {
                    return Err(ValidationFailure::NoValidators);
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
