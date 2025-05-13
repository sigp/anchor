use std::{fmt::Debug, hash::Hash, sync::Arc};

use dashmap::DashMap;
use message_sender::MessageSender;
use processor::{Error::Queue, Senders, work::DropOnFinish};
use qbft::{
    Completed, ConfigBuilder, ConfigBuilderError, DefaultLeaderFunction, InstanceHeight,
    WrappedQbftMessage,
};
use slot_clock::SlotClock;
use ssv_types::{
    Cluster, CommitteeId, OperatorId as QbftOperatorId, OperatorId,
    consensus::{BeaconVote, QbftData, ValidatorConsensusData},
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
use tracing::{Instrument, debug, error, info_span, warn};
use types::{Hash256, PublicKeyBytes};

use crate::instance::qbft_instance;

mod instance;
#[cfg(test)]
mod tests;

const QBFT_INSTANCE_NAME: &str = "qbft_instance";
const QBFT_MESSAGE_NAME: &str = "qbft_message";
const QBFT_CLEANER_NAME: &str = "qbft_cleaner";

/// Number of slots to keep before the current slot
const QBFT_RETAIN_SLOTS: u64 = 1;

// Unique Identifier for a committee and its corresponding QBFT instance
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CommitteeInstanceId {
    pub committee: CommitteeId,
    pub instance_height: InstanceHeight,
}

// Unique Identifier for a validator instance
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ValidatorInstanceId {
    pub validator: PublicKeyBytes,
    pub duty: ValidatorDutyKind,
    pub instance_height: InstanceHeight,
}

// Type of validator duty that is being voted one
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum ValidatorDutyKind {
    Proposal,
    Aggregator,
    SyncCommitteeAggregator,
}

// Message that is passed around the QbftManager
#[derive(Debug)]
pub struct QbftMessage<D: QbftData> {
    pub kind: QbftMessageKind<D>,
    pub drop_on_finish: Option<DropOnFinish>,
}

// Type of the QBFT Message
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // clippy is confused and thinks the first variant is 0 bytes
pub enum QbftMessageKind<D: QbftData> {
    // Initialize a new qbft instance with some initial data,
    // the configuration for the instance, and a channel to send the final data on
    Initialize(QbftInitialization<D>),
    // A message received from the network. The network exchanges SignedSsvMessages, but after
    // deserialziation we dermine the message is for the qbft instance and decode it into a
    // wrapped qbft messsage consisting of the signed message and the qbft message
    NetworkMessage(WrappedQbftMessage),
}

/// Represents the initialization data required to start a new QBFT instance.
#[derive(Debug)]
pub struct QbftInitialization<D: QbftData> {
    /// The data to use when we are the leader.
    initial: D,
    /// The message id to be embedded into outgoing messages.
    message_id: MessageId,
    /// The time when the first round is supposed to start. Rounds will be advanced based on this.
    start_time: Instant,
    /// The configuration for the instance.
    config: qbft::Config<DefaultLeaderFunction>,
    /// The channel to send the final result to.
    on_completed: oneshot::Sender<Completed<D>>,
}

// Map from an identifier to a sender for the instance
type Map<I, D> = DashMap<I, UnboundedSender<QbftMessage<D>>>;

// Top level QBFTManager structure
pub struct QbftManager {
    // Senders to send work off to the central processor
    processor: Senders,
    // OperatorID
    operator_id: QbftOperatorId,
    // All of the QBFT instances that are voting on validator consensus data
    validator_consensus_data_instances: Map<ValidatorInstanceId, ValidatorConsensusData>,
    // All of the QBFT instances that are voting on beacon data
    beacon_vote_instances: Map<CommitteeInstanceId, BeaconVote>,
    // Utility to sign and serialize network messages
    message_sender: Arc<dyn MessageSender>,
    // Network domain to embed into messages
    domain: DomainType,
}

impl QbftManager {
    // Construct a new QBFT Manager
    pub fn new(
        processor: Senders,
        operator_id: OperatorId,
        slot_clock: impl SlotClock + 'static,
        message_sender: Arc<dyn MessageSender>,
        domain: DomainType,
    ) -> Result<Arc<Self>, QbftError> {
        let manager = Arc::new(QbftManager {
            processor,
            operator_id,
            validator_consensus_data_instances: DashMap::new(),
            beacon_vote_instances: DashMap::new(),
            message_sender,
            domain,
        });

        // Start a long running task that will clean up old instances
        manager
            .processor
            .permitless
            .send_async(Arc::clone(&manager).cleaner(slot_clock), QBFT_CLEANER_NAME)?;

        Ok(manager)
    }

    // Decide a brand new qbft instance
    pub async fn decide_instance<D: QbftDecidable>(
        &self,
        id: D::Id,
        initial: D,
        start_time: Instant,
        committee: &Cluster,
    ) -> Result<Completed<D>, QbftError> {
        // Tx/Rx pair to send and retrieve the final result
        let (result_sender, result_receiver) = oneshot::channel();
        let message_id = D::message_id(&self.domain, &id);

        // General the qbft configuration
        let config = ConfigBuilder::new(
            self.operator_id,
            initial.instance_height(&id),
            committee.cluster_members.iter().copied().collect(),
        );
        let config = config
            .with_quorum_size(committee.cluster_members.len() - committee.get_f() as usize)
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
                        message_id,
                        start_time,
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

        debug!(?msg_id, ?instance_height, "Received valid qbft message");

        match msg_id.duty_executor() {
            Some(DutyExecutor::Validator(validator)) => {
                let duty = match msg_id.role() {
                    Some(Role::Proposer) => ValidatorDutyKind::Proposal,
                    Some(Role::Aggregator) => ValidatorDutyKind::Aggregator,
                    Some(Role::SyncCommittee) => ValidatorDutyKind::SyncCommitteeAggregator,
                    _ => {
                        // should never happen
                        error!(?msg_id, "Unexpected role/executor combination in msg id");
                        return Err(QbftError::InconsistentMessageId);
                    }
                };
                let id = ValidatorInstanceId {
                    validator,
                    duty,
                    instance_height,
                };
                self.pass_to_instance::<ValidatorConsensusData>(
                    id,
                    WrappedQbftMessage {
                        signed_message: full_message,
                        qbft_message,
                    },
                )
            }
            Some(DutyExecutor::Committee(committee)) => {
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
            None => {
                warn!(?msg_id, "received invalid message id");
                Err(QbftError::InconsistentMessageId)
            }
        }
    }

    fn pass_to_instance<D: QbftDecidable>(
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
    async fn cleaner(self: Arc<Self>, slot_clock: impl SlotClock) {
        while !self.processor.permitless.is_closed() {
            sleep(
                slot_clock
                    .duration_to_next_slot()
                    .unwrap_or(slot_clock.slot_duration()),
            )
            .await;
            let Some(slot) = slot_clock.now() else {
                continue;
            };
            let cutoff = slot.saturating_sub(QBFT_RETAIN_SLOTS);
            self.beacon_vote_instances
                .retain(|k, _| *k.instance_height >= cutoff.as_usize());
            self.validator_consensus_data_instances
                .retain(|k, _| *k.instance_height >= cutoff.as_usize());
        }
    }
}

// Trait that describes any data that is able to be decided upon during a qbft instance
pub trait QbftDecidable: QbftData<Hash = Hash256> + Send + Sync + 'static {
    type Id: Hash + Eq + Send + Debug;

    fn get_map(manager: &QbftManager) -> &Map<Self::Id, Self>;

    fn get_or_spawn_instance(
        manager: &QbftManager,
        id: Self::Id,
    ) -> UnboundedSender<QbftMessage<Self>> {
        let map = Self::get_map(manager);
        let ret = match map.entry(id) {
            dashmap::Entry::Occupied(entry) => entry.get().clone(),
            dashmap::Entry::Vacant(entry) => {
                // There is not an instance running yet, store the sender and spawn a new instance
                // with the reeiver
                let (tx, rx) = mpsc::unbounded_channel();
                let span = info_span!("qbft_instance", instance_id = ?entry.key());
                let tx = entry.insert(tx);
                let _ = manager.processor.permitless.send_async(
                    Box::pin(qbft_instance(rx, manager.message_sender.clone()).instrument(span)),
                    QBFT_INSTANCE_NAME,
                );
                tx.clone()
            }
        };
        ret
    }

    fn instance_height(&self, id: &Self::Id) -> InstanceHeight;

    fn message_id(domain: &DomainType, id: &Self::Id) -> MessageId;
}

impl QbftDecidable for ValidatorConsensusData {
    type Id = ValidatorInstanceId;
    fn get_map(manager: &QbftManager) -> &Map<Self::Id, Self> {
        &manager.validator_consensus_data_instances
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

impl QbftDecidable for BeaconVote {
    type Id = CommitteeInstanceId;
    fn get_map(manager: &QbftManager) -> &Map<Self::Id, Self> {
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

#[derive(Debug, Clone)]
pub enum QbftError {
    QueueClosedError,
    QueueFullError,
    ConfigBuilderError(ConfigBuilderError),
    InconsistentMessageId,
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
