use std::sync::LazyLock;

pub use metrics::*;

pub const AGGREGATE_AND_PROOF: &str = "aggregate_and_proof";
pub const BLOCK: &str = "block";
pub const BEACON_VOTE: &str = "beacon_vote";
pub const SYNC_CONTRIBUTION_AND_PROOF: &str = "sync_contribution_and_proof";
pub const TIMEOUT: &str = "timeout";
pub const OTHER_ERROR: &str = "other_error";

pub static CONSENSUS_TIMES: LazyLock<Result<HistogramVec>> = LazyLock::new(|| {
    try_create_histogram_vec(
        "anchor_consensus_times_seconds",
        "Duration to come to consensus",
        &["type"],
    )
});

pub static SIGNED_RANDAO_REVEALS_TOTAL: LazyLock<Result<IntCounterVec>> = LazyLock::new(|| {
    try_create_int_counter_vec(
        "vc_signed_randao_reveals_total",
        "Total count of RandaoReveal signings",
        &["status"],
    )
});
