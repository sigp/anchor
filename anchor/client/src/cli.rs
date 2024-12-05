use clap::builder::styling::*;
use clap::builder::{ArgAction, ArgPredicate};
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use strum::Display;
// use clap_utils::{get_color_style, FLAG_HEADER};
use ethereum_hashing::have_sha_extensions;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::LazyLock;
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

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize, Display, ValueEnum)]
pub enum DebugLevel {
    #[strum(serialize = "info")]
    Info,
    #[strum(serialize = "debug")]
    Debug,
    #[strum(serialize = "trace")]
    Trace,
    #[strum(serialize = "warn")]
    Warn,
    #[strum(serialize = "error")]
    Error,
}

#[derive(Parser, Clone, Deserialize, Serialize, Debug)]
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
pub struct Anchor {
    #[clap(
        long,
        value_name = "LEVEL",
        help = "Specifies the verbosity level used when emitting logs to the terminal.",
        default_value_t = DebugLevel::Info,
        display_order = 0,
    )]
    pub debug_level: DebugLevel,

    #[clap(
        long,
        short = 'd',
        global = true,
        value_name = "DIR",
        help = "Used to specify a custom root data directory for lighthouse keys and databases. \
                Defaults to $HOME/.lighthouse/{network} where network is the value of the `network` flag \
                Note: Users should specify separate custom datadirs for different networks.",
        display_order = 0
    )]
    pub datadir: Option<PathBuf>,

    #[clap(
        long,
        value_name = "DIR",
        help = "The directory which contains the password to unlock the validator \
            voting keypairs. Each password should be contained in a file where the \
            name is the 0x-prefixed hex representation of the validators voting public \
            key. Defaults to ~/.lighthouse/{network}/secrets.",
        conflicts_with = "datadir",
        display_order = 0
    )]
    pub secrets_dir: Option<PathBuf>,

    /* External APIs */
    #[clap(
        long,
        value_name = "NETWORK_ADDRESSES",
        help = "Comma-separated addresses to one or more beacon node HTTP APIs. \
                Default is http://localhost:5052.",
        display_order = 0
    )]
    pub beacon_nodes: Option<Vec<String>>,

    #[clap(
        long,
        value_name = "NETWORK_ADDRESSES",
        help = "Comma-separated addresses to one or more beacon node HTTP APIs. \
                Default is http://localhost:8545.",
        display_order = 0
    )]
    pub execution_nodes: Option<Vec<String>>,

    #[clap(
        long,
        value_name = "CERTIFICATE-FILES",
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
        help = "Comma-separated paths to custom TLS certificates to use when connecting \
                to an exection node. These certificates must be in PEM format and are used \
                in addition to the OS trust store. Commas must only be used as a \
                delimiter, and must not be part of the certificate path",
        display_order = 0
    )]
    pub execution_nodes_tls_certs: Option<Vec<PathBuf>>,

    /* REST API related arguments */
    #[clap(
        long,
        help = "Enable the RESTful HTTP API server. Disabled by default.",
        help_heading = FLAG_HEADER,
        display_order = 0,
    )]
    pub http: bool,

    /*
     * Note: The HTTP server is **not** encrypted (i.e., not HTTPS) and therefore it is
     * unsafe to publish on a public network.
     *
     * If the `--http-address` flag is used, the `--unencrypted-http-transport` flag
     * must also be used in order to make it clear to the user that this is unsafe.
     */
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

    /* Network related arguments */
    #[clap(
        long,
        value_name = "ADDRESS",
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
                      The discovery UDP port will be set to this value and the Quic UDP port will be set to this value + 1. The discovery port can be modified by the \
                      --discovery-port flag and the quic port can be modified by the --quic-port flag. If listening over both IPv4 and IPv6 the --port flag \
                      will apply to the IPv4 address and --port6 to the IPv6 address.",
        default_value = "9100",
        action = ArgAction::Set,
    )]
    pub port: u16,

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
        help = "The UDP port that discovery will listen on. Defaults to `port`",
        action = ArgAction::Set,
    )]
    pub discovery_port: Option<u16>,

    #[clap(
        long,
        value_name = "PORT",
        help = "The UDP port that discovery will listen on over IPv6 if listening over \
                      both IPv4 and IPv6. Defaults to `port6`",
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

    /* Prometheus metrics HTTP server related arguments */
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
    // TODO: Metrics CORS Origin
    #[clap(
        long,
        global = true,
        help = "Prints help information",
        action = clap::ArgAction::HelpLong,
        display_order = 0,
        help_heading = FLAG_HEADER
    )]
    help: Option<bool>,
}

pub fn get_color_style() -> Styles {
    Styles::styled()
        .header(AnsiColor::Yellow.on_default())
        .usage(AnsiColor::Green.on_default())
        .literal(AnsiColor::Green.on_default())
        .placeholder(AnsiColor::Green.on_default())
}
