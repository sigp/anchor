use std::{fmt::Debug, future::Future, hash::Hash, num::NonZeroU64, sync::Arc};

use bls::PublicKeyBytes;
use dashmap::DashMap;
use database::OwnOperatorId;
use fork::{Fork, ForkSchedule};
use message_sender::MessageSender;
use processor::{Error::Queue, Senders, work::DropOnFinish};
use qbft::{
    Completed, ConfigBuilder, ConfigBuilderError, DefaultLeaderFunction, InstanceHeight,
    WrappedQbftMessage,
};
use slot_clock::SlotClock;
use ssv_types::{
    CommitteeId, IndexSet, OperatorId,
    consensus::{
        AggregatorCommitteeConsensusData, BeaconVote, PayloadAttestationVote,
        ProposerConsensusData, QbftData, QbftDataValidator,
    },
    domain_type::DomainType,
    message::SignedSSVMessage,
    msgid::{DutyExecutor, MessageId, Role},
};
use tokio::{
    sync::{
        mpsc,
        mpsc::{UnboundedSender, error::TrySendError},
        oneshot,
        oneshot::error::RecvError,
    },
    time::{Instant, sleep},
};
use tracing::{Instrument, debug_span, error, warn};
use types::{Epoch, EthSpec, Hash256, Slot};

use crate::instance::qbft_instance;

mod instance;
#[cfg(test)]
mod tests;
mod timeout;

const QBFT_INSTANCE_NAME: &str = "qbft_instance";
const QBFT_MESSAGE_NAME: &str = "qbft_message";
const QBFT_CLEANER_NAME: &str = "qbft_cleaner";

/// Number of slots to keep before the current slot
const QBFT_RETAIN_SLOTS: u64 = 1;

/// Determines how round timeouts are calculated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutMode {
    /// Cumulative timeouts from instance start. Never resets.
    /// Used for: attestations, aggregations, sync committee.
    SlotTime { instance_start_time: Instant },
    /// Per-round timeouts. Resets on round changes.
    /// Used for: block proposals.
    Relative { current_round_start_time: Instant },
}

// Unique Identifier for a committee and its corresponding QBFT instance
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CommitteeInstanceId {
    pub committee: CommitteeId,
    pub instance_height: InstanceHeight,
}

// Unique Identifier for an aggregator committee QBFT instance
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct AggregatorCommitteeInstanceId {
    pub committee: CommitteeId,
    pub instance_height: InstanceHeight,
}

// Unique Identifier for a PTC committee QBFT instance
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct PTCCommitteeInstanceId {
    pub committee: CommitteeId,
    pub instance_height: InstanceHeight,
}

// Unique Identifier for a proposer QBFT instance
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ProposerInstanceId {
    pub validator: PublicKeyBytes,
    // TODO(post-boole): remove `duty` field and `ValidatorDutyKind`. Post-boole,
    // `Proposal` is the only validator specific duty
    pub duty: ValidatorDutyKind,
    pub instance_height: InstanceHeight,
}

// TODO(post-boole): remove this enum. Post-boole, `Proposal` is the only
// validator specific duty kind.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum ValidatorDutyKind {
    Proposal,
    Aggregator,
    SyncCommitteeAggregator,
}

// Message that is passed around the QbftManager
pub struct QbftMessage<D: QbftData> {
    pub kind: QbftMessageKind<D>,
    pub drop_on_finish: Option<DropOnFinish>,
}

// Type of the QBFT Message
pub enum QbftMessageKind<D: QbftData> {
    // Initialize a new qbft instance with some initial data,
    // the configuration for the instance, and a channel to send the final data on
    Initialize(QbftInitialization<D>),
    // A message received from the network. The network exchanges SignedSsvMessages, but after
    // deserialization we determine the message is for the qbft instance and decode it into a
    // wrapped qbft message consisting of the signed message and the qbft message
    NetworkMessage(WrappedQbftMessage),
}

/// Represents the initialization data required to start a new QBFT instance.
pub struct QbftInitialization<D: QbftData> {
    /// The data to use when we are the leader.
    initial: D,
    /// The context needed for validation of other's data.
    validator: Box<dyn QbftDataValidator<D>>,
    /// The message id to be embedded into outgoing messages.
    message_id: MessageId,
    /// The timeout mode for this instance (includes timing reference).
    timeout_mode: TimeoutMode,
    /// The configuration for the instance.
    config: qbft::Config<DefaultLeaderFunction>,
    /// The channel to send the final result to.
    on_completed: oneshot::Sender<Completed<D>>,
}

