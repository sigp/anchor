use std::{collections::HashMap, error::Error, sync::Arc, time::Duration};

use task_executor::TaskExecutor;
use tokio::{
    select,
    sync::{Barrier, Notify, mpsc::error::TrySendError, oneshot},
    time::sleep,
};

/// Workers for the capacity tests; kept small so a few blocking tasks saturate every
/// permit and stop the processor draining `urgent_consensus`.
const BURST_TEST_MAX_WORKERS: usize = 3;

/// Burst size for the positive capacity test: above the old 1000 cap and below the new
/// 4096 default, so every send succeeds now but would have overflowed before (#1088).
const BURST_TEST_ITEM_COUNT: usize = 1500;

/// The previous `urgent_consensus` default. The negative test pins the queue here to prove
/// the burst test is non-vacuous: at this cap the same burst really is rejected.
const OLD_QUEUE_CAP: usize = 1000;

#[tokio::test]
async fn test_max_workers() -> Result<(), Box<dyn Error>> {
    let handle = tokio::runtime::Handle::current();
    let (_signal, exit) = async_channel::bounded(1);
    let (shutdown_tx, _) = futures::channel::mpsc::channel(1);
    let executor = TaskExecutor::new(handle, exit, shutdown_tx);

    let config = processor::Config {
        max_workers: 3,
        queue_size: Default::default(),
    };

    let sender_queues = processor::spawn(config, executor);

    let start_sync = Arc::new(Barrier::new(4));
    let continue_notify = Arc::new(Notify::new());

    // fill up the available workers
    for _ in 0..3 {
        let start_sync = start_sync.clone();
        let continue_notify = continue_notify.clone();
        sender_queues.urgent_consensus.send_async(
            async move {
                start_sync.wait().await;
                continue_notify.notified().await;
            },
            "test_task1",
        )?;

        // throw in some permitless tasks
        sender_queues
            .permitless
            .send_blocking(|| {}, "test_task2")?;
        sender_queues
            .permitless
            .send_immediate(|_| {}, "test_task3")?;
    }

    // wait until every task has been spawned
    select! {
        _ = sleep(Duration::from_millis(100)) => panic!("we should be able to run the blockers"),
        _ = start_sync.wait() => {},
    }

    let permitless_sync = Arc::new(Barrier::new(2));
    let passed_permitless_sync = permitless_sync.clone();
    // now, we should be able to spawn only via the "permitless" queue
    sender_queues.permitless.send_async(
        async move {
            passed_permitless_sync.wait().await;
        },
        "test_task4",
    )?;

    let (did_run_tx, mut did_run_rx) = oneshot::channel();
    // but other queues should only run after we freed up space:
    sender_queues.urgent_consensus.send_async(
        async move {
            let _ = did_run_tx.send(());
        },
        "test_task5",
    )?;

    // see if the permitless one ran
    select! {
        _ = sleep(Duration::from_millis(100)) => panic!("the permitless task should be executed"),
        _ = permitless_sync.wait() => {},
    }

    // see if the other one ran
    select! {
        _ = &mut did_run_rx => panic!("the task should not be executed yet"),
        // it's probably fine after one ms - increase if this fails spuriously. Sorry!
        // feel free to improve the approach here
        _ = sleep(Duration::from_millis(1)) => {},
    }

    // allow the three blocking tasks to finish
    continue_notify.notify_waiters();

    // now, the waiting task should be scheduled
    select! {
        _ = sleep(Duration::from_millis(100)) => panic!("the task should be executed now"),
        _ = did_run_rx => {},
    }

    Ok(())
}

/// Handle returned by [`saturate_workers`]. Releasing it lets the parked blockers finish so
/// the runtime can shut down cleanly.
struct BlockerRelease {
    release_blockers: Arc<Barrier>,
}

impl BlockerRelease {
    async fn release(self) {
        self.release_blockers.wait().await;
    }
}

