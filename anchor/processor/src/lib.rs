//! Central processor, serving roughly the same purpose as Lighthouse's `beacon_processor`.
//!
//! The processor does not centrally define the available work items, but provides [`WorkItem`]
//! which can be used to send work to the processor via [`Sender`]s. The processor then retrieves
//! work items from priority-ranked queues and launches the items in a way corresponding to their
//! type. For most queues, a permit is needed, which are handed out by the processor up to a
//! configured value, effectively limiting the number of concurrent tasks. This avoids overloading
//! the system and prioritizes items based on the queues they were submitted to.

pub(crate) mod metrics;
mod receivers;
pub mod senders;
pub mod work;

use std::{collections::HashMap, str::FromStr, sync::Arc};

use task_executor::TaskExecutor;
use tokio::{
    sync::{mpsc, mpsc::error::TrySendError, Semaphore},
    time::Instant,
};
use tracing::{error, warn};

pub use crate::senders::Senders;
use crate::{
    receivers::{Receiver, Receivers},
    senders::Sender,
    work::{DropOnFinish, WorkItem, WorkKind},
};

#[derive(Clone, Debug)]
/// Configuration for a processor. Provided to [spawn].
pub struct Config {
    /// The maximum amount of concurrent workers. Note that [WorkItem]s submitted via
    /// [Senders::permitless_tx] do not count towards this limit. By default, this is the number of
    /// logical CPUs.
    pub max_workers: usize,

    /// The sizes for the queues. If a queue is not present in the map, its default is used.
    pub queue_size: HashMap<QueueKind, usize>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_workers: num_cpus::get(),
            queue_size: HashMap::new(),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Processor queue full")]
    Queue(#[from] TrySendError<WorkItem>),
}

// TODO: add all the needed queues
// https://github.com/sigp/anchor/issues/254

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum QueueKind {
    Permitless,
    UrgentConsensus,
}

impl QueueKind {
    fn name(self) -> &'static str {
        match self {
            QueueKind::Permitless => "permitless",
            QueueKind::UrgentConsensus => "urgent_consensus",
        }
    }

    fn default_size(self) -> usize {
        match self {
            QueueKind::Permitless => 1000,
            QueueKind::UrgentConsensus => 1000,
        }
    }

    fn create(self, config: &Config) -> (Sender, Receiver) {
        let (tx, rx) = mpsc::channel(
            config
                .queue_size
                .get(&self)
                .copied()
                .unwrap_or(self.default_size()),
        );
        (Sender { tx, queue: self }, Receiver { rx, queue: self })
    }
}

impl FromStr for QueueKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "permitless" => QueueKind::Permitless,
            "urgent_consensus" => QueueKind::UrgentConsensus,
            _ => return Err(()),
        })
    }
}

/// Create a new processor and spawn it with the given executor. Returns the queue senders.
pub fn spawn(config: Config, executor: TaskExecutor) -> Senders {
    let (permitless_tx, permitless_rx) = QueueKind::Permitless.create(&config);
    let (urgent_consensus_tx, urgent_consensus_rx) = QueueKind::UrgentConsensus.create(&config);

    let senders = Senders {
        permitless: permitless_tx,
        urgent_consensus: urgent_consensus_tx,
    };
    let receivers = Receivers {
        permitless: permitless_rx,
        urgent_consensus: urgent_consensus_rx,
    };

    executor.spawn(processor(config, receivers, executor.clone()), "processor");
    senders
}

async fn processor(config: Config, mut receivers: Receivers, executor: TaskExecutor) {
    let semaphore = Arc::new(Semaphore::new(config.max_workers));

    loop {
        let _timer = metrics::start_timer(&metrics::ANCHOR_PROCESSOR_EVENT_HANDLING_SECONDS);

        // Try to get the next work event, which will be None when all queues are closed
        let received = receivers.next_work_item(&semaphore).await;
        let Some(received) = received else {
            error!("Processor queues closed unexpectedly");
            break;
        };

        let name = received.work_item.name();

        metrics::dec_gauge_vec(
            &metrics::ANCHOR_PROCESSOR_QUEUE_LENGTH,
            &[name, received.queue.name()],
        );

        if let Some(expiry) = received.work_item.expiry() {
            if expiry < &Instant::now() {
                warn!(task = name, "Processor skipped expired work");
                metrics::inc_counter_vec(
                    &metrics::ANCHOR_PROCESSOR_WORK_EVENTS_EXPIRED_COUNT,
                    &[name],
                );
                continue;
            }
        }

        // update metrics
        metrics::inc_gauge(&metrics::ANCHOR_PROCESSOR_WORKERS_ACTIVE_TOTAL);
        if received.permit.is_some() {
            metrics::inc_gauge(&metrics::ANCHOR_PROCESSOR_PERMIT_WORKERS_ACTIVE_TOTAL);
        }
        metrics::inc_counter_vec(
            &metrics::ANCHOR_PROCESSOR_WORK_EVENTS_STARTED_COUNT,
            &[received.work_item.name()],
        );
        let drop_on_finish = DropOnFinish {
            permit: received.permit,
            _work_timer: metrics::start_timer_vec(
                &metrics::ANCHOR_PROCESSOR_WORKER_TIME,
                &[received.work_item.name()],
            ),
        };

        match received.work_item.func() {
            WorkKind::Async(async_fn) => executor.spawn(
                async move {
                    async_fn.await;
                    drop(drop_on_finish);
                },
                name,
            ),
            WorkKind::Blocking(blocking_fn) => {
                executor.spawn_blocking(
                    move || {
                        blocking_fn();
                        drop(drop_on_finish);
                    },
                    name,
                );
            }
            WorkKind::Immediate(immediate_fn) => immediate_fn(drop_on_finish),
        }
    }
}
