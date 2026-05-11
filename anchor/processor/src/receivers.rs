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
    /// Acquires a permit and delegates to `next_work_item_with_permit`, or retrieves the next work
    /// item from the permitless queue if no permit is available.
    ///
    /// The permit branch is biased first so permit-bound work can win when capacity exists.
    ///
    /// Returns `None` if all queues are closed.
    pub async fn next_work_item(&mut self, semaphore: &Arc<Semaphore>) -> Option<ReceivedWork> {
        select! {
            biased;
            Ok(permit) = semaphore.clone().acquire_owned() => {
                // If only permitless work is ready, the permit is dropped before returning.
                // This is harmless while the processor has a single receiver loop; revisit it if
                // processor polling becomes concurrent.
                self.next_work_item_with_permit(permit).await
            },
            Some(work_item) = self.permitless.recv() => Some(work_item),
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

    fn new_receivers() -> (mpsc::Sender<WorkItem>, mpsc::Sender<WorkItem>, Receivers) {
        let (permitless_tx, permitless_rx) = mpsc::channel(1);
        let (urgent_consensus_tx, urgent_consensus_rx) = mpsc::channel(1);
        let receivers = Receivers {
            permitless: Receiver {
                rx: permitless_rx,
                queue: QueueKind::Permitless,
            },
            urgent_consensus: Receiver {
                rx: urgent_consensus_rx,
                queue: QueueKind::UrgentConsensus,
            },
        };

        (permitless_tx, urgent_consensus_tx, receivers)
    }

    async fn send_permitless(tx: &mpsc::Sender<WorkItem>) {
        tx.send(WorkItem::new_immediate(PERMITLESS_WORK_NAME, |_| {}))
            .await
            .expect("permitless receiver should be open");
    }

    async fn send_urgent_consensus(tx: &mpsc::Sender<WorkItem>) {
        tx.send(WorkItem::new_immediate(URGENT_WORK_NAME, |_| {}))
            .await
            .expect("urgent consensus receiver should be open");
    }

    #[tokio::test]
    async fn test_next_work_item_prioritizes_urgent_consensus_over_ready_permitless() {
        // Arrange: both queues are ready, and urgent consensus has worker capacity available.
        let (permitless_tx, urgent_consensus_tx, mut receivers) = new_receivers();
        let semaphore = Arc::new(Semaphore::new(1));

        send_permitless(&permitless_tx).await;
        send_urgent_consensus(&urgent_consensus_tx).await;

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

    #[tokio::test]
    async fn test_next_work_item_returns_permitless_without_permit_when_capacity_is_available() {
        // Arrange: permitless work is ready, and worker capacity is available.
        let (permitless_tx, _, mut receivers) = new_receivers();
        let semaphore = Arc::new(Semaphore::new(1));
        send_permitless(&permitless_tx).await;

        // Act: select the next item from the scheduler.
        let received = receivers
            .next_work_item(&semaphore)
            .await
            .expect("permitless work should be selected");

        // Assert: permitless work is not charged against the worker permit budget.
        assert_eq!(received.queue, QueueKind::Permitless);
        assert!(
            received.permit.is_none(),
            "permitless work should not hold a worker permit"
        );
        assert_eq!(
            semaphore.available_permits(),
            1,
            "temporary permit acquisition should be released before returning permitless work"
        );
    }

    #[tokio::test]
    async fn test_next_work_item_returns_permitless_when_worker_permits_are_saturated() {
        // Arrange: permitless work is ready, and the worker permit is already held.
        let (permitless_tx, _, mut receivers) = new_receivers();
        let semaphore = Arc::new(Semaphore::new(1));
        let _held_permit = semaphore
            .clone()
            .try_acquire_owned()
            .expect("test should be able to saturate the semaphore");
        send_permitless(&permitless_tx).await;

        // Act: select the next item from the scheduler.
        let received = receivers
            .next_work_item(&semaphore)
            .await
            .expect("permitless work should still drain while permits are saturated");

        // Assert: permitless fallback still works when no worker permits are available.
        assert_eq!(received.queue, QueueKind::Permitless);
        assert!(
            received.permit.is_none(),
            "permitless work should not hold a worker permit"
        );
    }
}
