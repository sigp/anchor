use std::sync::Arc;

use tokio::{
    select,
    sync::{mpsc, OwnedSemaphorePermit, Semaphore},
};

use crate::{work::WorkItem, QueueKind};

/// Result of retrieving the next work item from the queues
#[derive(Debug)]
pub struct ReceivedWork {
    /// The permit that was acquired (if any)
    pub permit: Option<OwnedSemaphorePermit>,
    /// The work item that was retrieved
    pub work_item: WorkItem,
    /// The queue from which the work item was retrieved
    pub queue: QueueKind,
}

impl ReceivedWork {
    pub fn with_permit(mut self, permit: OwnedSemaphorePermit) -> Self {
        self.permit = Some(permit);
        self
    }
}

#[derive(Debug)]
pub struct Receiver {
    pub rx: mpsc::Receiver<WorkItem>,
    pub queue: QueueKind,
}

impl Receiver {
    async fn recv(&mut self) -> Option<ReceivedWork> {
        self.rx.recv().await.map(|work_item| ReceivedWork {
            permit: None,
            work_item,
            queue: self.queue,
        })
    }
}

pub struct Receivers {
    pub permitless: Receiver,
    pub urgent_consensus: Receiver,
}

impl Receivers {
    /// Retrieves the next work item from the permitless queue, or acquires a permit and delegates
    /// to `next_work_item_with_permit`.
    ///
    /// Returns `None` if all queues are closed.
    pub async fn next_work_item(&mut self, semaphore: &Arc<Semaphore>) -> Option<ReceivedWork> {
        select! {
            biased;
            Some(work_item) = self.permitless.recv() => Some(work_item),
            Ok(permit) = semaphore.clone().acquire_owned() => {
                self.next_work_item_with_permit(permit).await
            },
            else => None,
        }
    }

    /// Retrieves the next work item from the queues, with appropriate priorities.
    ///
    /// Returns `None` if all queues are closed.
    pub async fn next_work_item_with_permit(
        &mut self,
        permit: OwnedSemaphorePermit,
    ) -> Option<ReceivedWork> {
        Some(select! {
            biased;
            Some(work_item) = self.urgent_consensus.recv() => work_item.with_permit(permit),

            // Also try permitless queue, to fall back if no permit work is incoming.
            Some(work_item) = self.permitless.recv() => work_item,
            else => return None,
        })
    }
}