// Map from an identifier to a sender for the instance
type Map<I, D> = DashMap<I, UnboundedSender<QbftMessage<D>>>;

// Top level QBFTManager structure
pub struct QbftManager<E: EthSpec, S: SlotClock> {
    // Senders to send work off to the central processor
    processor: Senders,
    // OperatorID
    operator_id: OwnOperatorId,
    // All of the QBFT instances that are voting on proposer consensus data
    proposer_consensus_data_instances: Map<ProposerInstanceId, ProposerConsensusData>,
    // All of the QBFT instances that are voting on beacon data
    beacon_vote_instances: Map<CommitteeInstanceId, BeaconVote>,
    // QBFT instances for AggregatorCommitteeConsensusData
    aggregator_committee_instances:
        Map<AggregatorCommitteeInstanceId, AggregatorCommitteeConsensusData<E>>,
    // QBFT instances for PayloadAttestationVote
    payload_attestation_vote_instances: Map<PTCCommitteeInstanceId, PayloadAttestationVote>,
    // Utility to sign and serialize network messages
    message_sender: Arc<dyn MessageSender>,
    // Number of slots per epoch
    slots_per_epoch: NonZeroU64,
    // Fork schedule for looking up the active fork's domain type
    fork_schedule: Arc<ForkSchedule>,
    // Slot clock for determining the current epoch
    slot_clock: S,
}