/// Saturates every worker permit with a parked task so the processor cannot drain
/// `urgent_consensus`, isolating the channel's capacity from the worker pool. Returns once
/// all permits are provably held; the returned handle frees the blockers afterwards.
async fn saturate_workers(senders: &processor::Senders, max_workers: usize) -> BlockerRelease {
    // Opens only once all blockers plus this task have arrived, proving the processor has
    // spawned every blocker and therefore acquired every permit.
    let permits_saturated = Arc::new(Barrier::new(max_workers + 1));
    // A second barrier releases them. Unlike `Notify::notify_waiters`, a barrier cannot lose
    // the wakeup if a blocker has not yet parked when the caller releases.
    let release_blockers = Arc::new(Barrier::new(max_workers + 1));

    // Occupy every worker permit with a task that parks without releasing its permit.
    for _ in 0..max_workers {
        let permits_saturated = permits_saturated.clone();
        let release_blockers = release_blockers.clone();
        senders
            .urgent_consensus
            .send_async(
                async move {
                    permits_saturated.wait().await;
                    release_blockers.wait().await;
                },
                "saturate_blocker",
            )
            .expect("blocker should enqueue while the queue still has capacity");
    }

    // Wait until all permits are confirmed held. After this, the processor can no longer
    // drain `urgent_consensus`, so the channel only accumulates work.
    select! {
        _ = sleep(Duration::from_secs(5)) => panic!("blockers should saturate all worker permits"),
        _ = permits_saturated.wait() => {},
    }

    BlockerRelease { release_blockers }
}

/// Verifies that with the default config the `urgent_consensus` queue absorbs a burst larger
/// than its old 1000 cap without returning `TrySendError::Full` (#1088). Determinism comes
/// from saturating every worker permit first, so the processor cannot drain the channel.
#[tokio::test]
async fn test_urgent_consensus_absorbs_burst_above_old_cap() -> Result<(), Box<dyn Error>> {
    let handle = tokio::runtime::Handle::current();
    let (_signal, exit) = async_channel::bounded(1);
    let (shutdown_tx, _) = futures::channel::mpsc::channel(1);
    let executor = TaskExecutor::new(handle, exit, shutdown_tx);

    // Default queue sizes: this exercises the `urgent_consensus` default of 4096.
    let config = processor::Config {
        max_workers: BURST_TEST_MAX_WORKERS,
        queue_size: Default::default(),
    };
    let sender_queues = processor::spawn(config, executor);

    let release = saturate_workers(&sender_queues, BURST_TEST_MAX_WORKERS).await;

    // Submit a burst larger than the old 1000 cap. With the 4096 default every send must
    // succeed; under the old cap these would have started failing with `Full` past 1000.
    for i in 0..BURST_TEST_ITEM_COUNT {
        sender_queues
            .urgent_consensus
            .send_async(async {}, "burst_test_item")
            .unwrap_or_else(|err| {
                panic!(
                    "urgent_consensus rejected burst item {i} of {BURST_TEST_ITEM_COUNT} \
                     (queue should hold {BURST_TEST_ITEM_COUNT} > old 1000 cap): {err:?}"
                )
            });
    }

    release.release().await;
    Ok(())
}

/// Backs the non-vacuity of the burst test above: with the queue pinned back to the old 1000
/// cap and the same saturated workers, the channel accepts exactly `OLD_QUEUE_CAP` items and
/// then rejects the next one with `TrySendError::Full` (#1088).
#[tokio::test]
async fn test_urgent_consensus_rejects_burst_past_old_cap() -> Result<(), Box<dyn Error>> {
    let handle = tokio::runtime::Handle::current();
    let (_signal, exit) = async_channel::bounded(1);
    let (shutdown_tx, _) = futures::channel::mpsc::channel(1);
    let executor = TaskExecutor::new(handle, exit, shutdown_tx);

    // Pin the queue to the old cap so the same harness must reject once it is full.
    let config = processor::Config {
        max_workers: BURST_TEST_MAX_WORKERS,
        queue_size: HashMap::from([(processor::QueueKind::UrgentConsensus, OLD_QUEUE_CAP)]),
    };
    let sender_queues = processor::spawn(config, executor);

    let release = saturate_workers(&sender_queues, BURST_TEST_MAX_WORKERS).await;

    // With workers saturated the empty channel accepts exactly `OLD_QUEUE_CAP` items.
    for i in 0..OLD_QUEUE_CAP {
        sender_queues
            .urgent_consensus
            .send_async(async {}, "fill_item")
            .unwrap_or_else(|err| {
                panic!("item {i} should fit in the {OLD_QUEUE_CAP}-cap queue: {err:?}")
            });
    }

    // The next send overflows: this is the drop the 4096 default now prevents.
    let overflow = sender_queues
        .urgent_consensus
        .send_async(async {}, "overflow_item");
    assert!(
        matches!(
            &overflow,
            Err(processor::Error::Queue(TrySendError::Full(_)))
        ),
        "the item past the {OLD_QUEUE_CAP}-cap queue must overflow with Full, got {overflow:?}"
    );

    release.release().await;
    Ok(())
}
