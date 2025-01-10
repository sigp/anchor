use std::error::Error;
use std::sync::Arc;
use std::time::Duration;
use task_executor::TaskExecutor;
use tokio::select;
use tokio::sync::{oneshot, Barrier, Notify};
use tokio::time::sleep;

#[tokio::test]
async fn test_max_workers() -> Result<(), Box<dyn Error>> {
    let handle = tokio::runtime::Handle::current();
    let (_signal, exit) = async_channel::bounded(1);
    let (shutdown_tx, _) = futures::channel::mpsc::channel(1);
    let executor = TaskExecutor::new(handle, exit, shutdown_tx);

    let config = processor::Config { max_workers: 3 };

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
