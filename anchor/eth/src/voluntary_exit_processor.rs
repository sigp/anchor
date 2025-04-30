use std::{sync::Arc, time::Duration};

use database::NetworkDatabase;
use slot_clock::SlotClock;
use ssv_types::ValidatorIndex;
use task_executor::TaskExecutor;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tracing::info;
use types::{voluntary_exit, PublicKeyBytes};

// Message type for exit requests
pub struct ExitRequest {
    pub validator_pubkey: PublicKeyBytes,
    pub validator_index: ValidatorIndex,
    pub block_timestamp: u64,
}

pub type ExitTx = UnboundedSender<ExitRequest>;

const EXIT_PROCESSOR_NAME: &str = "voluntary_exit_processor";

pub fn start_exit_processor(
    slot_clock: impl SlotClock + 'static,
    slots_per_epoch: u64,
    db: Arc<NetworkDatabase>,
    executor: TaskExecutor,
) -> ExitTx {
    let (tx, rx) = unbounded_channel();
    executor.spawn(
        process_exit_requests(slot_clock, slots_per_epoch, db, rx),
        EXIT_PROCESSOR_NAME,
    );
    tx
}

async fn process_exit_requests(
    slot_clock: impl SlotClock + 'static,
    slots_per_epoch: u64,
    db: Arc<NetworkDatabase>,
    mut exit_rx: UnboundedReceiver<ExitRequest>,
) {
    info!("Starting voluntary exit processor");

    while let Some(request) = exit_rx.recv().await {
        process_exit_request(slot_clock.clone(), slots_per_epoch, db.clone(), request).await;
    }

    info!("Exit processor shutting down");
}

async fn process_exit_request(
    slot_clock: impl SlotClock + 'static,
    slots_per_epoch: u64,
    _db: Arc<NetworkDatabase>,
    request: ExitRequest,
) {
    let ExitRequest {
        validator_pubkey,
        validator_index,
        block_timestamp,
    } = request;

    info!(
        validator_pubkey = %validator_pubkey,
        validator_index = ?validator_index,
        "Processing voluntary exit request"
    );

    let block_time = Duration::from_millis(block_timestamp);
    const VOLUNTARY_EXIT_SLOTS_TO_POSTPONE: u64 = 4;
    let slot =
        slot_clock.slot_of(block_time).unwrap_or_default() + VOLUNTARY_EXIT_SLOTS_TO_POSTPONE;

    let epoch = slot.epoch(slots_per_epoch);

    // Create exit message
    let _voluntary_exit = voluntary_exit::VoluntaryExit {
        epoch,
        validator_index: validator_index.0 as u64,
    };

    // Start signature collection process

    // ... rest of implementation

    metrics::inc_counter_vec(
        &crate::metrics::EXECUTION_EVENTS_PROCESSED,
        &["validator_exited"],
    );
}
