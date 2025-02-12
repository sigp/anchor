use dashmap::DashMap;
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Private};
use openssl::rsa::Rsa;
use openssl::sign::Signer;

use processor::{DropOnFinish, Senders, WorkItem};
use qbft::{
    Completed, ConfigBuilder, ConfigBuilderError, DefaultLeaderFunction, InstanceHeight, Message,
    WrappedQbftMessage,
};
use slot_clock::SlotClock;
use ssv_types::consensus::{BeaconVote, QbftData, UnsignedSSVMessage, ValidatorConsensusData};
use std::error::Error;

use ssv_types::message::SignedSSVMessage;
use ssv_types::OperatorId as QbftOperatorId;
use ssv_types::{Cluster, ClusterId, OperatorId};
use ssz::Encode;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;
use tokio::select;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot::error::RecvError;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep, Instant, Interval};
use tracing::{error, warn};
use types::{Hash256, PublicKeyBytes};

#[cfg(test)]
mod tests;

const QBFT_INSTANCE_NAME: &str = "qbft_instance";
const QBFT_MESSAGE_NAME: &str = "qbft_message";
const QBFT_CLEANER_NAME: &str = "qbft_cleaner";
const QBFT_SIGNER_NAME: &str = "qbft_signer";

/// Number of slots to keep before the current slot
const QBFT_RETAIN_SLOTS: u64 = 1;

// Unique Identifier for a Cluster and its corresponding QBFT instance
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CommitteeInstanceId {
    pub committee: ClusterId,
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
pub struct QbftMessage<D: QbftData<Hash = Hash256>> {
    pub kind: QbftMessageKind<D>,
    pub drop_on_finish: DropOnFinish,
}

// Type of the QBFT Message
#[derive(Debug)]
pub enum QbftMessageKind<D: QbftData<Hash = Hash256>> {
    // Initialize a new qbft instance with some initial data,
    // the configuration for the instance, and a channel to send the final data on
    Initialize {
        initial: D,
        config: qbft::Config<DefaultLeaderFunction>,
        on_completed: oneshot::Sender<Completed<D>>,
    },
    // A message received from the network. The network exchanges SignedSsvMessages, but after
    // deserialziation we dermine the message is for the qbft instance and decode it into a wrapped
    // qbft messsage consisting of the signed message and the qbft message
    NetworkMessage(WrappedQbftMessage),
}

type Qbft<D, S> = qbft::Qbft<DefaultLeaderFunction, D, S>;

// Map from an identifier to a sender for the instance
type Map<I, D> = DashMap<I, UnboundedSender<QbftMessage<D>>>;

// Top level QBFTManager structure
pub struct QbftManager<T: SlotClock + 'static> {
    // Senders to send work off to the central processor
    processor: Senders,
    // OperatorID
    operator_id: QbftOperatorId,
    // The slot clock for timing
    slot_clock: T,
    // All of the QBFT instances that are voting on validator consensus data
    validator_consensus_data_instances: Map<ValidatorInstanceId, ValidatorConsensusData>,
    // All of the QBFT instances that are voting on beacon data
    beacon_vote_instances: Map<CommitteeInstanceId, BeaconVote>,
    // Private key used for signing messages
    pkey: Arc<PKey<Private>>,
    // Channel to pass signed messages along to the network
    network_tx: mpsc::UnboundedSender<SignedSSVMessage>,
}

impl<T: SlotClock> QbftManager<T> {
    // Construct a new QBFT Manager
    pub fn new(
        processor: Senders,
        operator_id: OperatorId,
        slot_clock: T,
        key: Rsa<Private>,
        network_tx: mpsc::UnboundedSender<SignedSSVMessage>,
    ) -> Result<Arc<Self>, QbftError> {
        let pkey = Arc::new(PKey::from_rsa(key).expect("Failed to create PKey from RSA"));

        let manager = Arc::new(QbftManager {
            processor,
            operator_id,
            slot_clock,
            validator_consensus_data_instances: DashMap::new(),
            beacon_vote_instances: DashMap::new(),
            pkey,
            network_tx,
        });

        // Start a long running task that will clean up old instances
        manager
            .processor
            .permitless
            .send_async(Arc::clone(&manager).cleaner(), QBFT_CLEANER_NAME)?;

        Ok(manager)
    }

