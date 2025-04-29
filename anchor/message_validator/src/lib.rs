mod consensus_message;
mod consensus_state;
mod duties;
mod message_counts;
mod partial_signature;

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use dashmap::{mapref::one::RefMut, DashMap};
use database::NetworkState;
use gossipsub::MessageAcceptance;
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
    consensus::QbftMessage,
    message::{MsgType, SignedSSVMessage},
    msgid::{DutyExecutor, MessageId, Role},
    partial_sig::PartialSignatureMessages,
    CommitteeInfo, OperatorId,
};
use ssz::{Decode, Encode};
use tokio::sync::watch::Receiver;
use tracing::{error, trace};
use types::{Epoch, Slot};

pub use crate::duties::{duties_tracker::DutiesTracker, DutiesProvider};
use crate::{
    consensus_message::validate_consensus_message, consensus_state::ConsensusState,
    partial_signature::validate_partial_signature_message,
};

// TODO taken from go-SSV as rough guidance. feel free to adjust as needed. https://github.com/ssvlabs/ssv/blob/e12abf7dfbbd068b99612fa2ebbe7e3372e57280/message/validation/errors.go#L55
#[derive(Debug)]
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
    UndecodableMessageData,
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
    MalformedPrepareJustifications,
    UnexpectedPrepareJustifications,
    MalformedRoundChangeJustifications,
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
    DuplicatedMessage {
        got: String,
    }, // Updated to include context
    InvalidPartialSignatureTypeCount,
    TooManyPartialSignatureMessages,
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
    pub operators_pk: &'a [Rsa<Public>],
    pub slots_per_epoch: u64,
    pub epochs_per_sync_committee_period: u64,
    pub slot_clock: S,
}

pub struct Validator<S: SlotClock, D: DutiesProvider> {
    network_state_rx: Receiver<NetworkState>,
    consensus_state_map: DashMap<MessageId, ConsensusState>,
    slots_per_epoch: u64,
    epochs_per_sync_committee_period: u64,
    duties_provider: Arc<D>,
    slot_clock: S,
}

impl<S: SlotClock, D: DutiesProvider> Validator<S, D> {
    pub fn new(
        network_state_rx: Receiver<NetworkState>,
        slots_per_epoch: u64,
        epochs_per_sync_committee_period: u64,
        duties_provider: Arc<D>,
        slot_clock: S,
    ) -> Self {
        Self {
            network_state_rx,
            consensus_state_map: DashMap::new(),
            slots_per_epoch,
            epochs_per_sync_committee_period,
            duties_provider,
            slot_clock,
        }
    }

    pub fn validate(&self, message_data: &[u8]) -> Result<ValidatedMessage, ValidationFailure> {
        match SignedSSVMessage::from_ssz_bytes(message_data) {
            Ok(signed_ssv_message) => {
                trace!(msg = ?signed_ssv_message, "SignedSSVMessage deserialized");

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
                let operators_pks =
                    get_operator_pks(&network_state, signed_ssv_message.operator_ids())?;
                drop(network_state);

                let mut consensus_state =
                    self.get_consensus_state(ssv_message.msg_id(), self.slots_per_epoch);

                let validation_context = ValidationContext {
                    signed_ssv_message: &signed_ssv_message,
                    role,
                    committee_info: &committee_info,
                    received_at: SystemTime::now(),
                    operators_pk: &operators_pks,
                    slots_per_epoch: self.slots_per_epoch,
                    epochs_per_sync_committee_period: self.epochs_per_sync_committee_period,
                    slot_clock: self.slot_clock.clone(),
                };

                validate_ssv_message(
                    validation_context,
                    consensus_state.value_mut(),
                    self.duties_provider.clone(),
                )
                .map(|validated| ValidatedMessage::new(signed_ssv_message.clone(), validated))
            }
            Err(error) => {
                trace!("error" = ?error, "Failed to deserialize SignedSSVMessage");
                Err(ValidationFailure::UndecodableMessageData)
            }
        }
    }

