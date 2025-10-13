use std::future::Future;

use tokio::sync::{mpsc, mpsc::error::TrySendError};
use tracing::{error, warn};

use crate::{
    Error, QueueKind, metrics,
    work::{DropOnFinish, WorkItem},
};

/// Bag of available senders relevant for the Anchor client.
#[derive(Clone, Debug)]
pub struct Senders {
    /// Catch-all queue for tasks that are either very quick to run or behave well as async task in
    /// the Tokio runtime. Is launched immediately and does not require capacity as defined by
    /// [`Config::max_workers`].
    pub permitless: Sender,
    pub urgent_consensus: Sender,
}

impl Senders {
    pub fn get(&self, queue: QueueKind) -> &Sender {
        match queue {
            QueueKind::Permitless => &self.permitless,
            QueueKind::UrgentConsensus => &self.urgent_consensus,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Sender {
    pub(crate) tx: mpsc::Sender<WorkItem>,
    pub(crate) queue: QueueKind,
}

impl Sender {
    /// Convenience method creating an async [`WorkItem`] and sending it.
    pub fn send_async<F: Future<Output = ()> + Send + 'static>(
        &self,
        future: F,
        name: &'static str,
    ) -> Result<(), Error> {
        Ok(self.send_work_item(WorkItem::new_async(name, Box::pin(future)))?)
    }

    /// Convenience method creating a blocking [`WorkItem`] and sending it.
    pub fn send_blocking<F: FnOnce() + Send + 'static>(
        &self,
        func: F,
        name: &'static str,
    ) -> Result<(), Error> {
        Ok(self.send_work_item(WorkItem::new_blocking(name, Box::new(func)))?)
    }

    /// Convenience method creating an immediate [`WorkItem`] and sending it.
    pub fn send_immediate<F: FnOnce(DropOnFinish) + Send + 'static>(
        &self,
        func: F,
        name: &'static str,
    ) -> Result<(), Error> {
        Ok(self.send_work_item(WorkItem::new_immediate(name, Box::new(func)))?)
    }

    /// Sends a [`WorkItem`] into the queue, non-blocking, returning an error if the queue is full.
    /// Handles metrics and logging for you.
    pub fn send_work_item(&self, item: WorkItem) -> Result<(), TrySendError<WorkItem>> {
        let name = item.name();
        let result = self.tx.try_send(item);
        if let Err(err) = &result {
            metrics::inc_counter_vec(
                &metrics::ANCHOR_PROCESSOR_SEND_ERROR_PER_WORK_TYPE,
                &[name, self.queue.name()],
            );
            match err {
                TrySendError::Full(_) => {
                    warn!(
                        task = name,
                        queue = self.queue.name(),
                        "Processor queue full"
                    )
                }
                TrySendError::Closed(_) => {
                    error!(
                        queue = self.queue.name(),
                        "Processor queue closed unexpectedly"
                    )
                }
            }
        } else {
            metrics::inc_counter_vec(
                &metrics::ANCHOR_PROCESSOR_WORK_EVENTS_SUBMITTED_COUNT,
                &[name, self.queue.name()],
            );
            metrics::inc_gauge_vec(
                &metrics::ANCHOR_PROCESSOR_QUEUE_LENGTH,
                &[name, self.queue.name()],
            );
        }
        result
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}