    // Decide a brand new qbft instance
    pub async fn decide_instance<D: QbftDecidable<T>>(
        &self,
        id: D::Id,
        initial: D,
        committee: &Cluster,
    ) -> Result<Completed<D>, QbftError> {
        // Tx/Rx pair to send and retrieve the final result
        let (result_sender, result_receiver) = oneshot::channel();

        // General the qbft configuration
        let config = ConfigBuilder::new(
            self.operator_id,
            initial.instance_height(&id),
            committee.cluster_members.iter().copied().collect(),
        );
        let config = config
            .with_quorum_size(committee.cluster_members.len() - committee.get_f() as usize)
            .build()?;

        // Get or spawn a new qbft instance. This will return the sender that we can use to send
        // new messages to the specific instance
        let sender = D::get_or_spawn_instance(self, id);
        self.processor.urgent_consensus.send_immediate(
            move |drop_on_finish: DropOnFinish| {
                // A message to initialize this instance
                let _ = sender.send(QbftMessage {
                    kind: QbftMessageKind::Initialize {
                        initial,
                        config,
                        on_completed: result_sender,
                    },
                    drop_on_finish,
                });
            },
            QBFT_MESSAGE_NAME,
        )?;

        // Await the final result
        Ok(result_receiver.await?)
    }

