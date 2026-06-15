use std::{error::Error, sync::Arc, time::Duration};

use task_executor::TaskExecutor;
use tokio::{
    select,
    sync::{Barrier, Notify, oneshot},
    time::sleep,
};

/// Workers for the burst test; kept small so a few blocking tasks saturate every
/// permit and stop the processor draining `urgent_consensus`.
const BURST_TEST_MAX_WORKERS: usize = 3;

/// Burst size for the capacity test: above the old 1000 cap and below the new 4096
/// default, so every send succeeds now but would have overflowed before (#1088).
const BURST_TEST_ITEM_COUNT: usize = 1500;

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

/// Verifies that with the default config the `urgent_consensus` queue absorbs a burst
/// larger than its old 1000 cap without returning `TrySendError::Full` (#1088).
///
/// Determinism comes from saturating every worker permit before the burst, so the
/// processor cannot drain `urgent_consensus` and the channel only fills; the inline
/// comments justify each step.
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

    // Barrier opens only once all blockers plus this thread have arrived; reaching it
    // proves the processor has spawned every blocker and thus acquired every permit.
    let permits_saturated = Arc::new(Barrier::new(BURST_TEST_MAX_WORKERS + 1));
    // Holds the blockers (and their permits) until the capacity assertion is done.
    let release_blockers = Arc::new(Notify::new());

    // Occupy every worker permit with a task that parks without releasing its permit.
    for _ in 0..BURST_TEST_MAX_WORKERS {
        let permits_saturated = permits_saturated.clone();
        let release_blockers = release_blockers.clone();
        sender_queues.urgent_consensus.send_async(
            async move {
                permits_saturated.wait().await;
                release_blockers.notified().await;
            },
            "burst_test_blocker",
        )?;
    }

    // Wait until all permits are confirmed held. After this, the processor can no longer
    // drain `urgent_consensus`, so the channel only accumulates work.
    select! {
        _ = sleep(Duration::from_secs(5)) => panic!("blockers should saturate all worker permits"),
        _ = permits_saturated.wait() => {},
    }

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

    // Release the blockers so the runtime can shut down cleanly.
    release_blockers.notify_waiters();

    Ok(())
}
