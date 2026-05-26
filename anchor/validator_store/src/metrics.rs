use std::sync::LazyLock;

pub use metrics::*;

pub const AGGREGATE_AND_PROOF: &str = "aggregate_and_proof";
pub const BLOCK: &str = "block";
pub const BEACON_VOTE: &str = "beacon_vote";
pub const SYNC_CONTRIBUTION_AND_PROOF: &str = "sync_contribution_and_proof";
pub const TIMEOUT: &str = "timeout";
pub const OTHER_ERROR: &str = "other_error";
pub const TRIGGER_HEAD_EVENT: &str = "head_event";
pub const TRIGGER_TIMER: &str = "timer";

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

/// Count of Phase 2 firings, labelled by trigger source.
pub static METADATA_SERVICE_VOTING_CONTEXT_TRIGGERS_TOTAL: LazyLock<Result<IntCounterVec>> =
    LazyLock::new(|| {
        try_create_int_counter_vec(
            "anchor_metadata_service_voting_context_triggers_total",
            "Number of voting context updates by trigger source (head_event or timer)",
            &["trigger"],
        )
    });

/// Offset within the slot at which Phase 2 fired, in seconds.
/// Buckets target the 0–4s window before the spec-derived fallback timer.
pub static METADATA_SERVICE_VOTING_CONTEXT_OFFSET_SECONDS: LazyLock<Result<HistogramVec>> =
    LazyLock::new(|| {
        try_create_histogram_vec_with_buckets(
            "anchor_metadata_service_voting_context_offset_seconds",
            "Time into slot (seconds) when voting context was triggered, by source",
            Ok(vec![0.05, 0.1, 0.25, 0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0]),
            &["trigger"],
        )
    });

// ═══════════════════════════════════════════════════════════════════════════════
// AggregatorCommittee metrics
// ═══════════════════════════════════════════════════════════════════════════════

pub static AGGREGATOR_COMMITTEE_FETCH_TIMES: LazyLock<Result<HistogramVec>> = LazyLock::new(|| {
    try_create_histogram_vec(
        "anchor_aggregator_committee_fetch_times_seconds",
        "Time taken to fetch aggregated attestations and sync contributions",
        &["type"],
    )
});

pub static AGGREGATOR_COMMITTEE_PARTIAL_RESULTS: LazyLock<Result<IntCounterVec>> =
    LazyLock::new(|| {
        try_create_int_counter_vec(
            "anchor_aggregator_committee_partial_results_total",
            "Number of times partial results were returned due to timeout",
            &["type"],
        )
    });

pub static AGGREGATOR_COMMITTEE_FETCH_SUCCESS: LazyLock<Result<IntCounterVec>> =
    LazyLock::new(|| {
        try_create_int_counter_vec(
            "anchor_aggregator_committee_fetch_success_total",
            "Number of successful vs total fetches for aggregator committee data",
            &["type", "status"],
        )
    });

// ═══════════════════════════════════════════════════════════════════════════════
// Weighted Attestation Data (WAD) metrics
// ═══════════════════════════════════════════════════════════════════════════════

/// End-to-end time for the WAD fetch (from start to best-data selection).
pub static WAD_FETCH_TIMES: LazyLock<Result<Histogram>> = LazyLock::new(|| {
    try_create_histogram(
        "anchor_wad_fetch_times_seconds",
        "End-to-end duration of weighted attestation data fetch",
    )
});

/// Count of WAD fetches that returned at soft timeout instead of waiting for all BNs.
pub static WAD_SOFT_TIMEOUT_TOTAL: LazyLock<Result<IntCounter>> = LazyLock::new(|| {
    try_create_int_counter(
        "anchor_wad_soft_timeout_total",
        "Number of WAD fetches that returned at soft timeout with partial responses",
    )
});
