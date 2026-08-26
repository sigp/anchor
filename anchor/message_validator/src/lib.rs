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
use duties_tracker::DutyAssignment;
use fork::{Fork, ForkSchedule};
use libp2p::PeerId;
pub use libp2p::gossipsub::MessageAcceptance;
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
    message::{MsgType, SSVMessageError, SignedSSVMessage, SignedSSVMessageError},
    msgid::{DutyExecutor, MessageId, Role},
    partial_sig::PartialSignatureMessages,
};
use ssz::{Decode, DecodeError, Encode};
use subnet_service::topic::ParsedTopic;
use task_executor::TaskExecutor;
use tokio::{sync::watch::Receiver, time::sleep};
use tracing::{debug, trace};
use types::{ChainSpec, Epoch, ForkName, Slot};

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
    /// Topic's fork doesn't match the fork that should be active for the message's slot.
    ///
    /// Per SIP-43, messages should be on topics matching their slot's fork. For example,
    /// a message for a post-fork slot should be on a post-fork topic.
    TopicForkMismatch,
    /// Could not extract slot from message data.
    ///
    /// The message data could not be decoded to extract the slot information needed
    /// for slot-based validation.
    UnknownMessageSlot,
    NonExistentCommitteeID,
    RoundTooHigh,
    ValidatorIndexMismatch,
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
    TooManyValidatorIndexOccurrences {
        validator_index: ValidatorIndex,
        got: usize,
        limit: usize,
    },
    ZeroRound,
    RoundOverflow,
    DuplicatedMessage {
        got: String,
    }, // Updated to include context
    InvalidPartialSignatureTypeCount {
        got: String,
    },
    TooManyDistinctSigningRoots {
        got: String,
    },
    /// Any repeat of a signing root already recorded for the message's
    /// (`MessageId`, operator, slot, partial-signature kind), regardless of the propagation
    /// peer (SIP-94 §7). Ignore-class.
    RelayedDuplicateMessage {
        got: String,
    },
    TooManyPartialSignatureMessages {
        got: usize,
        limit: usize,
    },
    EncodeOperators,
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
    RoleNotActiveBeforeFork {
        role: Role,
        current_fork: fork::Fork,
        minimum_fork: fork::Fork,
    },
    RoleNotActiveAfterFork {
        role: Role,
        current_fork: fork::Fork,
        deprecated_since_fork: fork::Fork,
    },
    /// A role that only exists at/after an Ethereum hard fork was seen before that fork
    /// activated.
    RoleNotActiveBeforeEthFork {
        role: Role,
        current_fork: ForkName,
        minimum_fork: ForkName,
    },
    /// A role deprecated at an Ethereum hard fork was seen at/after that fork activated.
    RoleNotActiveAfterEthFork {
        role: Role,
        current_fork: ForkName,
        deprecated_since_fork: ForkName,
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
            | ValidationFailure::ExcessiveDutyCount { .. }
            | ValidationFailure::NoDuty
            | ValidationFailure::EstimatedRoundNotInAllowedSpread { .. }
            | ValidationFailure::TooManyDistinctSigningRoots { .. }
            | ValidationFailure::RelayedDuplicateMessage { .. } => MessageAcceptance::Ignore,
            _ => MessageAcceptance::Reject,
        }
    }
}

