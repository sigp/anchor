use std::sync::LazyLock;

pub use metrics::*;

pub const AGGREGATE_AND_PROOF: &str = "aggregate_and_proof";
pub const AGGREGATOR_COMMITTEE: &str = "aggregator_committee";
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

/// Offset into the slot at which RANDAO pre-consensus finished, before any proposer delay.
///
/// The input for tuning `--proposer-delay-ms`, which only bites when pre-consensus completes before
/// the target. Buckets cover the sub-second range where that is decided, and run past a full slot
/// so an overrun stays resolvable.
pub static RANDAO_REVEAL_COMPLETION_OFFSET: LazyLock<Result<Histogram>> = LazyLock::new(|| {
    try_create_histogram_with_buckets(
        "anchor_randao_reveal_completion_offset_seconds",
        "Time into slot (seconds) when RANDAO pre-consensus completed, before any proposer delay",
        Ok(vec![
            0.05, 0.1, 0.2, 0.3, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 8.0, 12.0, 16.0, 24.0,
        ]),
    )
});

/// Wait applied by the proposer delay, labelled `disabled`, `target_passed`, `waited`, or
/// `clock_unavailable`.
///
/// Recorded after the sleep, so it is the wait taken rather than the one planned. Non-waiting
/// outcomes record zero rather than nothing, so a flat zero is distinguishable from a missing
/// metric. Buckets run to the configured hard maximum.
pub static PROPOSER_DELAY_APPLIED: LazyLock<Result<HistogramVec>> = LazyLock::new(|| {
    try_create_histogram_vec_with_buckets(
        "anchor_proposer_delay_applied_seconds",
        "Wait applied before requesting a beacon block, by proposer delay outcome",
        Ok(vec![
            0.0, 0.05, 0.1, 0.2, 0.3, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0,
        ]),
        &["outcome"],
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

// Labels for `AGGREGATOR_COMMITTEE_PUBLISH_TOTAL` (`success` is `validator_metrics::SUCCESS`).
pub const CONSENSUS_ERROR: &str = "consensus_error";
pub const HTTP_ERROR: &str = "http_error";
pub const NO_AGGREGATES: &str = "no_aggregates";
pub const NO_SIGNATURES: &str = "no_signatures";

/// Outcomes of the Boole+ aggregate publisher, one increment per committee per slot. The labels
/// partition every committee the publisher processed: `success` and `http_error` from the publish
/// attempt, `consensus_error` when the outcome failed or timed out, `no_aggregates` when the
/// decided worklist held contributions only, `no_signatures` when aggregates were decided but no
/// root reached signature quorum in time. All increments live in
/// `AnchorValidatorStore::publish_decided_aggregates`.
pub static AGGREGATOR_COMMITTEE_PUBLISH_TOTAL: LazyLock<Result<IntCounterVec>> =
    LazyLock::new(|| {
        try_create_int_counter_vec(
            "anchor_aggregator_committee_publish_total",
            "Committees processed by the Boole+ aggregate publisher, by outcome",
            &["result"],
        )
    });

pub fn inc_publish_result(result: &str) {
    inc_counter_vec(&AGGREGATOR_COMMITTEE_PUBLISH_TOTAL, &[result]);
}

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
