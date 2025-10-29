use std::sync::LazyLock;

use metrics::*;

pub static PEERS_CONNECTED: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge("libp2p_peers", "Count of libp2p peers currently connected")
});

pub static PEERS_BY_CLIENT: LazyLock<Result<IntGaugeVec>> = LazyLock::new(|| {
    try_create_int_gauge_vec(
        "libp2p_peers_by_client",
        "Count of connected peers by client type (anchor, go-ssv, unknown)",
        &["client_type"],
    )
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

pub static PEERS_BLOCKED: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "libp2p_peers_blocked",
        "Current count of blocked libp2p peers",
    )
});

pub static PEER_BLOCKED_INBOUND_CONNECTIONS: LazyLock<Result<IntGaugeVec>> = LazyLock::new(|| {
    try_create_int_gauge_vec(
        "libp2p_blocked_peer_connection_attempts",
        "Count of blocked peers trying to reconnect",
        &["blocked_peer_id"],
    )
});