impl<E: EthSpec, S: SlotClock + Clone + 'static> QbftManager<E, S> {
    // Construct a new QBFT Manager
    pub fn new(
        processor: Senders,
        operator_id: OwnOperatorId,
        slot_clock: S,
        message_sender: Arc<dyn MessageSender>,
        slots_per_epoch: NonZeroU64,
        fork_schedule: Arc<ForkSchedule>,
    ) -> Result<Arc<Self>, QbftError> {
        let manager = Arc::new(QbftManager {
            processor,
            operator_id,
            proposer_consensus_data_instances: DashMap::new(),
            beacon_vote_instances: DashMap::new(),
            aggregator_committee_instances: DashMap::new(),
            payload_attestation_vote_instances: DashMap::new(),
            message_sender,
            slots_per_epoch,
            fork_schedule,
            slot_clock: slot_clock.clone(),
        });

        // Start a long running task that will clean up old instances
        manager
            .processor
            .permitless
            .send_async(Arc::clone(&manager).cleaner(), QBFT_CLEANER_NAME)?;

        Ok(manager)
    }

    /// Get the domain type for the slot associated with a QBFT instance.
    fn domain_type_for_instance(&self, instance_height: InstanceHeight) -> DomainType {
        let slot = Slot::new(*instance_height as u64);
        let epoch = slot.epoch(E::slots_per_epoch());
        self.fork_schedule.active_fork_config(epoch).domain_type
    }

    // Decide a brand new qbft instance
    pub async fn decide_instance<D: QbftDecidable<E>>(
        &self,
        id: D::Id,
        initial: D,
        validator: Box<dyn QbftDataValidator<D>>,
        timeout_mode: TimeoutMode,
        committee_members: &IndexSet<OperatorId>,
    ) -> Result<Completed<D>, QbftError> {
        let Some(operator_id) = self.operator_id.get() else {
            return Err(QbftError::OwnOperatorIdUnknown);
        };

        // Tx/Rx pair to send and retrieve the final result
        let (result_sender, result_receiver) = oneshot::channel();
        let instance_height = initial.instance_height(&id);
        let domain = self.domain_type_for_instance(instance_height);
        let message_id = D::message_id(&domain, &id);

        // Compute whether to include epoch shift based on fork schedule
        let instance_height = initial.instance_height(&id);
        let epoch = Epoch::new(*instance_height as u64 / self.slots_per_epoch);
        let include_epoch_shift = self.fork_schedule.active_fork(epoch) >= Fork::Boole;
        let leader_fn = DefaultLeaderFunction::new(self.slots_per_epoch, include_epoch_shift);

        // Generate the qbft configuration
        let config = ConfigBuilder::new_with_leader_fn(
            operator_id,
            instance_height,
            committee_members.iter().copied().collect(),
            leader_fn,
        );
        let config = config
            .with_max_rounds(
                message_id
                    .role()
                    .and_then(|r| r.max_round())
                    .ok_or(QbftError::InconsistentMessageId)? as usize,
            )
            .build()?;

        // Get or spawn a new qbft instance. This will return the sender that we can use to send
        // new messages to the specific instance
        let sender = D::get_or_spawn_instance(self, id);
        self.processor.urgent_consensus.send_immediate(
            move |drop_on_finish: DropOnFinish| {
                // A message to initialize this instance
                let _ = sender.send(QbftMessage {
                    kind: QbftMessageKind::Initialize(QbftInitialization {
                        initial,
                        validator,
                        message_id,
                        timeout_mode,
                        config,
                        on_completed: result_sender,
                    }),
                    drop_on_finish: Some(drop_on_finish),
                });
            },
            QBFT_MESSAGE_NAME,
        )?;

        // Await the final result
        Ok(result_receiver.await?)
    }

    /// Send a new network message to the instance
    pub fn receive_data(
        &self,
        full_message: SignedSSVMessage,
        qbft_message: ssv_types::consensus::QbftMessage,
    ) -> Result<(), QbftError> {
        let msg_id = full_message.ssv_message().msg_id();
        let instance_height = (qbft_message.height as usize).into();

        match msg_id.duty_executor() {
            Some(DutyExecutor::Validator(validator)) => {
                let duty = match msg_id.role() {
                    Some(Role::Proposer) => ValidatorDutyKind::Proposal,
                    Some(Role::Aggregator) => ValidatorDutyKind::Aggregator,
                    Some(Role::SyncCommittee) => ValidatorDutyKind::SyncCommitteeAggregator,
                    // Committee roles use DutyExecutor::Committee, not Validator
                    Some(Role::Committee | Role::AggregatorCommittee | Role::PTCCommittee)
                    // These roles don't use QBFT consensus
                    | Some(Role::ValidatorRegistration | Role::VoluntaryExit)
                    | None => {
                        error!(?msg_id, "Unexpected role/executor combination in msg id");
                        return Err(QbftError::InconsistentMessageId);
                    }
                };
                let id = ProposerInstanceId {
                    validator,
                    duty,
                    instance_height,
                };
                self.pass_to_instance::<ProposerConsensusData>(
                    id,
                    WrappedQbftMessage {
                        signed_message: full_message,
                        qbft_message,
                    },
                )
            }
            Some(DutyExecutor::Committee(committee)) => {
                match msg_id.role() {
                    Some(Role::Committee) => {
                        // Existing BeaconVote routing
                        let id = CommitteeInstanceId {
                            committee,
                            instance_height,
                        };
                        self.pass_to_instance::<BeaconVote>(
                            id,
                            WrappedQbftMessage {
                                signed_message: full_message,
                                qbft_message,
                            },
                        )
                    }
                    Some(Role::AggregatorCommittee) => {
                        // Route to aggregator committee instances with fork gating
                        let slot = types::Slot::new(qbft_message.height);
                        let epoch = slot.epoch(E::slots_per_epoch());

                        // Fork gating: Reject before Boole
                        if self.fork_schedule.active_fork(epoch) < Fork::Boole {
                            warn!(%slot, "Ignoring AggregatorCommittee message before Boole fork");
                            return Err(QbftError::RoleNotActive);
                        }

                        let id = AggregatorCommitteeInstanceId {
                            committee,
                            instance_height,
                        };
                        self.pass_to_instance::<AggregatorCommitteeConsensusData<E>>(
                            id,
                            WrappedQbftMessage {
                                signed_message: full_message,
                                qbft_message,
                            },
                        )
                    }
                    Some(Role::PTCCommittee) => {
                        // Route to PTC committee instances with fork gating
                        let slot = types::Slot::new(qbft_message.height);
                        let epoch = slot.epoch(E::slots_per_epoch());

                        // Fork gating: Reject before CStar
                        if self.fork_schedule.active_fork(epoch) < Fork::CStar {
                            warn!(%slot, "Ignoring PTCCommittee message before CStar fork");
                            return Err(QbftError::RoleNotActive);
                        }

                        let id = PTCCommitteeInstanceId {
                            committee,
                            instance_height,
                        };
                        self.pass_to_instance::<PayloadAttestationVote>(
                            id,
                            WrappedQbftMessage {
                                signed_message: full_message,
                                qbft_message,
                            },
                        )
                    }
                    // Validator roles should use DutyExecutor::Validator, not Committee
                    Some(Role::Aggregator | Role::Proposer | Role::SyncCommittee)
                    // These roles don't use QBFT consensus
                    | Some(Role::ValidatorRegistration | Role::VoluntaryExit)
                    | None => Err(QbftError::InconsistentMessageId),
                }
            }
            None => {
                warn!(?msg_id, "received invalid message id");
                Err(QbftError::InconsistentMessageId)
            }
        }
    }

    fn pass_to_instance<D: QbftDecidable<E>>(
        &self,
        id: D::Id,
        data: WrappedQbftMessage,
    ) -> Result<(), QbftError> {
        let sender = D::get_or_spawn_instance(self, id);
        self.processor.urgent_consensus.send_immediate(
            move |drop_on_finish: DropOnFinish| {
                let _ = sender.send(QbftMessage {
                    kind: QbftMessageKind::NetworkMessage(data),
                    drop_on_finish: Some(drop_on_finish),
                });
            },
            QBFT_MESSAGE_NAME,
        )?;
        Ok(())
    }

    // Long running cleaner that will remove instances that are no longer relevant
    async fn cleaner(self: Arc<Self>) {
        while !self.processor.permitless.is_closed() {
            sleep(
                self.slot_clock
                    .duration_to_next_slot()
                    .unwrap_or(self.slot_clock.slot_duration()),
            )
            .await;
            let Some(slot) = self.slot_clock.now() else {
                continue;
            };
            let cutoff = slot.saturating_sub(QBFT_RETAIN_SLOTS);
            self.beacon_vote_instances
                .retain(|k, _| *k.instance_height >= cutoff.as_usize());
            self.proposer_consensus_data_instances
                .retain(|k, _| *k.instance_height >= cutoff.as_usize());
            self.aggregator_committee_instances
                .retain(|k, _| *k.instance_height >= cutoff.as_usize());
            self.payload_attestation_vote_instances
                .retain(|k, _| *k.instance_height >= cutoff.as_usize());
        }
    }
}

