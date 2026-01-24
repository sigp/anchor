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

// ═══════════════════════════════════════════════════════════════════════════════
// MetadataService metrics
// ═══════════════════════════════════════════════════════════════════════════════

// Poll outcome labels
pub const ATTESTERS: &str = "attesters";
pub const SYNC: &str = "sync";
pub const SUCCESS: &str = "success";
pub const FAILED: &str = "failed";

/// Count of poll attempts by type (attesters/sync) and outcome (success/failed)
pub static METADATA_SERVICE_POLL_TOTAL: LazyLock<Result<IntCounterVec>> = LazyLock::new(|| {
    try_create_int_counter_vec(
        "anchor_metadata_service_poll_total",
        "Count of DutiesService poll wait attempts",
        &["type", "outcome"],
    )
});

/// Duration of poll wait phase (both polls in parallel)
pub static METADATA_SERVICE_POLL_DURATION: LazyLock<Result<Histogram>> = LazyLock::new(|| {
    try_create_histogram(
        "anchor_metadata_service_poll_duration_seconds",
        "Duration waiting for DutiesService poll signals",
    )
});

/// Current count of attesting validators in VotingAssignments
pub static METADATA_SERVICE_ATTESTING_VALIDATORS: LazyLock<Result<IntGauge>> =
    LazyLock::new(|| {
        try_create_int_gauge(
            "anchor_metadata_service_attesting_validators",
            "Count of validators with attestation duties this slot",
        )
    });

/// Current count of sync committee validators in VotingAssignments
pub static METADATA_SERVICE_SYNC_VALIDATORS: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "anchor_metadata_service_sync_validators",
        "Count of validators with sync committee duties this slot",
    )
});

/// Count of slots where VotingAssignments was empty
pub static METADATA_SERVICE_EMPTY_ASSIGNMENTS_TOTAL: LazyLock<Result<IntCounter>> =
    LazyLock::new(|| {
        try_create_int_counter(
            "anchor_metadata_service_empty_assignments_total",
            "Count of slots where VotingAssignments had no duties",
        )
    });
