use std::sync::LazyLock;

pub use metrics::*;

pub static RECONSTRUCTION_FALLBACKS_TOTAL: LazyLock<Result<IntCounter>> = LazyLock::new(|| {
    try_create_int_counter(
        "anchor_signature_collector_reconstruction_fallbacks_total",
        "Failed reconstruction attempts that entered per-share verification",
    )
});