/// Abstraction over QBFT consensus. Allows swapping in a mock for tests that don't
/// need real consensus (e.g., testing signing pipelines).
///
/// Uses static dispatch (not `dyn`) because `decide_instance` is generic over
/// `D: QbftDecidable<E>`, which prevents object safety.
pub trait ConsensusDecider<E: EthSpec>: Send + Sync {
    fn decide_instance<D: QbftDecidable<E>>(
        &self,
        id: D::Id,
        initial: D,
        validator: Box<dyn QbftDataValidator<D>>,
        timeout_mode: TimeoutMode,
        committee_members: &IndexSet<OperatorId>,
    ) -> impl Future<Output = Result<Completed<D>, QbftError>> + Send;
}

impl<E: EthSpec, S: SlotClock + 'static> ConsensusDecider<E> for QbftManager<E, S> {
    fn decide_instance<D: QbftDecidable<E>>(
        &self,
        id: D::Id,
        initial: D,
        validator: Box<dyn QbftDataValidator<D>>,
        timeout_mode: TimeoutMode,
        committee_members: &IndexSet<OperatorId>,
    ) -> impl Future<Output = Result<Completed<D>, QbftError>> + Send {
        self.decide_instance(id, initial, validator, timeout_mode, committee_members)
    }
}

// Trait that describes any data that is able to be decided upon during a qbft instance
pub trait QbftDecidable<E: EthSpec>: QbftData<Hash = Hash256> + Send + Sync + 'static {
    type Id: Hash + Eq + Send + Debug;

    fn get_map<S: SlotClock>(manager: &QbftManager<E, S>) -> &Map<Self::Id, Self>;

    fn get_or_spawn_instance<S: SlotClock>(
        manager: &QbftManager<E, S>,
        id: Self::Id,
    ) -> UnboundedSender<QbftMessage<Self>> {
        let map = Self::get_map(manager);
        match map.entry(id) {
            dashmap::Entry::Occupied(entry) => entry.get().clone(),
            dashmap::Entry::Vacant(entry) => {
                // There is not an instance running yet, store the sender and spawn a new instance
                // with the receiver
                let (tx, rx) = mpsc::unbounded_channel();
                let span = debug_span!("qbft_instance", instance_id = ?entry.key());
                let tx = entry.insert(tx);
                let _ = manager.processor.permitless.send_async(
                    Box::pin(qbft_instance(rx, manager.message_sender.clone()).instrument(span)),
                    QBFT_INSTANCE_NAME,
                );
                tx.clone()
            }
        }
    }

    fn instance_height(&self, id: &Self::Id) -> InstanceHeight;

    fn message_id(domain: &DomainType, id: &Self::Id) -> MessageId;
}

