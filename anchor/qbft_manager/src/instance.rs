use std::sync::Arc;

use message_sender::MessageSender;
use qbft::{Completed, DefaultLeaderFunction, UnsignedWrappedQbftMessage, WrappedQbftMessage};
use ssv_types::{CommitteeId, consensus::QbftData};
use tokio::{
    select,
    sync::{
        mpsc,
        mpsc::{UnboundedReceiver, UnboundedSender},
        oneshot,
    },
    time::Interval,
};
use tracing::{debug, error, trace, warn};
use types::Hash256;

use crate::{QbftInitialization, QbftMessage, QbftMessageKind};
type Qbft<D> = qbft::Qbft<DefaultLeaderFunction, D, MessageCallback>;

/// Maximum number of messages that are buffered before messages are dropped.
///
/// In a single round where we do not participate, we can have roughly up to (O - 1) * 2 + 1
/// messages, where O is the number of operators in the committee. With O = 13, we get 25, so this
/// limit should be generous enough.
const MESSAGE_BUFFER_LIMIT: usize = 100;

// States that Qbft instance may be in
enum QbftInstance<D: QbftData<Hash = Hash256>> {
    // The instance is uninitialized
    Uninitialized(Uninitialized),
    // The instance is initialized
    Initialized(Initialized<D>),
    // The instance has been decided
    Decided(Decided<D>),
}

#[derive(Default)]
struct Uninitialized {
    /// A buffer of messages that were sent into the system before the instance has been
    /// initialized. Will be filled up to [`MESSAGE_BUFFER_LIMIT`] messages, after which messages
    /// are dropped.
    message_buffer: Vec<WrappedQbftMessage>,
}

struct Initialized<D: QbftData<Hash = Hash256>> {
    qbft: Box<Qbft<D>>,
    round_end: Interval,
    msgs_sent_by_us: UnboundedReceiver<WrappedQbftMessage>,
    on_completed: Vec<oneshot::Sender<Completed<D>>>,
}

struct Decided<D: QbftData<Hash = Hash256>> {
    value: Completed<D>,
}

impl<D: QbftData<Hash = Hash256>> QbftInstance<D> {
    async fn initialize(
        mut self,
        init: QbftInitialization<D>,
        sender: &Arc<dyn MessageSender>,
    ) -> Self {
        match self {
            // The instance is uninitialized and we have received a manager message to
            // initialize it
            QbftInstance::Uninitialized(uninitialized) => {
                QbftInstance::Initialized(uninitialized.initialize(init, sender).await)
            }
            QbftInstance::Initialized(ref mut initialized) => {
                if initialized.qbft.start_data_hash() != &init.initial.hash() {
                    warn!("got conflicting double initialization of qbft instance");
                }
                initialized.on_completed.push(init.on_completed);
                self
            }
            // The instance has already been decided! Send the result to the callback.
            QbftInstance::Decided(Decided { ref value }) => {
                if init.on_completed.send(value.clone()).is_err() {
                    warn!("Callback dropped - qbft result is no longer relevant");
                }
                self
            }
        }
    }

    fn receive(&mut self, message: WrappedQbftMessage) {
        match self {
            QbftInstance::Initialized(initialized) => {
                // If the instance is already initialized, receive it in the instance
                // right away
                initialized.qbft.receive(message);
            }
            QbftInstance::Uninitialized(uninitialized) => {
                // The instance has not been initialized yet, save it in the buffer to
                // be received
                if uninitialized.message_buffer.len() < MESSAGE_BUFFER_LIMIT {
                    uninitialized.message_buffer.push(message);
                } else {
                    warn!("QBFT message buffer full, dropping message");
                }
            }
            QbftInstance::Decided { .. } => {
                // message no longer relevant
            }
        }
    }
}

impl Uninitialized {
    async fn initialize<D: QbftData<Hash = Hash256>>(
        self,
        init: QbftInitialization<D>,
        sender: &Arc<dyn MessageSender>,
    ) -> Initialized<D> {
        // Create the interval and tick it right away to wait until the start time if necessary.
        let mut interval = tokio::time::interval_at(init.start_time, init.config.round_time());
        interval.tick().await;

        let (sent_by_us_tx, sent_by_us_rx) = mpsc::unbounded_channel();

        let sender = sender.clone();
        let committee_id = init
            .config
            .committee_members()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .into();
        // Create a new instance and receive any buffered messages
        let mut instance = Box::new(Qbft::new(
            init.config,
            init.initial,
            init.message_id,
            MessageCallback {
                sent_by_us_tx,
                committee_id,
                sender,
            },
        ));
        if !self.message_buffer.is_empty() {
            debug!(
                len = self.message_buffer.len(),
                "Replaying buffered messages"
            );
            for message in self.message_buffer {
                instance.receive(message);
            }
        }

        Initialized {
            round_end: interval,
            qbft: instance,
            msgs_sent_by_us: sent_by_us_rx,
            on_completed: vec![init.on_completed],
        }
    }
}

