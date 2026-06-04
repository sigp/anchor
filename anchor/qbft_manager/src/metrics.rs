//! Metrics for proposer QBFT.
#![expect(
    dead_code,
    reason = "Expected to be implemented by proposer QBFT instrumentation"
)]

use std::sync::LazyLock;

pub use metrics::*;

pub static PROPOSER_QBFT_DECIDED_ROUND: LazyLock<Result<Histogram>> = LazyLock::new(|| {
    // 12 buckets created as headroom to avoid unintentional overflow.
    try_create_histogram_with_buckets(
        "anchor_proposer_qbft_decided_round",
        "Round on which proposer QBFT decided or timed out",
        linear_buckets(1.0, 1.0, 12),
    )
});

pub static PROPOSER_QBFT_DURATION_SECONDS: LazyLock<Result<Histogram>> = LazyLock::new(|| {
    try_create_histogram_with_buckets(
        "anchor_proposer_qbft_duration_seconds",
        "Total duration of a proposer QBFT instance",
        Ok(vec![0.5, 1.0, 2.0, 4.0, 6.0, 8.0, 10.0, 15.0, 20.0, 30.0]),
    )
});

pub static PROPOSER_QBFT_HANDOFF_BUDGET_SECONDS: LazyLock<Result<Histogram>> =
    LazyLock::new(|| {
        // Budget is bounded by the slot duration (12s on mainnet).
        try_create_histogram_with_buckets(
            "anchor_proposer_qbft_handoff_budget_seconds",
            "Slot budget remaining when QBFT instance starts (slot_duration - slot_elapsed)",
            Ok(vec![0.5, 1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 10.0, 12.0]),
        )
    });

pub static PROPOSER_QBFT_OUTCOME_TOTAL: LazyLock<Result<IntCounterVec>> = LazyLock::new(|| {
    try_create_int_counter_vec(
        "anchor_proposer_qbft_outcome_total",
        "Count of proposer QBFT instance outcomes",
        &["outcome"],
    )
});

pub static PROPOSER_ROUND_ADVANCE_TOTAL: LazyLock<Result<IntCounterVec>> = LazyLock::new(|| {
    try_create_int_counter_vec(
        "anchor_proposer_round_advance_total",
        "Count of proposer QBFT round advances by reason",
        &["reason"],
    )
});