impl<E: EthSpec> QbftDecidable<E> for ProposerConsensusData {
    type Id = ProposerInstanceId;
    fn get_map<S: SlotClock>(manager: &QbftManager<E, S>) -> &Map<Self::Id, Self> {
        &manager.proposer_consensus_data_instances
    }

    fn instance_height(&self, id: &Self::Id) -> InstanceHeight {
        id.instance_height
    }

    fn message_id(domain: &DomainType, id: &Self::Id) -> MessageId {
        let role = match id.duty {
            ValidatorDutyKind::Proposal => Role::Proposer,
            ValidatorDutyKind::Aggregator => Role::Aggregator,
            ValidatorDutyKind::SyncCommitteeAggregator => Role::SyncCommittee,
        };
        MessageId::new(domain, role, &DutyExecutor::Validator(id.validator))
    }
}

impl<E: EthSpec> QbftDecidable<E> for BeaconVote {
    type Id = CommitteeInstanceId;
    fn get_map<S: SlotClock>(manager: &QbftManager<E, S>) -> &Map<Self::Id, Self> {
        &manager.beacon_vote_instances
    }

    fn instance_height(&self, id: &Self::Id) -> InstanceHeight {
        id.instance_height
    }

    fn message_id(domain: &DomainType, id: &Self::Id) -> MessageId {
        MessageId::new(
            domain,
            Role::Committee,
            &DutyExecutor::Committee(id.committee),
        )
    }
}

impl<E: EthSpec> QbftDecidable<E> for AggregatorCommitteeConsensusData<E> {
    type Id = AggregatorCommitteeInstanceId;
    fn get_map<S: SlotClock>(manager: &QbftManager<E, S>) -> &Map<Self::Id, Self> {
        &manager.aggregator_committee_instances
    }

    fn instance_height(&self, id: &Self::Id) -> InstanceHeight {
        id.instance_height
    }

    fn message_id(domain: &DomainType, id: &Self::Id) -> MessageId {
        MessageId::new(
            domain,
            Role::AggregatorCommittee,
            &DutyExecutor::Committee(id.committee),
        )
    }
}

impl<E: EthSpec> QbftDecidable<E> for PayloadAttestationVote {
    type Id = PTCCommitteeInstanceId;
    fn get_map<S: SlotClock>(manager: &QbftManager<E, S>) -> &Map<Self::Id, Self> {
        &manager.payload_attestation_vote_instances
    }

    fn instance_height(&self, id: &Self::Id) -> InstanceHeight {
        id.instance_height
    }

    fn message_id(domain: &DomainType, id: &Self::Id) -> MessageId {
        MessageId::new(
            domain,
            Role::PTCCommittee,
            &DutyExecutor::Committee(id.committee),
        )
    }
}

#[derive(Debug, Clone)]
pub enum QbftError {
    QueueClosedError,
    QueueFullError,
    ConfigBuilderError(ConfigBuilderError),
    InconsistentMessageId,
    OwnOperatorIdUnknown,
    RoleNotActive,
}

impl From<processor::Error> for QbftError {
    fn from(value: processor::Error) -> Self {
        match value {
            Queue(TrySendError::Full(_)) => QbftError::QueueFullError,
            Queue(TrySendError::Closed(_)) => QbftError::QueueClosedError,
        }
    }
}

impl From<RecvError> for QbftError {
    fn from(_: RecvError) -> Self {
        QbftError::QueueClosedError
    }
}

impl From<ConfigBuilderError> for QbftError {
    fn from(value: ConfigBuilderError) -> Self {
        QbftError::ConfigBuilderError(value)
    }
}