impl From<SignedSSVMessageError> for ValidationFailure {
    fn from(err: SignedSSVMessageError) -> Self {
        match err {
            // Reachable: `validate()` checks these on SSZ-decoded messages
            SignedSSVMessageError::WrongRSASignatureSize { .. } => {
                ValidationFailure::WrongRSASignatureSize
            }
            SignedSSVMessageError::NoSigners => ValidationFailure::NoSigners,
            SignedSSVMessageError::NoSignatures => ValidationFailure::NoSignatures,
            SignedSSVMessageError::ZeroSigner => ValidationFailure::ZeroSigner,
            SignedSSVMessageError::DuplicatedSigner => ValidationFailure::DuplicatedSigner,
            SignedSSVMessageError::SignersAndSignaturesWithDifferentLength => {
                ValidationFailure::SignersAndSignaturesWithDifferentLength
            }
            SignedSSVMessageError::SSVMessageError(ssv_err) => match ssv_err {
                SSVMessageError::EmptyData => ValidationFailure::EmptyData,
                SSVMessageError::SSVDataTooBig { .. } => ValidationFailure::SSVDataTooBig,
                SSVMessageError::WrongDomain { .. } => ValidationFailure::WrongDomain,
                SSVMessageError::SignerNotInCommittee { .. } => {
                    ValidationFailure::SignerNotInCommittee
                }
            },
            // Not returned by `validate()`:
            // - `TooMany*` / `FullDataTooLong`: only from `new()`/`aggregate()` when converting raw
            //   Vecs into VariableLists. `validate()` operates on data already in `VariableList`
            //   form, so these are type-enforced.
            // - `SignersNotSorted`: removed from `validate()` because the Go spec's `Validate()`
            //   does not enforce sorting and Go's `Aggregate()` appends without sorting.
            SignedSSVMessageError::TooManySignatures { .. }
            | SignedSSVMessageError::TooManyOperatorIDs { .. }
            | SignedSSVMessageError::FullDataTooLong { .. }
            | SignedSSVMessageError::SignersNotSorted => ValidationFailure::UnexpectedFailure {
                msg: err.to_string(),
            },
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

/// Context for topic-aware message validation.
///
/// This enum makes explicit whether topic validation should be performed:
/// - `SkipValidation`: Used for tests where topic validation is not needed
/// - `Validate`: Used for incoming network messages where topic validation is required
///
/// For incoming network messages, always use `TopicContext::Validate`. If topic parsing
/// fails at the network layer, the message should be rejected immediately rather than
/// passed to the validator with skip context.
#[derive(Debug, Clone, Default)]
pub enum TopicContext {
    /// Skip topic validation entirely.
    ///
    /// Used for testing scenarios where topic context is irrelevant.
    #[default]
    SkipValidation,

    /// Validate message against the parsed topic.
    ///
    /// Used for incoming network messages where we need to verify the message
    /// is on the correct subnet for its content.
    Validate {
        /// The parsed topic information (subnet_id, fork).
        parsed: ParsedTopic,
    },
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
    pub fork_schedule: Arc<ForkSchedule>,
    pub spec: Arc<ChainSpec>,
}

pub struct Validator<S: SlotClock, D: DutiesProvider> {
    network_state_rx: Receiver<NetworkState>,
    duty_state_map: DashMap<MessageId, DutyState>,
    slots_per_epoch: u64,
    epochs_per_sync_committee_period: u64,
    sync_committee_size: usize,
    duties_provider: Arc<D>,
    slot_clock: S,
    subnet_service: Arc<subnet_service::SubnetService<S>>,
    fork_schedule: Arc<ForkSchedule>,
    spec: Arc<ChainSpec>,
}

/// Decode and perform stateless structural validation of an outbound message.
pub fn validate_outbound(message_data: &[u8]) -> Result<Slot, ValidationFailure> {
    let signed_ssv_message = SignedSSVMessage::from_ssz_bytes(message_data)
        .map_err(ValidationFailure::UndecodableMessageData)?;
    validate_outbound_message(&signed_ssv_message)
}

/// Perform stateless structural validation of an outbound message and return its routing slot.
///
/// This is not an authorization boundary. It deliberately excludes all network, duty, timing,
/// fork-role, signature-verification, and validation-state checks. Outbound producers must enforce
/// those invariants before constructing the message. Incoming messages continue through
/// [`Validator::validate`], which owns gossip validation state.
pub fn validate_outbound_message(
    signed_ssv_message: &SignedSSVMessage,
) -> Result<Slot, ValidationFailure> {
    validate_structure_and_role(signed_ssv_message)?;
    signed_ssv_message
        .ssv_message()
        .extract_slot()
        .ok_or(ValidationFailure::UnknownMessageSlot)
}

fn validate_structure_and_role(
    signed_ssv_message: &SignedSSVMessage,
) -> Result<Role, ValidationFailure> {
    signed_ssv_message
        .validate()
        .map_err(ValidationFailure::from)?;
    signed_ssv_message
        .ssv_message()
        .msg_id()
        .role()
        .ok_or(ValidationFailure::InvalidRole)
}

impl<S: SlotClock + 'static, D: DutiesProvider> Validator<S, D> {
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        network_state_rx: Receiver<NetworkState>,
        slots_per_epoch: u64,
        epochs_per_sync_committee_period: u64,
        sync_committee_size: usize,
        duties_provider: Arc<D>,
        slot_clock: S,
        subnet_service: Arc<subnet_service::SubnetService<S>>,
        fork_schedule: Arc<ForkSchedule>,
        spec: Arc<ChainSpec>,
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
            subnet_service,
            fork_schedule,
            spec,
        });

        task_executor.spawn(Arc::clone(&validator).cleaner(), VALIDATOR_CLEANER_NAME);

