use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    num::NonZeroU16,
    path::PathBuf,
    sync::LazyLock,
};

use clap::{
    Parser,
    builder::{ArgAction, ArgPredicate, styling::*},
};
use ethereum_hashing::have_sha_extensions;
use logging::FileLoggingFlags;
use version::VERSION;

pub static SHORT_VERSION: LazyLock<String> = LazyLock::new(|| VERSION.replace("Anchor/", ""));
pub static LONG_VERSION: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{}\n\
         SHA256 hardware acceleration: {}\n\
         Allocator: {}\n\
         Profile: {}",
        SHORT_VERSION.as_str(),
        have_sha_extensions(),
        allocator_name(),
        build_profile_name(),
    )
});

pub const FLAG_HEADER: &str = "Flags";

fn allocator_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "system"
    } else {
        "jemalloc"
    }
}

fn build_profile_name() -> &'static str {
    // Nice hack from https://stackoverflow.com/questions/73595435/how-to-get-profile-from-cargo-toml-in-build-rs-or-at-runtime
    // The profile name is always the 3rd last part of the path (with 1 based indexing).
    // e.g. /code/core/target/cli/build/my-build-info-9f91ba6f99d7a061/out
    env!("OUT_DIR")
        .split(std::path::MAIN_SEPARATOR)
        .nth_back(3)
        .unwrap_or("unknown")
}