enum RecvResult<D: QbftData> {
    Message(Box<QbftMessage<D>>),
    RoundEnd,
    Closed,
}

impl<D: QbftData> From<Option<QbftMessage<D>>> for RecvResult<D> {
    fn from(value: Option<QbftMessage<D>>) -> Self {
        match value {
            None => RecvResult::Closed,
            Some(msg) => RecvResult::Message(Box::new(msg)),
        }
    }
}

impl<D: QbftData<Hash = Hash256>> Initialized<D> {
    async fn recv(&mut self, rx: &mut UnboundedReceiver<QbftMessage<D>>) -> RecvResult<D> {
        select! {
            message = rx.recv() => message.into(),
            sent_by_us = self.msgs_sent_by_us.recv() => {
                sent_by_us.map(|msg| QbftMessage {
                    kind: QbftMessageKind::NetworkMessage(msg),
                    drop_on_finish: None
                }).into()
            },
            _ = self.round_end.tick() => RecvResult::RoundEnd,
        }
    }

    fn complete(self, value: Completed<D>) {
        for on_completed in self.on_completed {
            if on_completed.send(value.clone()).is_err() {
                error!("could not send qbft result");
            }
        }
    }

    fn complete_if_done(self, message_sender: &Arc<dyn MessageSender>) -> QbftInstance<D> {
        if let Some(completed) = self.qbft.completed() {
            for on_completed in self.on_completed {
                if on_completed.send(completed.clone()).is_err() {
                    error!("could not send qbft result");
                }
            }

            // Send the decided message (aggregated commit)
            match self.qbft.get_aggregated_commit() {
                Some(msg) => {
                    let committee_id = self
                        .qbft
                        .config()
                        .committee_members()
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .into();

                    if let Err(err) = message_sender.send(msg, committee_id) {
                        error!(?err, "Unable to send aggregated commit message");
                    }
                }
                None => {
                    if let Completed::Success(_) = completed {
                        error!("Aggregated commit does not exist");
                    }
                }
            }

            trace!(?completed, "Completed");

            QbftInstance::Decided(Decided { value: completed })
        } else {
            QbftInstance::Initialized(self)
        }
    }
}

pub async fn qbft_instance<D: QbftData<Hash = Hash256>>(
    mut rx: UnboundedReceiver<QbftMessage<D>>,
    message_sender: Arc<dyn MessageSender>,
) {
    // Signal a new instance that is uninitialized
    let mut instance = QbftInstance::Uninitialized(Uninitialized::default());

    loop {
        // Receive a new message for this instance
        let recv_result = match &mut instance {
            QbftInstance::Uninitialized(_) | QbftInstance::Decided(_) => rx.recv().await.into(),
            QbftInstance::Initialized(initialized) => initialized.recv(&mut rx).await,
        };

        // Handle message, round end, or closed queue. Keep the drop guard if we have one.
        let guard = match recv_result {
            RecvResult::Message(msg) => {
                debug!(msg = ?msg.kind, "Handling message in qbft_instance");
                match msg.kind {
                    QbftMessageKind::Initialize(initialization) => {
                        instance = instance.initialize(initialization, &message_sender).await;
                    }
                    // We got a new network message, this should be passed onto the instance
                    QbftMessageKind::NetworkMessage(message) => {
                        instance.receive(message);
                    }
                }
                msg.drop_on_finish
            }
            RecvResult::RoundEnd => {
                // There is nothing to do on round end if the instance is not ongoing.
                if let QbftInstance::Initialized(initialized) = &mut instance {
                    warn!("Round timer elapsed");
                    initialized.qbft.end_round();
                };
                None
            }
            RecvResult::Closed => {
                // If the instance can receive no more messages, we no longer need it. Signal
                // time out to listeners, as this instance was likely cleaned up.
                if let QbftInstance::Initialized(initialized) = instance {
                    initialized.complete(Completed::TimedOut);
                }
                break;
            }
        };

        // If the instance is ongoing, check whether it is done.
        if let QbftInstance::Initialized(initialized) = instance {
            instance = initialized.complete_if_done(&message_sender);
        }

        // Drop guard as late as possible to keep the processor permit.
        drop(guard);
    }
}

struct MessageCallback {
    sent_by_us_tx: UnboundedSender<WrappedQbftMessage>,
    sender: Arc<dyn MessageSender>,
    committee_id: CommitteeId,
}

impl qbft::MessageSender for MessageCallback {
    fn send(&mut self, msg: UnsignedWrappedQbftMessage) {
        let sent_by_us_tx = self.sent_by_us_tx.clone();
        if let Err(err) = self.sender.clone().sign_and_send(
            msg.unsigned_message,
            self.committee_id,
            Some(Box::new(move |signed| {
                // this might fail, but that's ok: it simply means that the
                // instance has shut down (e.g. because it's done)
                let _ = sent_by_us_tx.send(WrappedQbftMessage {
                    signed_message: signed.clone(),
                    qbft_message: msg.qbft_message,
                });
            })),
        ) {
            error!(?err, "Unable to send qbft message!");
        }
    }
}
