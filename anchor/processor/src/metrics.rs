pub use metrics::*;
use std::sync::LazyLock;

/*
 * Gossip processor
 */
pub static ANCHOR_PROCESSOR_WORK_EVENTS_SUBMITTED_COUNT: LazyLock<Result<IntCounterVec>> =
    LazyLock::new(|| {
        try_create_int_counter_vec(
            "anchor_processor_work_events_submitted_count",
            "Count of work events submitted",
            &["type"],
        )
    });
pub static ANCHOR_PROCESSOR_WORK_EVENTS_STARTED_COUNT: LazyLock<Result<IntCounterVec>> =
    LazyLock::new(|| {
        try_create_int_counter_vec(
            "anchor_processor_work_events_started_count",
            "Count of work events which have been started by a worker",
            &["type"],
        )
    });
pub static ANCHOR_PROCESSOR_WORK_EVENTS_EXPIRED_COUNT: LazyLock<Result<IntCounterVec>> =
    LazyLock::new(|| {
        try_create_int_counter_vec(
            "anchor_processor_work_events_expired_count",
            "Count of work events which expired before processing",
            &["type"],
        )
    });
pub static ANCHOR_PROCESSOR_WORKER_TIME: LazyLock<Result<HistogramVec>> = LazyLock::new(|| {
    try_create_histogram_vec(
        "anchor_processor_worker_time",
        "Time taken for a worker to fully process some parcel of work.",
        &["type"],
    )
});
pub static ANCHOR_PROCESSOR_WORKERS_ACTIVE_TOTAL: LazyLock<Result<IntGauge>> =
    LazyLock::new(|| {
        try_create_int_gauge(
            "anchor_processor_workers_active_total",
            "Count of active workers in the processing pool.",
        )
    });
pub static ANCHOR_PROCESSOR_PERMIT_WORKERS_ACTIVE_TOTAL: LazyLock<Result<IntGauge>> =
    LazyLock::new(|| {
        try_create_int_gauge(
            "anchor_processor_permit_workers_active_total",
            "Count of active workers in the processing pool, holding one permit.",
        )
    });
pub static ANCHOR_PROCESSOR_EVENT_HANDLING_SECONDS: LazyLock<Result<Histogram>> =
    LazyLock::new(|| {
        try_create_histogram(
            "anchor_processor_event_handling_seconds",
            "Time spent handling a new message and allocating it to a queue or worker.",
        )
    });
pub static ANCHOR_PROCESSOR_QUEUE_LENGTH: LazyLock<Result<IntGaugeVec>> = LazyLock::new(|| {
    try_create_int_gauge_vec(
        "anchor_processor_work_event_queue_length",
        "Count of work events in queue waiting to be processed.",
        &["type"],
    )
});

/// Errors and Debugging Stats
pub static ANCHOR_PROCESSOR_SEND_ERROR_PER_WORK_TYPE: LazyLock<Result<IntCounterVec>> =
    LazyLock::new(|| {
        try_create_int_counter_vec(
            "anchor_processor_send_error_per_work_type",
            "Total number of anchor processor send error per work type",
            &["type"],
        )
    });
