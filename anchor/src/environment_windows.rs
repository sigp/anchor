use futures::channel::mpsc::Receiver;
use futures::{future, Future, StreamExt};
use std::{pin::Pin, task::Context, task::Poll};
use task_executor::ShutdownReason;
use tokio::signal::windows::{ctrl_c, CtrlC};
use tracing::error;

pub(crate) async fn handle_shutdown_signals(
    mut signal_rx: Receiver<ShutdownReason>,
) -> Result<ShutdownReason, String> {
    let inner_shutdown = async move {
        signal_rx
            .next()
            .await
            .ok_or("Internal shutdown channel exhausted")
    };
    futures::pin_mut!(inner_shutdown);

    let register_handlers = async {
        let mut handles = vec![];

        // Setup for handling Ctrl+C
        match ctrl_c() {
            Ok(ctrl_c) => {
                let ctrl_c = SignalFuture::new(ctrl_c, "Received Ctrl+C");
                handles.push(ctrl_c);
            }
            Err(e) => error!(error = ?e, "Could not register Ctrl+C handler"),
        }

        future::select(inner_shutdown, future::select_all(handles.into_iter())).await
    };

    match register_handlers.await {
        future::Either::Left((Ok(reason), _)) => Ok(reason),
        future::Either::Left((Err(e), _)) => Err(e.into()),
        future::Either::Right(((res, _, _), _)) => {
            res.ok_or_else(|| "Handler channel closed".to_string())
        }
    }
}

struct SignalFuture {
    signal: CtrlC,
    message: &'static str,
}

impl SignalFuture {
    pub fn new(signal: CtrlC, message: &'static str) -> SignalFuture {
        SignalFuture { signal, message }
    }
}

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
