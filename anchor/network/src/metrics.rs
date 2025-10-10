use std::sync::LazyLock;

use metrics::*;

pub static PEERS_CONNECTED: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge("libp2p_peers", "Count of libp2p peers currently connected")
});

pub static HANDSHAKE_SUCCESSFUL: LazyLock<Result<IntCounter>> = LazyLock::new(|| {
    try_create_int_counter(
        "libp2p_handshake_successful_total",
        "Total count of successful handshakes",
    )
});

pub static HANDSHAKE_FAILED: LazyLock<Result<IntCounterVec>> = LazyLock::new(|| {
    try_create_int_counter_vec(
        "libp2p_handshake_failed_total",
        "Total count of failed handshakes by reason",
        &["reason"],
    )
});

pub static HANDSHAKE_SUBNET_MATCHES: LazyLock<Result<IntGaugeVec>> = LazyLock::new(|| {
    try_create_int_gauge_vec(
        "libp2p_handshake_subnet_matches",
        "Count of successful handshakes by number of matching subnets",
        &["match_count"],
    )
});
