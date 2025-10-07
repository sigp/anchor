use std::{collections::HashSet, sync::LazyLock};

use tracing_log::NormalizeEvent;

use crate::utils::{LIGHTHOUSE_CRATES, WORKSPACE_CRATES};

// Global metrics counters
pub static INFOS_TOTAL: LazyLock<metrics::Result<metrics::IntCounter>> = LazyLock::new(|| {
    metrics::try_create_int_counter(
        "info_total",
        "Total count of info logs across all dependencies",
    )
});

pub static WARNS_TOTAL: LazyLock<metrics::Result<metrics::IntCounter>> = LazyLock::new(|| {
    metrics::try_create_int_counter(
        "warn_total",
        "Total count of warn logs across all dependencies",
    )
});

pub static ERRORS_TOTAL: LazyLock<metrics::Result<metrics::IntCounter>> = LazyLock::new(|| {
    metrics::try_create_int_counter(
        "error_total",
        "Total count of error logs across all dependencies",
    )
});

// Dependency-specific metrics
pub static DEP_INFOS_TOTAL: LazyLock<metrics::Result<metrics::IntCounterVec>> =
    LazyLock::new(|| {
        metrics::try_create_int_counter_vec(
            "dep_info_total",
            "Count of infos logged per enabled dependency",
            &["target"],
        )
    });

pub static DEP_WARNS_TOTAL: LazyLock<metrics::Result<metrics::IntCounterVec>> =
    LazyLock::new(|| {
        metrics::try_create_int_counter_vec(
            "dep_warn_total",
            "Count of warns logged per enabled dependency",
            &["target"],
        )
    });

pub static DEP_ERRORS_TOTAL: LazyLock<metrics::Result<metrics::IntCounterVec>> =
    LazyLock::new(|| {
        metrics::try_create_int_counter_vec(
            "dep_error_total",
            "Count of errors logged per enabled dependency",
            &["target"],
        )
    });

// Count layer implementation
pub struct CountLayer;
impl<S: tracing_core::Subscriber> tracing_subscriber::layer::Layer<S> for CountLayer {
    fn on_event(
        &self,
        event: &tracing_core::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        // get the event's normalized metadata
        let normalized_meta = event.normalized_metadata();
        let meta = normalized_meta.as_ref().unwrap_or_else(|| event.metadata());
        if !meta.is_event() {
            // ignore tracing span events
            return;
        }

        // Obtain the target
        let full_target = meta.module_path().unwrap_or_else(|| meta.target());
        let target = full_target
            .split_once("::")
            .map(|(name, _rest)| name)
            .unwrap_or(full_target);

        let mut workspace_crates: HashSet<&str> = WORKSPACE_CRATES.iter().copied().collect();
        workspace_crates.extend(LIGHTHOUSE_CRATES.iter().copied());

        // Global counters should only include Anchor crates.
        if workspace_crates.contains(target) {
            match *meta.level() {
                tracing_core::Level::INFO => metrics::inc_counter(&INFOS_TOTAL),
                tracing_core::Level::WARN => metrics::inc_counter(&WARNS_TOTAL),
                tracing_core::Level::ERROR => metrics::inc_counter(&ERRORS_TOTAL),
                _ => {}
            }
        }

        // Record only relevant dependency logs
        if ["libp2p", "libp2p2_gossipsub", "discv5"].contains(&target) {
            let target = &[target];
            match *meta.level() {
                tracing_core::Level::INFO => metrics::inc_counter_vec(&DEP_INFOS_TOTAL, target),
                tracing_core::Level::WARN => metrics::inc_counter_vec(&DEP_WARNS_TOTAL, target),
                tracing_core::Level::ERROR => metrics::inc_counter_vec(&DEP_ERRORS_TOTAL, target),
                _ => {}
            }
        }
    }
}
