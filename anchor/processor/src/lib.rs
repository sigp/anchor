//! Central processor, serving roughly the same purpose as Lighthouse's `beacon_processor`.
//!
//! The processor does not centrally define the available work items, but provides [`WorkItem`]
//! which can be used to send work to the processor via [`Sender`]s. The processor then retrieves
//! work items from priority-ranked queues and launches the items in a way corresponding to their
//! type. For most queues, a permit is needed, which are handed out by the processor up to a
//! configured value, effectively limiting the number of concurrent tasks. This avoids overloading
//! the system and prioritizes items based on the queues they were submitted to.

mod metrics;

use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use task_executor::TaskExecutor;
use tokio::select;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;
use tracing::{error, warn};

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Configuration for a processor. Provided to [spawn].
pub struct Config {
    /// The maximum amount of concurrent workers. Note that [WorkItem]s submitted via
    /// [Senders::permitless_tx] do not count towards this limit. By default, this is the number of
    /// logical CPUs.
    pub max_workers: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_workers: num_cpus::get(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Sender {
    tx: mpsc::Sender<WorkItem>,
}

impl Sender {
    /// Convenience method creating an async [`WorkItem`] and sending it.
    pub fn send_async<F: Future<Output = ()> + Send + 'static>(
        &self,
        future: F,
        name: &'static str,
    ) -> Result<(), TrySendError<WorkItem>> {
        self.send_work_item(WorkItem {
            func: WorkKind::Async(Box::pin(future)),
            expiry: None,
            name,
        })
    }

    /// Convenience method creating a blocking [`WorkItem`] and sending it.
    pub fn send_blocking<F: FnOnce() + Send + 'static>(
        &self,
        func: F,
        name: &'static str,
    ) -> Result<(), TrySendError<WorkItem>> {
        self.send_work_item(WorkItem {
            func: WorkKind::Blocking(Box::new(func)),
            expiry: None,
            name,
        })
    }

    /// Convenience method creating an immediate [`WorkItem`] and sending it.
    pub fn send_immediate<F: FnOnce(DropOnFinish) + Send + 'static>(
        &self,
        func: F,
        name: &'static str,
    ) -> Result<(), TrySendError<WorkItem>> {
        self.send_work_item(WorkItem {
            func: WorkKind::Immediate(Box::new(func)),
            expiry: None,
            name,
        })
    }

    /// Sends a [`WorkItem`] into the queue, non-blocking, returning an error if the queue is full.
    /// Handles metrics and logging for you.
    pub fn send_work_item(&self, item: WorkItem) -> Result<(), TrySendError<WorkItem>> {
        let name = item.name;
        let result = self.tx.try_send(item);
        if let Err(err) = &result {
            metrics::inc_counter_vec(&metrics::ANCHOR_PROCESSOR_SEND_ERROR_PER_WORK_TYPE, &[name]);
            match err {
                TrySendError::Full(_) => {
                    warn!(task = name, "Processor queue full")
                }
                TrySendError::Closed(_) => {
                    error!("Processor queue closed unexpectedly")
                }
            }
        } else {
            metrics::inc_counter_vec(
                &metrics::ANCHOR_PROCESSOR_WORK_EVENTS_SUBMITTED_COUNT,
                &[name],
            );
            metrics::inc_gauge_vec(&metrics::ANCHOR_PROCESSOR_QUEUE_LENGTH, &[name]);
        }
        result
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// Bag of available senders relevant for the Anchor client.
#[derive(Clone, Debug)]
pub struct Senders {
    /// Catch-all queue for tasks that are either very quick to run or behave well as async task in
    /// the Tokio runtime. Is launched immediately and does not require capacity as defined by
    /// [`Config::max_workers`].
    pub permitless: Sender,
    pub urgent_consensus: Sender,
    // todo add all the needed queues here
}

struct Receivers {
    permitless_rx: mpsc::Receiver<WorkItem>,
    urgent_consensus_rx: mpsc::Receiver<WorkItem>,
    // todo add all the needed queues here
}

pub type AsyncFn = Pin<Box<dyn Future<Output = ()> + Send>>;
pub type BlockingFn = Box<dyn FnOnce() + Send>;
pub type ImmediateFn = Box<dyn FnOnce(DropOnFinish) + Send>;

enum WorkKind {
    Async(AsyncFn),
    Blocking(BlockingFn),
    Immediate(ImmediateFn),
}

impl Debug for WorkKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkKind::Async(_) => f.write_str("Async"),
            WorkKind::Blocking(_) => f.write_str("Blocking"),
            WorkKind::Immediate(_) => f.write_str("Immediate"),
        }
    }
}

#[derive(Debug)]
pub struct WorkItem {
    func: WorkKind,
    expiry: Option<Instant>,
    name: &'static str,
}

