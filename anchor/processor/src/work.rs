use std::{
    fmt::{Debug, Formatter},
    future::Future,
    pin::Pin,
};

use tokio::{sync::OwnedSemaphorePermit, time::Instant};

use crate::metrics;

pub type AsyncFn = Pin<Box<dyn Future<Output = ()> + Send>>;
pub type BlockingFn = Box<dyn FnOnce() + Send>;
pub type ImmediateFn = Box<dyn FnOnce(DropOnFinish) + Send>;

pub(crate) enum WorkKind {
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

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn expiry(&self) -> &Option<Instant> {
        &self.expiry
    }

    pub(crate) fn func(self) -> WorkKind {
        self.func
    }
}

/// Refunds the permit and updates metrics on drop.
#[derive(Debug)]
pub struct DropOnFinish {
    pub(crate) permit: Option<OwnedSemaphorePermit>,
    pub(crate) _work_timer: Option<metrics::HistogramTimer>,
}
impl Drop for DropOnFinish {
    fn drop(&mut self) {
        metrics::dec_gauge(&metrics::ANCHOR_PROCESSOR_WORKERS_ACTIVE_TOTAL);
        if self.permit.is_some() {
            metrics::dec_gauge(&metrics::ANCHOR_PROCESSOR_PERMIT_WORKERS_ACTIVE_TOTAL);
        }
    }
}
