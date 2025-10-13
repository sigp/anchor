use std::sync::LazyLock;

pub use metrics::*;

// Execution layer metrics
pub static EXECUTION_SYNC_STATUS: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "anchor_execution_sync_status",
        "Status of execution layer sync (0 = down, 1 = up)",
    )
});

pub static EXECUTION_EVENTS_PROCESSED: LazyLock<Result<IntCounterVec>> = LazyLock::new(|| {
    try_create_int_counter_vec(
        "anchor_execution_events_processed_total",
        "Count of events processed by type",
        &["event_type"],
    )
});

pub static EXECUTION_HISTORICAL_SYNC_PROGRESS: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "anchor_execution_historical_sync_progress",
        "Current block number of historical sync",
    )
});

pub static EXECUTION_CURRENT_BLOCK: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "anchor_execution_current_block",
        "Current L1 block tracked by execution layer",
    )
});

pub static EXECUTION_CONNECTION_ERRORS: LazyLock<Result<IntCounterVec>> = LazyLock::new(|| {
    try_create_int_counter_vec(
        "anchor_execution_connection_errors_total",
        "Count of connection errors by endpoint type",
        &["endpoint_type"],
    )
});

pub static EXECUTION_BACKOFF_ATTEMPTS: LazyLock<Result<IntCounterVec>> = LazyLock::new(|| {
    try_create_int_counter_vec(
        "anchor_execution_backoff_attempts_total",
        "Count of backoff attempts by endpoint type",
        &["endpoint_type"],
    )
});

pub static EXECUTION_LOG_FETCH_TIME: LazyLock<Result<HistogramVec>> = LazyLock::new(|| {
    try_create_histogram_vec(
        "anchor_execution_log_fetch_time_seconds",
        "Time taken to fetch logs from L1",
        &["batch_size"],
    )
});

pub static EXECUTION_LOG_PROCESSING_TIME: LazyLock<Result<Histogram>> = LazyLock::new(|| {
    try_create_histogram(
        "anchor_execution_log_processing_time_seconds",
        "Time taken to process logs",
    )
});