    /// Send a new network message to the instance
    pub fn receive_data<D: QbftDecidable<T>>(
        &self,
        id: D::Id,
        data: WrappedQbftMessage,
    ) -> Result<(), QbftError> {
        let sender = D::get_or_spawn_instance(self, id);
        self.processor.urgent_consensus.send_immediate(
            move |drop_on_finish: DropOnFinish| {
                let _ = sender.send(QbftMessage {
                    kind: QbftMessageKind::NetworkMessage(data),
                    drop_on_finish,
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
                .retain(|k, _| *k.instance_height >= cutoff.as_usize())
        }
    }
}

// Trait that describes any data that is able to be decided upon during a qbft instance
pub trait QbftDecidable<T: SlotClock + 'static>: QbftData<Hash = Hash256> + Send + 'static {
    type Id: Hash + Eq + Send + Clone;

    fn get_map(manager: &QbftManager<T>) -> &Map<Self::Id, Self>;

    fn get_or_spawn_instance(
        manager: &QbftManager<T>,
        id: Self::Id,
    ) -> UnboundedSender<QbftMessage<Self>> {
        let map = Self::get_map(manager);
        let ret = match map.entry(id) {
            dashmap::Entry::Occupied(entry) => entry.get().clone(),
            dashmap::Entry::Vacant(entry) => {
                // There is not an instance running yet, store the sender and spawn a new instance
                // with the reeiver
                let (tx, rx) = mpsc::unbounded_channel();
                let tx = entry.insert(tx);
                let _ = manager.processor.permitless.send_async(
                    Box::pin(qbft_instance(
                        rx,
                        manager.network_tx.clone(),
                        manager.pkey.clone(),
                        manager.processor.clone(),
                    )),
                    QBFT_INSTANCE_NAME,
                );
                tx.clone()
            }
        };
        ret
    }

    fn instance_height(&self, id: &Self::Id) -> InstanceHeight;
}

impl<T: SlotClock + 'static> QbftDecidable<T> for ValidatorConsensusData {
    type Id = ValidatorInstanceId;
    fn get_map(manager: &QbftManager<T>) -> &Map<Self::Id, Self> {
        &manager.validator_consensus_data_instances
    }

    fn instance_height(&self, id: &Self::Id) -> InstanceHeight {
        id.instance_height
    }
}

impl<T: SlotClock + 'static> QbftDecidable<T> for BeaconVote {
    type Id = CommitteeInstanceId;
    fn get_map(manager: &QbftManager<T>) -> &Map<Self::Id, Self> {
        &manager.beacon_vote_instances
    }

    fn instance_height(&self, id: &Self::Id) -> InstanceHeight {
        id.instance_height
    }
}

// States that Qbft instance may be in
enum QbftInstance<D: QbftData<Hash = Hash256>, S: FnMut(Message)> {
    // The instance is uninitialized
    Uninitialized {
        // todo: proooobably limit this
        // A buffer of message that are being send into the system. Qbft instace RECEIVES
        // WrappedQBFTMessage, but sends out Message
        message_buffer: Vec<WrappedQbftMessage>,
    },
    // The instance is initialized
    Initialized {
        qbft: Box<Qbft<D, S>>,
        round_end: Interval,
        on_completed: Vec<oneshot::Sender<Completed<D>>>,
    },
    // The instance has been decided
    Decided {
        value: Completed<D>,
    },
}

async fn qbft_instance<D: QbftData<Hash = Hash256>>(
    mut rx: UnboundedReceiver<QbftMessage<D>>,
    network_tx: mpsc::UnboundedSender<SignedSSVMessage>,
    pkey: Arc<PKey<Private>>,
    processor: Senders,
) {
    // Signal a new instance that is uninitialized
    let mut instance = QbftInstance::Uninitialized {
        message_buffer: Vec::new(),
    };

    loop {
        // receive a new message for this instance
        let message = match &mut instance {
            QbftInstance::Uninitialized { .. } | QbftInstance::Decided { .. } => rx.recv().await,
            QbftInstance::Initialized {
                qbft: instance,
                round_end,
                ..
            } => {
                select! {
                    message = rx.recv() => message,
                    _ = round_end.tick() => {
                        warn!("Round timer elapsed");
                        instance.end_round();
                        continue;
                    }
                }
            }
        };

        let Some(message) = message else {
            break;
        };

        match message.kind {
            QbftMessageKind::Initialize {
                initial,
                config,
                on_completed,
            } => {
                instance = match instance {
                    // The instance is uninitialized and we have received a manager message to
                    // initialize it
                    QbftInstance::Uninitialized { message_buffer } => {
                        // Create a new instance and receive any buffered messages
                        let mut instance = Box::new(Qbft::new(config, initial, |message| {
                            let (id, unsigned) = message.desugar();
                            let serialized = unsigned.as_ssz_bytes();
                            let pkey = pkey.clone();
                            let network_tx = network_tx.clone();

                            processor
                                .urgent_consensus
                                .send_blocking(
                                    move || {
                                        if let Err(e) = sign_and_send_message(
                                            pkey, id, unsigned, serialized, network_tx,
                                        ) {
                                            error!("Signing failed: {}", e);
                                        }
                                    },
                                    QBFT_SIGNER_NAME,
                                )
                                .unwrap_or_else(|e| warn!("Failed to send to processor: {}", e));
                        }));
                        for message in message_buffer {
                            instance.receive(message);
                        }
                        QbftInstance::Initialized {
                            // Ensure we do not tick right away
                            round_end: tokio::time::interval_at(
                                Instant::now() + instance.config().round_time(),
                                instance.config().round_time(),
                            ),
                            qbft: instance,
                            on_completed: vec![on_completed],
                        }
                    }
                    QbftInstance::Initialized {
                        qbft,
                        round_end,
                        on_completed: mut on_completed_vec,
                    } => {
                        if qbft.start_data_hash() != &initial.hash() {
                            warn!("got conflicting double initialization of qbft instance");
                        }
                        on_completed_vec.push(on_completed);
                        QbftInstance::Initialized {
                            qbft,
                            round_end,
                            on_completed: on_completed_vec,
                        }
                    }
                    // The instance has been decided! Send off the final result and mark the
                    // instance state as decided
                    QbftInstance::Decided { value } => {
                        if on_completed.send(value.clone()).is_err() {
                            error!("could not send qbft result");
                        }
                        QbftInstance::Decided { value }
                    }
                }
            }
            // We got a new network message, this should be passed onto the instance
            QbftMessageKind::NetworkMessage(message) => match &mut instance {
                QbftInstance::Initialized { qbft: instance, .. } => {
                    // If the instance is already initialized, receive it in the instance right away
                    instance.receive(message);
                }
                QbftInstance::Uninitialized { message_buffer } => {
                    // The instance has not been initialized yet, save it in the buffer to be
                    // received
                    message_buffer.push(message);
                }
                QbftInstance::Decided { .. } => {
                    // no longer relevant
                }
            },
        }

        if let QbftInstance::Initialized {
            qbft,
            round_end,
            on_completed,
        } = instance
        {
            if let Some(completed) = qbft.completed() {
                for on_completed in on_completed {
                    if on_completed.send(completed.clone()).is_err() {
                        error!("could not send qbft result");
                    }
                }
                instance = QbftInstance::Decided { value: completed };
            } else {
                instance = QbftInstance::Initialized {
                    qbft,
                    round_end,
                    on_completed,
                }
            }
        }
    }
}

// Sign a message and send it to the network via the network_tx
fn sign_and_send_message(
    pkey: Arc<PKey<Private>>,
    id: OperatorId,
    unsigned: UnsignedSSVMessage,
    serialized: Vec<u8>,
    network_tx: UnboundedSender<SignedSSVMessage>,
) -> Result<(), Box<dyn Error>> {
    // Create the signature
    let mut signer = Signer::new(MessageDigest::sha256(), &pkey)?;
    signer.update(&serialized)?;
    let sig = signer.sign_to_vec()?;

    // Build the signed ssv message, then serialize it and send to the network
    let signed = SignedSSVMessage::new(
        vec![sig],
        vec![*id],
        unsigned.ssv_message,
        unsigned.full_data,
    )?;
    network_tx
        .send(signed)
        .map_err(|e| format!("Failed to send signed ssv message to network: {}", e))?;

    Ok(())
}

#[derive(Debug, Clone)]
pub enum QbftError {
    QueueClosedError,
    QueueFullError,
    ConfigBuilderError(ConfigBuilderError),
}

impl From<TrySendError<WorkItem>> for QbftError {
    fn from(value: TrySendError<WorkItem>) -> Self {
        match value {
            TrySendError::Full(_) => QbftError::QueueFullError,
            TrySendError::Closed(_) => QbftError::QueueClosedError,
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
