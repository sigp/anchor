use std::sync::LazyLock;

use metrics::*;

pub static PEERS_CONNECTED: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge("libp2p_peers", "Count of libp2p peers currently connected")
});
<<<<<<< HEAD
=======

pub static PEERS_BLOCKED_LOW_SCORE: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "libp2p_blocked_peers_low_score",
        "Count of peers blocked due to low gossipsub score",
    )
});

pub static PEERS_BLOCKED_FAILED_HANDSHAKE: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "libp2p_blocked_peers_failed_handshake",
        "Count of peers blocked due to failed handshake",
    )
});

pub static PEERS_BLOCKED_OUTGOING_CONNECTION_ERROR: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "libp2p_blocked_peers_outgoing_connection_error",
        "Count of peers blocked due to outgoing connection errors",
    )
});

pub static PEERS_BLOCKED_INCOMING_CONNECTION_ERROR: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "libp2p_blocked_peers_incoming_connection_error",
        "Count of peers blocked due to incoming connection errors",
    )
});

pub static PEERS_BLOCKED_OTHER: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "libp2p_blocked_peers_other",
        "Count of peers blocked for other reasons",
    )
});

pub static HANDSHAKE_SUCCESSFUL: LazyLock<Result<IntCounter>> = LazyLock::new(|| {
    try_create_int_counter(
        "libp2p_handshake_successful_total",
        "Total count of successful handshakes",
    )
});

pub static HANDSHAKE_FAILED: LazyLock<Result<IntCounter>> = LazyLock::new(|| {
    try_create_int_counter(
        "libp2p_handshake_failed_total",
        "Total count of failed handshakes",
    )
});

pub static HANDSHAKE_SUBNET_MATCHES: LazyLock<Result<IntGaugeVec>> = LazyLock::new(|| {
    try_create_int_gauge_vec(
        "libp2p_handshake_subnet_matches",
        "Count of successful handshakes by number of matching subnets",
        &["match_count"],
    )
});
>>>>>>> a40904f9 (feat: calculate subnets for handshake)
