//! This struct is used to initialize the tokio runtime, task manager and provides basic
//! functionality for the application to start and shutdown gracefully.

use futures::channel::mpsc::{channel, Receiver, Sender};
use futures::{future, StreamExt};
use std::sync::Arc;
use task_executor::{ShutdownReason, TaskExecutor};
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use {
    futures::Future,
    std::{pin::Pin, task::Context, task::Poll},
    tokio::signal::unix::{signal, Signal, SignalKind},
};

#[cfg(target_family = "windows")]
#[path = "environment_windows.rs"]
mod environment_windows;

/// The maximum time in seconds the client will wait for all internal tasks to shutdown.
const MAXIMUM_SHUTDOWN_TIME: u64 = 15;

pub struct Environment {
    /// The tokio runtime for the client, used to spawn tasks.
    runtime: Arc<Runtime>,
    /// Receiver side of an internal shutdown signal.
    signal_rx: Option<Receiver<ShutdownReason>>,
    /// Sender to request shutting down.
    signal_tx: Sender<ShutdownReason>,
    signal: Option<async_channel::Sender<()>>,
    exit: async_channel::Receiver<()>,
}

impl Default for Environment {
    /// Generates a default multi-threaded tokio task executor with the required channels in order
    /// to gracefully shutdown the application.
    ///
    /// If a more fine-grained executor is required, a more general function should be built.
    fn default() -> Self {
        // Default logging to `debug` for the time being
        let env_filter = EnvFilter::new("debug");
        tracing_subscriber::fmt().with_env_filter(env_filter).init();

        // Create a multi-threaded task executor
        let runtime = match RuntimeBuilder::new_multi_thread().enable_all().build() {
            Err(e) => {
                error!(error = %e, "Failed to initialize runtime");
                panic!("Failed to start the application");
            }
            Ok(runtime) => Arc::new(runtime),
        };
        // Channels to exit tasks and gracefully terminate the application
        let (signal, exit) = async_channel::bounded(1);
        let (signal_tx, signal_rx) = channel(1);

        Environment {
            runtime,
            signal_rx: Some(signal_rx),
            signal_tx,
            signal: Some(signal),
            exit,
        }
    }
}

impl Environment {
    /// Generates a task executor that can be used by the application to spawn tasks.
    pub fn executor(&mut self) -> TaskExecutor {
        TaskExecutor::new(
            Arc::downgrade(self.runtime()),
            self.exit.clone(),
            self.signal_tx.clone(),
        )
    }

    /// Returns a mutable reference to the `tokio` runtime.
    ///
    /// Useful in the rare scenarios where it's necessary to block the current thread until a task
    /// is finished (e.g., during testing).
    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }

    /// Block the current thread until a shutdown signal is received.
    ///
    /// This can be either the user Ctrl-C'ing or a task requesting to shutdown.
    #[cfg(target_family = "unix")]
    pub fn block_until_shutdown_requested(&mut self) -> Result<ShutdownReason, String> {
        // future of a task requesting to shutdown
        let mut signal_rx = self
            .signal_rx
            .take()
            .ok_or("Inner shutdown already received")?;
        let inner_shutdown = async move {
            signal_rx
                .next()
                .await
                .ok_or("Internal shutdown channel exhausted")
        };
        futures::pin_mut!(inner_shutdown);

        let register_handlers = async {
            let mut handles = vec![];

            // setup for handling SIGTERM
            match signal(SignalKind::terminate()) {
                Ok(terminate_stream) => {
                    let terminate = SignalFuture::new(terminate_stream, "Received SIGTERM");
                    handles.push(terminate);
                }
                Err(e) => error!(error = ?e, "Could not register SIGTERM handler"),
            };

            // setup for handling SIGINT
            match signal(SignalKind::interrupt()) {
                Ok(interrupt_stream) => {
                    let interrupt = SignalFuture::new(interrupt_stream, "Received SIGINT");
                    handles.push(interrupt);
                }
                Err(e) => error!(error = ?e, "Could not register SIGINT handler"),
            }

            // setup for handling a SIGHUP
            match signal(SignalKind::hangup()) {
                Ok(hup_stream) => {
                    let hup = SignalFuture::new(hup_stream, "Received SIGHUP");
                    handles.push(hup);
                }
                Err(e) => error!(error = ?e, "Could not register SIGHUP handler"),
            }

            future::select(inner_shutdown, future::select_all(handles.into_iter())).await
        };

        match self.runtime().block_on(register_handlers) {
            future::Either::Left((Ok(reason), _)) => {
                info!(reason = reason.message(), "Internal shutdown received");
                Ok(reason)
            }
            future::Either::Left((Err(e), _)) => Err(e.into()),
            future::Either::Right(((res, _, _), _)) => {
                res.ok_or_else(|| "Handler channel closed".to_string())
            }
        }
    }

    #[cfg(target_family = "windows")]
    pub fn block_until_shutdown_requested(&mut self) -> Result<ShutdownReason, String> {
        let signal_rx = self
            .signal_rx
            .take()
            .ok_or("Inner shutdown already received")?;

        match self
            .runtime()
            .block_on(environment_windows::handle_shutdown_signals(signal_rx))
        {
            Ok(reason) => {
                info!(reason = reason.message(), "Internal shutdown received");
                Ok(reason)
            }
            Err(e) => Err(e),
        }
    }

    /// Shutdown the `tokio` runtime when all tasks are idle.
    pub fn shutdown_on_idle(self) {
        match Arc::try_unwrap(self.runtime) {
            Ok(runtime) => {
                runtime.shutdown_timeout(std::time::Duration::from_secs(MAXIMUM_SHUTDOWN_TIME))
            }
            Err(e) => warn!(
                error = ?e,
                "Failed to obtain runtime access to shutdown gracefully"
            ),
        }
    }

    /// Fire exit signal which shuts down all spawned services
    pub fn fire_signal(&mut self) {
        if let Some(signal) = self.signal.take() {
            drop(signal);
        }
    }
}

#[cfg(target_family = "unix")]
struct SignalFuture {
    signal: Signal,
    message: &'static str,
}

#[cfg(target_family = "unix")]
impl SignalFuture {
    pub fn new(signal: Signal, message: &'static str) -> SignalFuture {
        SignalFuture { signal, message }
    }
}

#[cfg(target_family = "unix")]
impl Future for SignalFuture {
    type Output = Option<ShutdownReason>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.signal.poll_recv(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(_)) => Poll::Ready(Some(ShutdownReason::Success(self.message))),
            Poll::Ready(None) => Poll::Ready(None),
        }
    }
}
