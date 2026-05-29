//! Metrics for proposer QBFT.
#![expect(
    dead_code,
    reason = "Expected to be implemented by proposer QBFT instrumentation"
)]

use std::sync::LazyLock;

pub use metrics::*;

pub static PROPOSER_QBFT_DECIDED_ROUND: LazyLock<Result<Histogram>> = LazyLock::new(|| {
    try_create_histogram(
        "anchor_proposer_qbft_decided_round",
        "Round on which proposer QBFT decided or timed out",
    )
});

pub static PROPOSER_QBFT_DURATION_SECONDS: LazyLock<Result<Histogram>> = LazyLock::new(|| {
    try_create_histogram(
        "anchor_proposer_qbft_duration_seconds",
        "Total duration of a proposer QBFT instance",
    )
});

pub static PROPOSER_QBFT_HANDOFF_BUDGET_SECONDS: LazyLock<Result<Histogram>> =
    LazyLock::new(|| {
        try_create_histogram(
            "anchor_proposer_qbft_handoff_budget_seconds",
            "Slot budget remaining when QBFT instance starts (slot_duration - slot_elapsed)",
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
