use std::{
    sync::LazyLock,
    time::{SystemTime, UNIX_EPOCH},
};

pub use metrics::*;
use tracing::error;
use version::VERSION;

pub static PROCESS_START_TIME_SECONDS: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "process_start_time_seconds",
        "The unix timestamp at which the process was started",
    )
});

pub static ANCHOR_VERSION: LazyLock<Result<IntGaugeVec>> = LazyLock::new(|| {
    try_create_int_gauge_vec(
        "anchor_info",
        "The build of Anchor running on the server",
        &["version"],
    )
});

/// Count of head events dropped by the fan-out relay because a downstream
/// channel was full, labelled by which downstream filled up.
pub static HEAD_EVENT_FANOUT_DROPS: LazyLock<Result<IntCounterVec>> = LazyLock::new(|| {
    try_create_int_counter_vec(
        "anchor_head_event_fanout_drops_total",
        "Head events dropped by the fan-out relay due to a full downstream channel",
        &["downstream"],
    )
});

pub fn expose_process_start_time() {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => set_gauge(&PROCESS_START_TIME_SECONDS, duration.as_secs() as i64),
        Err(e) => error!(
            error = %e,
            "Failed to read system time"
        ),
    }
}

pub fn expose_anchor_version() {
    set_gauge_vec(&ANCHOR_VERSION, &[VERSION], 1);
}