    /// Gets the consensus state for a message ID, creating a new one if it doesn't exist
    fn get_consensus_state(
        &self,
        message_id: &MessageId,
        slots_per_epoch: u64,
    ) -> RefMut<MessageId, ConsensusState> {
        self.consensus_state_map
            .entry(message_id.clone())
            .or_insert_with(|| {
                let stored_slot_count = slots_per_epoch * 2; // Store last two epochs

                ConsensusState::new(stored_slot_count as usize)
            })
    }
}

fn validate_ssv_message(
    validation_context: ValidationContext<impl SlotClock>,
    consensus_state: &mut ConsensusState,
    duty_provider: Arc<impl DutiesProvider>,
) -> Result<ValidatedSSVMessage, ValidationFailure> {
    let ssv_message = validation_context.signed_ssv_message.ssv_message();

    match ssv_message.msg_type() {
        MsgType::SSVConsensusMsgType => {
            validate_consensus_message(validation_context, consensus_state, duty_provider)
        }
        MsgType::SSVPartialSignatureMsgType => {
            validate_partial_signature_message(validation_context)
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
            reason: format!("Failed to create PKey: {}", e),
        }
    })?;

    let mut verifier = Verifier::new(MessageDigest::sha256(), &p_key).map_err(|e| {
        ValidationFailure::SignatureVerificationFailed {
            reason: format!("Failed to create verifier: {}", e),
        }
    })?;

    verifier
        .update(&signed_message.ssv_message().as_ssz_bytes())
        .map_err(|e| ValidationFailure::SignatureVerificationFailed {
            reason: format!("Failed to update verifier: {}", e),
        })?;

    match verifier.verify(signature) {
        Ok(true) => Ok(()),
        Ok(false) => Err(ValidationFailure::SignatureVerificationFailed {
            reason: "Signature verification failed".to_string(),
        }),
        Err(e) => Err(ValidationFailure::SignatureVerificationFailed {
            reason: format!("Signature verification error: {}", e),
        }),
    }
}

/// Verifies all signatures in a signed SSV message
fn verify_message_signatures(
    signed_message: &SignedSSVMessage,
    operators_pks: &[Rsa<Public>],
) -> Result<(), ValidationFailure> {
    let signatures = signed_message.signatures();

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

fn get_operator_pks(
    network_state: &NetworkState,
    operator_ids: &[OperatorId],
) -> Result<Vec<Rsa<Public>>, ValidationFailure> {
    operator_ids
        .iter()
        .map(|o_id| {
            network_state
                .get_operator(o_id)
                .ok_or(ValidationFailure::OperatorNotFound { operator_id: *o_id })
                .map(|operator| operator.rsa_pubkey)
        })
        .collect() // This will combine all the Results into a single Result<Vec<>>
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
    use bls::PublicKeyBytes;
    use openssl::{pkey::Public, rsa::Rsa};
    use ssv_types::{
        domain_type::DomainType,
        msgid::{DutyExecutor, MessageId, Role},
        CommitteeId, CommitteeInfo, IndexSet, OperatorId, ValidatorIndex,
    };

    use crate::{compute_quorum_size, hash_data, ValidationFailure};

    // Constants for committee sizes in tests to improve readability
    pub(crate) const SINGLE_NODE_COMMITTEE: usize = 1;
    pub(crate) const FOUR_NODE_COMMITTEE: usize = 4;
    pub(crate) const SEVEN_NODE_COMMITTEE: usize = 7;

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
    pub fn create_message_id_for_test(role: Role) -> MessageId {
        let domain = DomainType([0, 0, 0, 1]);
        let duty_executor = match role {
            Role::Committee => DutyExecutor::Committee(CommitteeId([0u8; 32])),
            _ => DutyExecutor::Validator(PublicKeyBytes::empty()),
        };
        MessageId::new(&domain, role, &duty_executor)
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
