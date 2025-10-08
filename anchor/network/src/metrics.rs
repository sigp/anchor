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