impl WorkItem {
    /// Create an async work task. Will be spawned on the Tokio runtime.
    pub fn new_async<F: Future<Output = ()> + Send + 'static>(name: &'static str, func: F) -> Self {
        Self {
            name,
            expiry: None,
            func: WorkKind::Async(Box::pin(func)),
        }
    }

    /// Create a blocking work task. Will be spawned on the Tokio runtime using `spawn_blocking`.
    pub fn new_blocking<F: FnOnce() + Send + 'static>(name: &'static str, func: F) -> Self {
        Self {
            name,
            expiry: None,
            func: WorkKind::Blocking(Box::new(func)),
        }
    }

    /// Create an immediate work task. Has access to the [`ProcessorState`], and is thus ideal for
    /// triggering some process, e.g. via a queue retrieved from the state. Must *NEVER* block!
    ///
    /// The [`DropOnFinish`] should be dropped when the work is done, for proper permit accounting
    /// and metrics. This includes any work triggered by the closure, so [`DropOnFinish`] should
    /// be sent along if any other process such as a QBFT instance is messaged.
    pub fn new_immediate<F: FnOnce(DropOnFinish) + Send + 'static>(
        name: &'static str,
        func: F,
    ) -> Self {
        Self {
            name,
            expiry: None,
            func: WorkKind::Immediate(Box::new(func)),
        }
    }

    /// Set expiry of this work item. If the processor retrieves the work item after the expiry,
    /// it drops the work item instead.
    pub fn set_expiry(&mut self, expiry: Option<Instant>) {
        self.expiry = expiry;
    }

    pub fn with_expiry(mut self, expiry: Instant) -> Self {
        self.expiry = Some(expiry);
        self
    }
}

/// Refunds the permit and updates metrics on drop.
#[derive(Debug)]
pub struct DropOnFinish {
    permit: Option<OwnedSemaphorePermit>,
    _work_timer: Option<metrics::HistogramTimer>,
}
impl Drop for DropOnFinish {
    fn drop(&mut self) {
        metrics::dec_gauge(&metrics::ANCHOR_PROCESSOR_WORKERS_ACTIVE_TOTAL);
        if self.permit.is_some() {
            metrics::dec_gauge(&metrics::ANCHOR_PROCESSOR_PERMIT_WORKERS_ACTIVE_TOTAL);
        }
    }
}

/// Create a new processor and spawn it with the given executor. Returns the queue senders.
pub fn spawn(config: Config, executor: TaskExecutor) -> Senders {
    let (permitless_tx, permitless_rx) = mpsc::channel(1000);
    let (urgent_consensus_tx, urgent_consensus_rx) = mpsc::channel(1000);

    let senders = Senders {
        permitless: Sender { tx: permitless_tx },
        urgent_consensus: Sender {
            tx: urgent_consensus_tx,
        },
    };
    let receivers = Receivers {
        permitless_rx,
        urgent_consensus_rx,
    };

    executor.spawn(processor(config, receivers, executor.clone()), "processor");
    senders
}

async fn processor(config: Config, mut receivers: Receivers, executor: TaskExecutor) {
    let semaphore = Arc::new(Semaphore::new(config.max_workers));

    loop {
        let _timer = metrics::start_timer(&metrics::ANCHOR_PROCESSOR_EVENT_HANDLING_SECONDS);

        // Try to get the next work event. work_item will only be None when the queues are closed.
        // Permit will be None when the event was received from permitless_rx.
        let (permit, work_item) = select! {
            biased;
            Some(w) = receivers.permitless_rx.recv() => (None, Some(w)),
            Ok(permit) = semaphore.clone().acquire_owned() => {
                select! {
                    biased;
                    Some(w) = receivers.urgent_consensus_rx.recv() => (Some(permit), Some(w)),

                    // we have a permit, so we prefer other queues at this point,
                    // but it should still be possible to receive a permitless event
                    Some(w) = receivers.permitless_rx.recv() => (None, Some(w)),
                    else => (None, None),
                }
            }
            else => (None, None),
        };
        let Some(work_item) = work_item else {
            error!("Processor queues closed unexpectedly");
            break;
        };
        if let Some(expiry) = work_item.expiry {
            if expiry < Instant::now() {
                warn!(task = work_item.name, "Processor skipped expired work");
                metrics::inc_counter_vec(
                    &metrics::ANCHOR_PROCESSOR_WORK_EVENTS_EXPIRED_COUNT,
                    &[work_item.name],
                );
                continue;
            }
        }

        // update metrics
        metrics::inc_gauge(&metrics::ANCHOR_PROCESSOR_WORKERS_ACTIVE_TOTAL);
        if permit.is_some() {
            metrics::inc_gauge(&metrics::ANCHOR_PROCESSOR_PERMIT_WORKERS_ACTIVE_TOTAL);
        }
        metrics::inc_counter_vec(
            &metrics::ANCHOR_PROCESSOR_WORK_EVENTS_STARTED_COUNT,
            &[work_item.name],
        );
        let drop_on_finish = DropOnFinish {
            permit,
            _work_timer: metrics::start_timer_vec(
                &metrics::ANCHOR_PROCESSOR_WORKER_TIME,
                &[work_item.name],
            ),
        };

        match work_item.func {
            WorkKind::Async(async_fn) => executor.spawn(
                async move {
                    async_fn.await;
                    drop(drop_on_finish);
                },
                work_item.name,
            ),
            WorkKind::Blocking(blocking_fn) => {
                executor.spawn_blocking(
                    move || {
                        blocking_fn();
                        drop(drop_on_finish);
                    },
                    work_item.name,
                );
            }
            WorkKind::Immediate(immediate_fn) => immediate_fn(drop_on_finish),
        }
    }
}