        validator
    }

    /// Validate a message with topic context for fork-aware validation.
    ///
    /// The `topic_context` provides information about which topic the message was
    /// received on, enabling validation of whether the message is on the correct
    /// subnet for its committee based on the topic's fork.
    pub fn validate(
        &self,
        message_data: &[u8],
        topic_context: &TopicContext,
        received_from: Option<PeerId>,
    ) -> ValidationResult {
        match SignedSSVMessage::from_ssz_bytes(message_data) {
            Ok(signed_ssv_message) => {
                trace!(msg = ?signed_ssv_message, "SignedSSVMessage deserialized");
                match self.validate_decoded_message(
                    &signed_ssv_message,
                    topic_context,
                    received_from,
                ) {
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
        topic_context: &TopicContext,
        received_from: Option<PeerId>,
    ) -> Result<ValidatedMessage, ValidationFailure> {
        let role = validate_structure_and_role(signed_ssv_message)?;
        let ssv_message = signed_ssv_message.ssv_message();

        // Get committee ID for topic validation
        let committee_id = match ssv_message.msg_id().duty_executor() {
            Some(DutyExecutor::Committee(id)) => Some(id),
            _ => None,
        };

        // Get committee info based on role and duty executor
        let network_state = self.network_state_rx.borrow();
        let committee_info = match role {
            Role::Committee | Role::AggregatorCommittee => {
                let committee_id = committee_id.ok_or(ValidationFailure::NonExistentCommitteeID)?;
                network_state
                    .get_committee_info_by_committee_id(&committee_id)
                    .ok_or(ValidationFailure::NonExistentCommitteeID)?
            }
            // Validator roles use DutyExecutor::Validator with public key
            Role::Aggregator
            | Role::Proposer
            | Role::SyncCommittee
            | Role::ValidatorRegistration
            | Role::VoluntaryExit
            | Role::PTCAttester
            | Role::ProposerPreferences
            | Role::EnvelopeProposer => {
                let validator_pk = match ssv_message.msg_id().duty_executor() {
                    Some(DutyExecutor::Validator(pk)) => pk,
                    _ => return Err(ValidationFailure::UnknownValidator),
                };

                network_state
                    .get_committee_info_by_validator_pk(&validator_pk)
                    .ok_or(ValidationFailure::UnknownValidator)?
            }
        };

        // Validate topic - message is on correct subnet and has correct domain for its committee
        let operator_ids: Vec<_> = committee_info.committee_members.iter().copied().collect();
        self.validate_topic_and_domain(
            topic_context,
            committee_id,
            &operator_ids,
            ssv_message,
            ssv_message.msg_id(),
        )?;

        let operator_pub_keys =
            &get_operator_pub_keys(&network_state, &committee_info.committee_members);

        drop(network_state);

        let mut duty_state = self.get_duty_state(ssv_message.msg_id(), role, self.slots_per_epoch);

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
            fork_schedule: Arc::clone(&self.fork_schedule),
            spec: Arc::clone(&self.spec),
        };

        validate_ssv_message(
            validation_context,
            duty_state.value_mut(),
            self.duties_provider.clone(),
            received_from,
        )
        .map(|validated| ValidatedMessage::new(signed_ssv_message.clone(), validated))
    }

    /// Gets the duty state for a message ID, creating a new one if it doesn't exist
    fn get_duty_state(
        &self,
        message_id: &MessageId,
        role: Role,
        slots_per_epoch: u64,
    ) -> RefMut<'_, MessageId, DutyState> {
        self.duty_state_map
            .entry(message_id.clone())
            .or_insert_with(|| DutyState::new(stored_slot_count(role, slots_per_epoch, &self.spec)))
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

    /// Validates that a message is on the correct topic for its committee using slot-based rules.
    ///
    /// Per SIP-43, validation is slot-based: the message's slot determines which fork rules apply,
    /// and the topic serves as a consistency check.
    ///
    /// This performs three validations:
    /// 1. **Subnet validation**: The message is on the correct subnet for its committee.
    /// 2. **Domain validation**: The message's domain matches the expected domain for the fork that
    ///    is active at the message's slot.
    /// 3. **Topic consistency**: The topic's fork matches the fork that should be active for the
    ///    message's slot.
    ///
    /// # Arguments
    ///
    /// * `topic_context` - The parsed topic information (subnet_id, fork)
    /// * `committee_id` - The committee ID from the message
    /// * `operator_ids` - The operator IDs from the committee
    /// * `ssv_message` - The SSV message to validate (for slot extraction)
    /// * `msg_id` - The message ID containing the domain to validate
    ///
    /// # Returns
    ///
    /// * `Ok(())` if all validations pass or if validation is skipped
    /// * `Err(ValidationFailure::IncorrectTopic)` if the subnet is wrong
    /// * `Err(ValidationFailure::WrongDomain)` if the domain doesn't match the slot's fork
    /// * `Err(ValidationFailure::TopicForkMismatch)` if the topic's fork doesn't match the slot's
    ///   fork
    /// * `Err(ValidationFailure::UnknownMessageSlot)` if the slot cannot be extracted
    fn validate_topic_and_domain(
        &self,
        topic_context: &TopicContext,
        committee_id: Option<ssv_types::CommitteeId>,
        operator_ids: &[OperatorId],
        ssv_message: &ssv_types::message::SSVMessage,
        msg_id: &MessageId,
    ) -> Result<(), ValidationFailure> {
        let parsed = match topic_context {
            TopicContext::SkipValidation => {
                trace!("Topic validation skipped");
                return Ok(());
            }
            TopicContext::Validate { parsed } => parsed,
        };

        // Extract slot from message for slot-based validation
        let message_slot = ssv_message
            .extract_slot()
            .ok_or(ValidationFailure::UnknownMessageSlot)?;

        // Validate subnet using slot-based fork selection
        let committee_id =
            committee_id.unwrap_or_else(|| ssv_types::CommitteeId::from(operator_ids.to_vec()));

        let expected_subnet = self
            .subnet_service
            .subnet_for_committee_with_operators_at_slot(committee_id, operator_ids, message_slot)
            .map_err(|e| {
                debug!(?e, "Failed to calculate expected subnet");
                ValidationFailure::IncorrectTopic
            })?;

        if parsed.subnet_id != expected_subnet {
            debug!(
                actual_subnet = ?parsed.subnet_id,
                ?expected_subnet,
                topic_fork = ?parsed.fork,
                "Message on incorrect subnet"
            );
            return Err(ValidationFailure::IncorrectTopic);
        }

        // Determine the expected fork based on the message's slot
        let message_epoch = message_slot.epoch(self.slots_per_epoch);
        let expected_fork = self.subnet_service.router().active_fork(message_epoch);

        // Topic consistency check: verify topic's fork matches the slot's expected fork
        if parsed.fork != expected_fork {
            debug!(
                topic_fork = ?parsed.fork,
                ?expected_fork,
                ?message_slot,
                ?message_epoch,
                "Topic fork does not match expected fork for message slot"
            );
            return Err(ValidationFailure::TopicForkMismatch);
        }

        // Validate domain against the fork active at the message's slot
        let expected_domain = self
            .subnet_service
            .router()
            .domain_type_for_epoch(message_epoch)
            .ok_or_else(|| {
                debug!(
                    ?expected_fork,
                    ?message_epoch,
                    "Unknown fork, cannot validate domain"
                );
                ValidationFailure::WrongDomain
            })?;

        let msg_domain = msg_id.domain();
        if msg_domain != expected_domain {
            debug!(
                ?msg_domain,
                ?expected_domain,
                fork = ?expected_fork,
                ?message_slot,
                "Message domain does not match expected domain for slot's fork"
            );
            return Err(ValidationFailure::WrongDomain);
        }

        trace!(subnet = ?expected_subnet, fork = ?expected_fork, "Topic validation passed");
        Ok(())
    }
}

/// Number of slots the `DutyState` ring must retain for `role`.
///
/// Two consumers rely on the sizing (the ring is indexed by `slot % len`):
/// - Per-slot signer state (dedup, message counts): an entry must not be evicted while its slot is
///   still inside the role's message acceptance window (earliness + lateness, see
///   `early_slot_allowance` / `message_lateness`).
/// - `OperatorState::get_duty_count`, which derives per-epoch duty counts from ring occupancy
///   (SIP-94 §7 retention). Exactness needs MORE than the acceptance window: a count query for the
///   epoch of the oldest acceptable slot probes back to that epoch's FIRST slot, so the ring must
///   cover `earliness + lateness + slots_per_epoch`, plus one slot padding the sub-slot lateness
///   margins (`LATE_MESSAGE_MARGIN` + `CLOCK_ERROR_TOLERANCE`).
///
/// The default arm covers the widest default-role window (lateness `slots_per_epoch +
/// LATE_SLOT_ALLOWANCE`, no earliness). `ProposerPreferences` spans the proposer lookahead into
/// the future (its envelope slot is a future `proposal_slot`) with a 2-slot lateness; its
/// lookahead-sized ring exceeds its bound (`64 + 2 + 32 + 1 = 99 <= 128`) with headroom.
///
/// The match is exhaustive so adding a role forces an explicit sizing decision here; a silent
/// default would under-size a wide-window role and quietly disable its duty limit.
///
/// Shared by the production selector (`get_duty_state`) and its regression test so neither can
/// drift from the other.
pub(crate) fn stored_slot_count(role: Role, slots_per_epoch: u64, spec: &ChainSpec) -> usize {
    let count = match role {
        Role::ProposerPreferences => {
            (1 + spec.min_seed_lookahead.as_u64()) * slots_per_epoch + 2 * slots_per_epoch
        }
        Role::Committee
        | Role::Aggregator
        | Role::Proposer
        | Role::SyncCommittee
        | Role::ValidatorRegistration
        | Role::VoluntaryExit
        | Role::PTCAttester
        | Role::EnvelopeProposer
        | Role::AggregatorCommittee => 2 * slots_per_epoch + LATE_SLOT_ALLOWANCE + 1,
    };
    count as usize
}

fn validate_ssv_message(
    validation_context: ValidationContext<impl SlotClock>,
    duty_state: &mut DutyState,
    duty_provider: Arc<impl DutiesProvider>,
    received_from: Option<PeerId>,
) -> Result<ValidatedSSVMessage, ValidationFailure> {
    let ssv_message = validation_context.signed_ssv_message.ssv_message();

    match ssv_message.msg_type() {
        MsgType::SSVConsensusMsgType => {
            validate_consensus_message(validation_context, duty_state, duty_provider)
        }
        MsgType::SSVPartialSignatureMsgType => validate_partial_signature_message(
            validation_context,
            duty_state,
            duty_provider,
            received_from,
        ),
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

/// Returns the single validator index of a non-committee duty message.
///
/// The index is absent when the validator is known locally but its beacon metadata has not been
/// synced yet. That is a gap in the local view, not a fault of the sending peer, so it maps to
/// [`ValidationFailure::NoShareMetadata`] (Ignore) rather than a Reject.
fn single_validator_index(
    validation_context: &ValidationContext<impl SlotClock>,
) -> Result<ValidatorIndex, ValidationFailure> {
    validation_context
        .committee_info
        .validator_indices
        .first()
        .copied()
        .ok_or(ValidationFailure::NoShareMetadata)
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

        let validator_index = single_validator_index(validation_context)?;

        if !duty_provider.is_validator_proposer_at_slot(slot, validator_index) {
            return Err(ValidationFailure::NoDuty);
        }
    }

    // Rule: For a proposer-preferences or envelope-proposer message, the validator must be the
    // assigned proposer at the slot. Checked only once the slot-epoch's proposer duties are known
    // locally, so a not-yet-fetched epoch is tolerated. No RANDAO tolerance: neither
    // ProposerPreferences nor EnvelopeProposer carry a RANDAO signature.
    if matches!(role, Role::ProposerPreferences | Role::EnvelopeProposer) {
        let validator_pubkey = match validation_context
            .signed_ssv_message
            .ssv_message()
            .msg_id()
            .duty_executor()
        {
            Some(DutyExecutor::Validator(public_key)) => public_key,
            _ => return Err(ValidationFailure::UnknownValidator),
        };

        if duty_provider.proposer_assignment_at_slot(slot, &validator_pubkey)
            == DutyAssignment::NotAssigned
        {
            return Err(ValidationFailure::NoDuty);
        }
    }

    // Rule: For a sync committee duty message, check if the validator is assigned
    if role == Role::SyncCommittee {
        let period =
            sync_committee_period(epoch, validation_context.epochs_per_sync_committee_period)?;
        let validator_index = single_validator_index(validation_context)?;

        if !duty_provider.is_validator_in_sync_committee(period, validator_index) {
            return Err(ValidationFailure::NoDuty);
        }
    }

    Ok(())
}

/// Validates that a role is allowed for the fork active at the given slot.
///
/// Rejects:
/// - AggregatorCommittee before Boole fork (not yet active)
/// - Aggregator and SyncCommittee after Boole fork (deprecated)
/// - PTCAttester before the Ethereum Gloas (ePBS) fork (not yet active)
/// - ValidatorRegistration at/after the Ethereum Gloas (ePBS) fork (deprecated by SIP-94)
/// - ProposerPreferences before the Ethereum Gloas (ePBS) fork (not yet active)
/// - EnvelopeProposer before the Ethereum Gloas (ePBS) fork (not yet active)
pub(crate) fn validate_role_for_fork(
    slot: Slot,
    validation_context: &ValidationContext<impl SlotClock>,
) -> Result<(), ValidationFailure> {
    let role = validation_context.role;
    let epoch = slot.epoch(validation_context.slots_per_epoch);
    let active_fork = validation_context.fork_schedule.active_fork(epoch);

    // Reject AggregatorCommittee before Boole fork (safety net)
    if role == Role::AggregatorCommittee && active_fork < Fork::Boole {
        return Err(ValidationFailure::RoleNotActiveBeforeFork {
            role,
            current_fork: active_fork,
            minimum_fork: Fork::Boole,
        });
    }

    // Reject deprecated roles after Boole fork
    if matches!(role, Role::Aggregator | Role::SyncCommittee) && active_fork >= Fork::Boole {
        return Err(ValidationFailure::RoleNotActiveAfterFork {
            role,
            current_fork: active_fork,
            deprecated_since_fork: Fork::Boole,
        });
    }

    // Reject ValidatorRegistration at/after the Ethereum Gloas (ePBS) fork; SIP-94
    // deprecates the duty (proposer preferences replace relay registrations). Gated
    // on the message's duty slot, not wall clock, so registrations for pre-fork
    // slots remain valid through their TTL window. Wire values are retained for
    // pre-Gloas decode per SIP-94.
    if role == Role::ValidatorRegistration {
        let current_fork = validation_context.spec.fork_name_at_epoch(epoch);
        if current_fork.gloas_enabled() {
            return Err(ValidationFailure::RoleNotActiveAfterEthFork {
                role,
                current_fork,
                deprecated_since_fork: ForkName::Gloas,
            });
        }
    }

    // Reject post-Gloas roles (PTCAttester, ProposerPreferences, EnvelopeProposer) before the
    // Ethereum Gloas (ePBS) fork, read from the consensus spec.
    if matches!(
        role,
        Role::PTCAttester | Role::ProposerPreferences | Role::EnvelopeProposer
    ) {
        let current_fork = validation_context.spec.fork_name_at_epoch(epoch);
        if !current_fork.gloas_enabled() {
            return Err(ValidationFailure::RoleNotActiveBeforeEthFork {
                role,
                current_fork,
                minimum_fork: ForkName::Gloas,
            });
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
    if earliness > CLOCK_ERROR_TOLERANCE + early_slot_allowance(validation_context) {
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

/// Extra future-slot tolerance (on top of `CLOCK_ERROR_TOLERANCE`) allowed for a role's message
/// slot. Only `ProposerPreferences` is non-zero: its envelope slot is the duty's future
/// `proposal_slot`, so the whole proposer lookahead (current epoch + `min_seed_lookahead`) ahead of
/// that slot must be accepted. Every other role keeps the strict no-future rule.
fn early_slot_allowance(validation_context: &ValidationContext<impl SlotClock>) -> Duration {
    match validation_context.role {
        Role::ProposerPreferences => {
            let allowance_slots = u32::try_from(
                (1 + validation_context.spec.min_seed_lookahead.as_u64())
                    * validation_context.slots_per_epoch,
            )
            .unwrap_or(u32::MAX);
            validation_context
                .slot_clock
                .slot_duration()
                .saturating_mul(allowance_slots)
        }
        _ => Duration::ZERO,
    }
}

/// Returns how late a message is compared to its deadline based on role.
/// If the message was received before the deadline, it returns 0.
/// If the message was received after the deadline, it returns the duration by which it was late.
fn message_lateness(
    slot: Slot,
    validation_context: &ValidationContext<impl SlotClock>,
) -> Result<Duration, ValidationFailure> {
    let ttl = match validation_context.role {
        Role::Proposer | Role::SyncCommittee | Role::PTCAttester | Role::EnvelopeProposer => {
            1 + LATE_SLOT_ALLOWANCE
        }
        Role::Committee
        | Role::Aggregator
        | Role::ValidatorRegistration
        | Role::VoluntaryExit
        | Role::AggregatorCommittee => validation_context.slots_per_epoch + LATE_SLOT_ALLOWANCE,
        Role::ProposerPreferences => LATE_SLOT_ALLOWANCE,
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
    operator_state: &OperatorState,
    duty_provider: Arc<impl DutiesProvider>,
) -> Result<(), ValidationFailure> {
    let Some(limit) = duty_limit(
        validation_context,
        slot,
        &validation_context.committee_info.validator_indices,
        duty_provider,
    )?
    else {
        return Ok(());
    };

    // We only want to check the limit if this is the first message of that duty, as otherwise
    // the check will fail for non-first messages of the last allowed duty. We do this by
    // checking if there is a signer state already set for that slot. If so, we have already
    // processed a message for this duty and it contributes no new count; skipping also
    // avoids deriving the count from the ring for every follow-up message.
    if !operator_state.is_first_message_for_duty(slot) {
        return Ok(());
    }

    // Error if this validator has already been assigned at least as many duties as allowed
    // for the target epoch. We perform this check *before* the duty is recorded in the ring
    // (so the very first duty will see count==0), hence the inclusive “>=” comparison.
    let epoch = slot.epoch(validation_context.slots_per_epoch);
    let duty_count = operator_state.get_duty_count(epoch, validation_context.slots_per_epoch);
    if duty_count >= limit {
        return Err(ValidationFailure::ExcessiveDutyCount {
            got: duty_count,
            limit,
            role: validation_context.role,
        });
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
        // Validator-scoped roles with at most ~1 duty per validator per epoch
        // (the duty counter is keyed per-validator); the limit of 2 leaves a
        // one-duty margin for epoch-boundary/reorg edge cases.
        Role::Aggregator | Role::ValidatorRegistration | Role::PTCAttester => Ok(Some(2)),
        // Committee roles (Committee and AggregatorCommittee) use the same duty limit formula:
        // min(slots_per_epoch, 2*validator_count), or slots_per_epoch if any validator is in sync
        // committee
        Role::Committee | Role::AggregatorCommittee => {
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
        // Proposer and SyncCommittee have no duty limit
        Role::Proposer | Role::SyncCommittee => Ok(None),
        // Per-proposal-slot roles: max duties capped at SLOTS_PER_EPOCH (one preferences packet /
        // one self-build envelope per proposal slot). Overflow is IGNORE-classified. Both
        // ProposerPreferences kinds (preferences and request-auth) share each proposal slot's
        // single ring entry, so kind-9 packets add no distinct slots beyond kind-8's; this is
        // the stricter reading of SIP-94 §7's "type-9 messages ride existing duty slots".
        Role::ProposerPreferences | Role::EnvelopeProposer => {
            Ok(Some(validation_context.slots_per_epoch))
        }
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

pub(crate) fn hash_data(full_data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(full_data);
    let hash: [u8; 32] = hasher.finalize().into();
    hash
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use bls::{Hash256, PublicKeyBytes, Signature};
    use duties_tracker::{DutiesProvider, DutyAssignment};
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
        partial_sig::{PartialSignatureKind, PartialSignatureMessage, PartialSignatureMessages},
    };
    use ssz::Encode;
    use types::{Epoch, Slot};

    use crate::{MessageAcceptance, ValidationFailure, hash_data, validate_outbound};

    // Constants for committee sizes in tests to improve readability.
    pub(crate) const SINGLE_NODE_COMMITTEE: usize = 1;
    pub(crate) const FOUR_NODE_COMMITTEE: usize = 4;

    fn signed_test_message(
        msg_type: MsgType,
        message_id: MessageId,
        data: Vec<u8>,
    ) -> SignedSSVMessage {
        let ssv_message = SSVMessage::new(msg_type, message_id, data)
            .expect("test SSVMessage should be structurally valid");
        SignedSSVMessage::new(
            vec![[0xAA; RSA_SIGNATURE_SIZE]],
            vec![OperatorId(1)],
            ssv_message,
            vec![],
        )
        .expect("test SignedSSVMessage should be structurally valid")
    }

    #[test]
    fn validate_outbound_returns_nested_message_slots() {
        let consensus_message_id = create_message_id_for_test(Role::Committee);
        let mut qbft_message = QbftMessageBuilder::new(Role::Committee, QbftMessageType::Proposal)
            .with_identifier(consensus_message_id.clone())
            .build();
        qbft_message.height = 42;
        let signed_consensus_message = signed_test_message(
            MsgType::SSVConsensusMsgType,
            consensus_message_id,
            qbft_message.as_ssz_bytes(),
        );

        assert_eq!(
            validate_outbound(&signed_consensus_message.as_ssz_bytes()),
            Ok(Slot::new(42))
        );

        let partial_signature_messages = PartialSignatureMessages {
            kind: PartialSignatureKind::RandaoPartialSig,
            slot: Slot::new(43),
            messages: VariableList::new(vec![PartialSignatureMessage {
                partial_signature: Signature::empty(),
                signing_root: Hash256::ZERO,
                signer: OperatorId(1),
                validator_index: ValidatorIndex(0),
            }])
            .expect("one partial signature should fit"),
        };
        let signed_partial_signature_message = signed_test_message(
            MsgType::SSVPartialSignatureMsgType,
            create_message_id_for_test(Role::Proposer),
            partial_signature_messages.as_ssz_bytes(),
        );

        assert_eq!(
            validate_outbound(&signed_partial_signature_message.as_ssz_bytes()),
            Ok(Slot::new(43))
        );
    }

    #[test]
    fn validate_outbound_rejects_malformed_outer_and_nested_messages() {
        assert!(matches!(
            validate_outbound(&[]),
            Err(ValidationFailure::UndecodableMessageData(_))
        ));

        for (msg_type, role) in [
            (MsgType::SSVConsensusMsgType, Role::Committee),
            (MsgType::SSVPartialSignatureMsgType, Role::Proposer),
        ] {
            let signed_message =
                signed_test_message(msg_type, create_message_id_for_test(role), vec![0x01]);
            assert_eq!(
                validate_outbound(&signed_message.as_ssz_bytes()),
                Err(ValidationFailure::UnknownMessageSlot)
            );
        }
    }

    #[test]
    fn validate_outbound_rejects_duplicate_signers() {
        let qbft_message =
            QbftMessageBuilder::new(Role::Committee, QbftMessageType::Proposal).build();
        let mut signed_message =
            create_signed_consensus_message(qbft_message, vec![OperatorId(1)], vec![], vec![]);
        signed_message
            .aggregate([signed_message.clone()])
            .expect("aggregation permits duplicate signers for validation tests");

        assert_eq!(
            validate_outbound(&signed_message.as_ssz_bytes()),
            Err(ValidationFailure::DuplicatedSigner)
        );
    }

    #[test]
    fn validate_outbound_rejects_invalid_role() {
        let mut invalid_message_id = [0u8; 56];
        invalid_message_id[4] = u8::MAX;
        let invalid_message_id = MessageId::from(invalid_message_id);
        let qbft_message = QbftMessageBuilder::new(Role::Committee, QbftMessageType::Proposal)
            .with_identifier(invalid_message_id.clone())
            .build();
        let signed_message = signed_test_message(
            MsgType::SSVConsensusMsgType,
            invalid_message_id,
            qbft_message.as_ssz_bytes(),
        );

        assert_eq!(
            validate_outbound(&signed_message.as_ssz_bytes()),
            Err(ValidationFailure::InvalidRole)
        );
    }

    /// Test that an `ExcessiveDutyCount` maps to `Ignore`.
    /// Duty-limit breach is a rate condition. An honest relayer can forward a message that
    /// pushes a signer over its per-epoch duty count. Not a provable protocol violation.
    #[test]
    fn excessive_duty_count_maps_to_ignore() {
        // Duty-limit breach (count over the per-epoch limit for a committee duty).
        let failure = ValidationFailure::ExcessiveDutyCount {
            got: 5,
            limit: 4,
            role: Role::Committee,
        };

        // Gossip classification must be Ignore, not Reject.
        assert_eq!(
            MessageAcceptance::from(&failure),
            MessageAcceptance::Ignore,
            "duty-limit breach (ExcessiveDutyCount) must classify as Ignore, not Reject."
        );
    }

    // Helper struct for directly creating consensus messages for tests
    pub(crate) struct QbftMessageBuilder {
        msg_type: QbftMessageType,
        height: u64,
        round: u64,
        identifier: MessageId,
        prepare_justification: Vec<SignedSSVMessage>,
        round_change_justification: Vec<SignedSSVMessage>,
    }

    impl QbftMessageBuilder {
        pub(crate) fn new(role: Role, msg_type: QbftMessageType) -> Self {
            Self {
                msg_type,
                height: 1,
                round: 1,
                identifier: create_message_id_for_test(role),
                prepare_justification: vec![],
                round_change_justification: vec![],
            }
        }

        pub(crate) fn with_height(mut self, height: u64) -> Self {
            self.height = height;
            self
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
                height: self.height,
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
            Role::Committee | Role::AggregatorCommittee => {
                DutyExecutor::Committee(CommitteeId([0u8; 32]))
            }
            Role::Aggregator
            | Role::Proposer
            | Role::SyncCommittee
            | Role::ValidatorRegistration
            | Role::VoluntaryExit
            | Role::PTCAttester
            | Role::ProposerPreferences
            | Role::EnvelopeProposer => DutyExecutor::Validator(PublicKeyBytes::empty()),
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

    /// Build a `ChainSpec` whose Ethereum Gloas (ePBS) fork activates at
    /// `gloas_fork_epoch` (`None` = "Gloas never happens"). Used by role gates
    /// keyed to the Ethereum fork, e.g. PTCAttester activation and
    /// ValidatorRegistration deprecation.
    pub(crate) fn spec_with_gloas(gloas_fork_epoch: Option<u64>) -> Arc<types::ChainSpec> {
        let mut spec = types::ChainSpec::mainnet();
        spec.gloas_fork_epoch = gloas_fork_epoch.map(types::Epoch::new);
        Arc::new(spec)
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

    pub struct MockDutiesProvider {
        pub(crate) voluntary_exit_duty_count: u64,
        /// Value returned by `is_epoch_known_for_proposers`. Defaults to `true`
        /// so existing tests keep the historical "epoch always known" behavior.
        pub(crate) epoch_known_for_proposers: bool,
        /// Value returned by `is_validator_proposer_at_slot`. Defaults to `true`
        /// so existing tests keep the historical "validator is always proposer"
        /// behavior.
        pub(crate) validator_is_proposer: bool,
        /// Value returned by `proposer_assignment_at_slot`, the pubkey-keyed
        /// lookup used by the `ProposerPreferences` / `EnvelopeProposer` arm.
        /// `DutyAssignment::Assigned` = assigned proposer at the slot,
        /// `DutyAssignment::NotAssigned` = a fetched epoch proves the pubkey is
        /// not the proposer at the slot, `DutyAssignment::Unknown` = the slot's
        /// epoch is not fetched (unknown). Defaults to `DutyAssignment::Assigned`
        /// so pre-existing tests keep the "assigned proposer" behavior; new tests
        /// set it explicitly to drive the three cases.
        pub(crate) proposer_assignment: DutyAssignment,
    }

    // Manual `Default` (not derived) so the proposer flags default to their
    // "assigned" values, preserving the behavior all pre-existing tests relied on
    // before these fields were added. New tests set them explicitly to drive the
    // proposer-assignment arm.
    impl Default for MockDutiesProvider {
        fn default() -> Self {
            Self {
                voluntary_exit_duty_count: 0,
                epoch_known_for_proposers: true,
                validator_is_proposer: true,
                proposer_assignment: DutyAssignment::Assigned,
            }
        }
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
            self.epoch_known_for_proposers
        }

        fn is_validator_proposer_at_slot(
            &self,
            _slot: Slot,
            _validator_index: ValidatorIndex,
        ) -> bool {
            self.validator_is_proposer
        }

        fn get_voluntary_exit_duty_count(&self, _slot: Slot, _pubkey: &PublicKeyBytes) -> u64 {
            self.voluntary_exit_duty_count
        }

        fn proposer_assignment_at_slot(
            &self,
            _slot: Slot,
            _validator_pubkey: &PublicKeyBytes,
        ) -> DutyAssignment {
            self.proposer_assignment
        }
    }

    // ---------------------------------------------------------------------
    // Utility function tests
    // ---------------------------------------------------------------------

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
