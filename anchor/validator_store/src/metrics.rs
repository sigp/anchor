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

// ═══════════════════════════════════════════════════════════════════════════════
// PTC (Payload Timeliness Committee) metrics
// ═══════════════════════════════════════════════════════════════════════════════

/// The committee never reached the partial signature threshold. At the current collector
/// granularity this also covers genuine channel closes, so it is an upper bound on the true
/// observation-divergence rate.
pub const PTC_FAILURE_NO_SIGNATURE: &str = "no_signature";
/// Local collection or reconstruction infrastructure fault.
pub const PTC_FAILURE_INFRA: &str = "infra";

pub static PTC_RECONSTRUCTION_FAILURES: LazyLock<Result<IntCounterVec>> = LazyLock::new(|| {
    try_create_int_counter_vec(
        "anchor_ptc_reconstruction_failures_total",
        "Payload attestation signature collection failures by reason",
        &["reason"],
    )
});