#[derive(Parser, Clone, Debug)]
#[clap(
    name = "ssv",
    about = "SSV Validator client. Maintained by Sigma Prime.",
    author = "Sigma Prime <contact@sigmaprime.io>",
    long_version = LONG_VERSION.as_str(),
    version = SHORT_VERSION.as_str(),
    styles = get_color_style(),
    disable_help_flag = true,
    next_line_help = true,
    term_width = 80,
    display_order = 0,
)]
pub struct Node {
    #[clap(
        long,
        global = true,
        value_name = "PATH",
        help = "Path to the operator key file. File name needs to end in \
                `.txt` for unencrypted keys, or `.json` for encrypted keys. \
                If not provided, Anchor will look for the key in the data dir. \
                If provided and the file does not exist, Anchor will exit.",
        display_order = 0
    )]
    pub key_file: Option<PathBuf>,

    #[clap(
        long,
        global = true,
        value_name = "PATH",
        help = "Path to the password used to decrypt the operator private key. \
                If not provided but required, Anchor will request the password interactively.",
        display_order = 0
    )]
    pub password_file: Option<PathBuf>,

    // External APIs
    #[clap(
        long,
        value_name = "NETWORK_ADDRESSES",
        value_delimiter = ',',
        help = "Comma-separated addresses to one or more beacon node HTTP APIs. \
                Default is http://localhost:5052.",
        display_order = 0
    )]
    pub beacon_nodes: Option<Vec<String>>,

    #[clap(
        long,
        value_name = "NETWORK_ADDRESSES",
        value_delimiter = ',',
        help = "Comma-separated addresses to one or more execution node JSON-RPC APIs. \
                Default is http://localhost:8545.",
        display_order = 0
    )]
    pub execution_rpc: Option<Vec<String>>,

    #[clap(
        long,
        value_name = "NETWORK_ADDRESSES",
        value_delimiter = ',',
        help = "Address of execution node WS API. \
                Default is ws://localhost:8546.",
        display_order = 0
    )]
    pub execution_ws: Option<String>,

    #[clap(
        long,
        value_name = "CERTIFICATE-FILES",
        value_delimiter = ',',
        help = "Comma-separated paths to custom TLS certificates to use when connecting \
                to a beacon node (and/or proposer node). These certificates must be in PEM format and are used \
                in addition to the OS trust store. Commas must only be used as a \
                delimiter, and must not be part of the certificate path.",
        display_order = 0
    )]
    pub beacon_nodes_tls_certs: Option<Vec<PathBuf>>,

    #[clap(
        long,
        value_name = "CERTIFICATE-FILES",
        value_delimiter = ',',
        help = "Comma-separated paths to custom TLS certificates to use when connecting \
                to an exection node. These certificates must be in PEM format and are used \
                in addition to the OS trust store. Commas must only be used as a \
                delimiter, and must not be part of the certificate path",
        display_order = 0
    )]
    pub execution_nodes_tls_certs: Option<Vec<PathBuf>>,

    // REST API related arguments
    #[clap(
        long,
        help = "Enable the RESTful HTTP API server. Disabled by default.",
        help_heading = FLAG_HEADER,
        display_order = 0,
    )]
    pub http: bool,

    // Note: The HTTP server is **not** encrypted (i.e., not HTTPS) and therefore it is
    // unsafe to publish on a public network.
    //
    // If the `--http-address` flag is used, the `--unencrypted-http-transport` flag
    // must also be used in order to make it clear to the user that this is unsafe.
    #[clap(
        long,
        value_name = "ADDRESS",
        help = "Set the address for the HTTP address. The HTTP server is not encrypted \
                and therefore it is unsafe to publish on a public network. When this \
                flag is used, it additionally requires the explicit use of the \
                `--unencrypted-http-transport` flag to ensure the user is aware of the \
                risks involved. For access via the Internet, users should apply \
                transport-layer security like a HTTPS reverse-proxy or SSH tunnelling.",
        display_order = 0,
        requires = "http",
        requires = "unencrypted_http_transport"
    )]
    pub http_address: Option<IpAddr>,

    #[clap(
        long,
        help = "This is a safety flag to ensure that the user is aware that the http \
                transport is unencrypted and using a custom HTTP address is unsafe.",
        display_order = 0,
        requires = "http_address",
        help_heading = FLAG_HEADER,
    )]
    pub unencrypted_http_transport: bool,

    #[clap(
        long,
        value_name = "PORT",
        requires = "http",
        help = "Set the listen TCP port for the RESTful HTTP API server.",
        display_order = 0,
        default_value_if("http", ArgPredicate::IsPresent, "5062")
    )]
    pub http_port: Option<u16>,

    #[clap(
        long,
        value_name = "ORIGIN",
        help = "Set the value of the Access-Control-Allow-Origin response HTTP header. \
                Use * to allow any origin (not recommended in production). \
                If no value is supplied, the CORS allowed origin is set to the listen \
                address of this server (e.g., http://localhost:5062).",
        display_order = 0,
        requires = "http"
    )]
    pub http_allow_origin: Option<String>,

    // Network related arguments
    #[clap(
        long,
        value_name = "ADDRESS",
        value_delimiter = ',',
        help = "The address anchor will listen for UDP and TCP connections. To listen \
                      over IpV4 and IpV6 set this flag twice with the different values.\n\
                      Examples:\n\
                      - --listen-addresses '0.0.0.0' will listen over IPv4.\n\
                      - --listen-addresses '::' will listen over IPv6.\n\
                      - --listen-addresses '0.0.0.0' --listen-addresses '::' will listen over both \
                      IPv4 and IPv6. The order of the given addresses is not relevant. However, \
                      multiple IPv4, or multiple IPv6 addresses will not be accepted.",
        num_args(0..=2),
        action = ArgAction::Append,
        default_value = "0.0.0.0",
    )]
    pub listen_addresses: Vec<IpAddr>,

    #[clap(
        long,
        value_name = "PORT",
        help = "The TCP/UDP ports to listen on. There are two UDP ports. \
                      The discovery UDP and TCP port will be set to this value. The Quic UDP port will be set to this value + 1. The discovery port can be modified by the \
                      --discovery-port flag and the quic port can be modified by the --quic-port flag. If listening over both IPv4 and IPv6 the --port flag \
                      will apply to the IPv4 address and --port6 to the IPv6 address. If this flag is not set, the default values will be 12001 for discovery and 13001 for TCP.",
        action = ArgAction::Set,
    )]
    pub port: Option<u16>,

    #[clap(
        long,
        value_name = "PORT",
        help = "The TCP/UDP ports to listen on over IPv6 when listening over both IPv4 and \
                      IPv6. The Quic UDP port will be set to this value + 1.",
        action = ArgAction::Set,
    )]
    pub port6: Option<u16>,

    #[clap(
        long,
        value_name = "PORT",
        help = "The UDP port that discovery will listen on. Defaults to --port if --port is explicitly specified, and `12001` otherwise.",
        action = ArgAction::Set,
    )]
    pub discovery_port: Option<u16>,

    #[clap(
        long,
        value_name = "PORT",
        help = "The UDP port that discovery will listen on over IPv6 if listening over \
                      both IPv4 and IPv6. Defaults to `discovery_port`",
        action = ArgAction::Set,
    )]
    pub discovery_port6: Option<u16>,

    #[clap(
        long,
        value_name = "PORT",
        help = "The UDP port that quic will listen on. Defaults to `port` + 1",
        action = ArgAction::Set,
    )]
    pub quic_port: Option<u16>,

    #[clap(
        long,
        value_name = "PORT",
        help = "The UDP port that quic will listen on over IPv6 if listening over \
                      both IPv4 and IPv6. Defaults to `port6` + 1",
        action = ArgAction::Set,
    )]
    pub quic_port6: Option<u16>,

    #[clap(
        long,
        help = "Sets all listening TCP/UDP ports to 0, allowing the OS to choose some \
                       arbitrary free ports.",
        action = ArgAction::SetTrue,
        hide = true,
    )]
    pub use_zero_ports: bool,

    // Prometheus metrics HTTP server related arguments
    #[clap(
        long,
        help = "Enable the Prometheus metrics HTTP server. Disabled by default.",
        display_order = 0,
        help_heading = FLAG_HEADER,
    )]
    pub metrics: bool,

    #[clap(
        long,
        value_name = "ADDRESS",
        help = "Set the listen address for the Prometheus metrics HTTP server.",
        default_value_if("metrics", ArgPredicate::IsPresent, "127.0.0.1"),
        display_order = 0,
        requires = "metrics"
    )]
    pub metrics_address: Option<IpAddr>,

    #[clap(
        long,
        value_name = "PORT",
        help = "Set the listen TCP port for the Prometheus metrics HTTP server.",
        display_order = 0,
        default_value_if("metrics", ArgPredicate::IsPresent, "5164"),
        requires = "metrics"
    )]
    pub metrics_port: Option<u16>,

    #[clap(
        long,
        help = "Enable per validator metrics for > 64 validators. \
                Note: This flag is automatically enabled for <= 64 validators. \
                Enabling this metric for higher validator counts will lead to higher volume \
                of prometheus metrics being collected.",
        display_order = 0,
        help_heading = FLAG_HEADER
    )]
    pub enable_high_validator_count_metrics: bool,
    // TODO: Metrics CORS Origin
    // https://github.com/sigp/anchor/issues/249
    #[clap(
        long,
        global = true,
        help = "Prints help information",
        action = clap::ArgAction::HelpLong,
        display_order = 0,
        help_heading = FLAG_HEADER
    )]
    help: Option<bool>,

    #[clap(
        long,
        global = true,
        value_delimiter = ',',
        help = "One or more comma-delimited ENRs or Multiaddrs to bootstrap the p2p network",
        display_order = 0
    )]
    pub boot_nodes: Vec<String>,

    #[clap(
        long,
        value_name = "ADDRESS",
        global = true,
        help = "The IPv4 address to broadcast to other peers on how to reach \
                      this node. Set this only if you are sure other nodes can connect to your \
                      local node on this address. This will update the `ip4` ENR field accordingly.",
        display_order = 0
    )]
    pub enr_address: Option<Ipv4Addr>,

    #[clap(
        long,
        value_name = "ADDRESS",
        global = true,
        help = "The IPv6 address to broadcast to other peers on how to reach \
                      this node. Set this only if you are sure other nodes can connect to your \
                      local node on this address. This will update the `ip6` ENR field accordingly.",
        display_order = 0
    )]
    pub enr_address6: Option<Ipv6Addr>,

    #[clap(
        long,
        value_name = "PORT",
        global = true,
        help = "The UDP4 port of the local ENR. Set this only if you are sure other nodes \
                      can connect to your local node on this port over IPv4.",
        display_order = 0
    )]
    pub enr_udp_port: Option<NonZeroU16>,

    #[clap(
        long,
        value_name = "PORT",
        global = true,
        help = "The TCP4 port of the local ENR. Set this only if you are sure other nodes \
                      can connect to your local node on this port over IPv4. The --port flag is \
                      used if this is not set.",
        display_order = 0
    )]
    pub enr_tcp_port: Option<NonZeroU16>,

    #[clap(
        long,
        value_name = "PORT",
        global = true,
        help = "The quic UDP4 port that will be set on the local ENR. Set this only if you are sure other nodes \
                      can connect to your local node on this port over IPv4.",
        display_order = 0
    )]
    pub enr_quic_port: Option<NonZeroU16>,

    #[clap(
        long,
        value_name = "PORT",
        global = true,
        help = "The UDP6 port of the local ENR. Set this only if you are sure other nodes \
                      can connect to your local node on this port over IPv6.",
        display_order = 0
    )]
    pub enr_udp6_port: Option<NonZeroU16>,

    #[clap(
        long,
        value_name = "PORT",
        global = true,
        help = "The TCP6 port of the local ENR. Set this only if you are sure other nodes \
                      can connect to your local node on this port over IPv6. The --port6 flag is \
                      used if this is not set.",
        display_order = 0
    )]
    pub enr_tcp6_port: Option<NonZeroU16>,

    #[clap(
        long,
        value_name = "PORT",
        global = true,
        help = "The quic UDP6 port that will be set on the local ENR. Set this only if you are sure other nodes \
                      can connect to your local node on this port over IPv6.",
        display_order = 0
    )]
    pub enr_quic6_port: Option<NonZeroU16>,

    #[clap(
        long,
        global = true,
        help = "Discovery can automatically discover external addresses if the node has correctly set up port forwards.\
                It will automatically update this nodes ENR with values it finds. This can have undesired effects for complicated networks.\
                Setting this flag will disable discovery from updating the ENR from CLI set values.",
        display_order = 0
    )]
    pub disable_enr_auto_update: bool,

    #[clap(
        long,
        help = "Subscribe to all subnets, regardless of committee membership.",
        display_order = 0,
        help_heading = FLAG_HEADER,
    )]
    pub subscribe_all_subnets: bool,

    #[clap(
        long,
        help = "Disable slashing protection for all validator clients. DO NOT ENABLE THIS UNLESS YOU HAVE A MORE THAN SUFFICIENT REASON TO",
        hide = true,
        display_order = 0
    )]
    pub disable_slashing_protection: bool,

    // debugging stuff
    #[clap(
        long,
        hide = true,
        help = "Act as if we were a certain operator, except for sending messages."
    )]
    pub impostor: Option<u64>,

    // Performance options
    #[clap(
        long,
        help = "The number of maximum concurrent workers. Defaults to logical cores.",
        hide = true,
        display_order = 0
    )]
    pub max_workers: Option<usize>,

    #[clap(
        long,
        value_delimiter = ',',
        help = "Override size for a specific queue. Needs to be of the format \"queue_name=42\".",
        hide = true,
        display_order = 0
    )]
    pub work_queue_size: Vec<String>,

    #[clap(
        long,
        value_name = "INTEGER",
        default_value_t = 36_000_000,
        requires = "builder_proposals",
        help = "The gas limit to be used in all builder proposals for all validators managed. \
                Note this will not necessarily be used if the gas limit \
                set here moves too far from the previous block's gas limit.",
        display_order = 0
    )]
    pub gas_limit: u64,

    #[clap(
        long,
        alias = "private-tx-proposals",
        help = "If this flag is set, Anchor will query the Beacon Node for only block \
                headers during proposals and will sign over headers. Useful for outsourcing \
                execution payload construction during proposals.",
        display_order = 0,
        help_heading = FLAG_HEADER
    )]
    pub builder_proposals: bool,

    #[clap(
        long,
        value_name = "UINT64",
        help = "Defines the boost factor, \
                a percentage multiplier to apply to the builder's payload value \
                when choosing between a builder payload header and payload from \
                the local execution node.",
        conflicts_with = "prefer_builder_proposals",
        display_order = 0
    )]
    pub builder_boost_factor: Option<u64>,

    #[clap(
        long,
        help = "If this flag is set, Anchor will always prefer blocks \
                constructed by builders, regardless of payload value.",
        display_order = 0,
        help_heading = FLAG_HEADER
    )]
    pub prefer_builder_proposals: bool,

    #[clap(
        long,
        help = "Disable the latency measurement service.",
        display_order = 0,
        help_heading = FLAG_HEADER
    )]
    pub disable_latency_measurement_service: bool,

    #[clap(
        long,
        help = "Disables gossipsub peer scoring.",
        display_order = 0,
        help_heading = FLAG_HEADER
    )]
    pub disable_gossipsub_peer_scoring: bool,

    #[clap(long, help = "Disables gossipsub topic scoring.", hide = true)]
    pub disable_gossipsub_topic_scoring: bool,

    #[clap(flatten)]
    pub logging_flags: FileLoggingFlags,
}

pub fn get_color_style() -> Styles {
    Styles::styled()
        .header(AnsiColor::Yellow.on_default())
        .usage(AnsiColor::Green.on_default())
        .literal(AnsiColor::Green.on_default())
        .placeholder(AnsiColor::Green.on_default())
}
