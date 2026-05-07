use std::sync::Arc;

use tokio::{
    select,
    sync::{OwnedSemaphorePermit, Semaphore, mpsc},
};

use crate::{QueueKind, work::WorkItem};

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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::{Semaphore, mpsc};

    use super::{Receiver, Receivers};
    use crate::{QueueKind, work::WorkItem};

    const PERMITLESS_WORK_NAME: &str = "permitless_test_work";
    const URGENT_WORK_NAME: &str = "urgent_consensus_test_work";

    #[tokio::test]
    async fn test_next_work_item_prioritizes_urgent_consensus_over_ready_permitless() {
        // Arrange: both queues are ready, and urgent consensus has worker capacity available.
        let (permitless_tx, permitless_rx) = mpsc::channel(1);
        let (urgent_consensus_tx, urgent_consensus_rx) = mpsc::channel(1);
        let mut receivers = Receivers {
            permitless: Receiver {
                rx: permitless_rx,
                queue: QueueKind::Permitless,
            },
            urgent_consensus: Receiver {
                rx: urgent_consensus_rx,
                queue: QueueKind::UrgentConsensus,
            },
        };
        let semaphore = Arc::new(Semaphore::new(1));

        permitless_tx
            .send(WorkItem::new_immediate(PERMITLESS_WORK_NAME, |_| {}))
            .await
            .expect("permitless receiver should be open");
        urgent_consensus_tx
            .send(WorkItem::new_immediate(URGENT_WORK_NAME, |_| {}))
            .await
            .expect("urgent consensus receiver should be open");

        // Act: select the next item from the scheduler.
        let received = receivers
            .next_work_item(&semaphore)
            .await
            .expect("at least one queue should have work");

        // Assert: urgent consensus should not be delayed behind ready permitless work.
        assert_eq!(
            received.queue,
            QueueKind::UrgentConsensus,
            "BUG: ready permitless work is selected before ready urgent consensus work"
        );
        assert!(
            received.permit.is_some(),
            "urgent consensus work should hold a worker permit"
        );
    }
}
