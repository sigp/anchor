use clap::builder::styling::*;
use clap::{builder::ArgPredicate, Arg, ArgAction, Command};
// use clap_utils::{get_color_style, FLAG_HEADER};
use crate::version::VERSION;
use ethereum_hashing::have_sha_extensions;
use std::sync::LazyLock;

pub static SHORT_VERSION: LazyLock<String> = LazyLock::new(|| VERSION.replace("SSV/", ""));
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

fn build_profile_name() -> String {
    // Nice hack from https://stackoverflow.com/questions/73595435/how-to-get-profile-from-cargo-toml-in-build-rs-or-at-runtime
    // The profile name is always the 3rd last part of the path (with 1 based indexing).
    // e.g. /code/core/target/cli/build/my-build-info-9f91ba6f99d7a061/out
    std::env!("OUT_DIR")
        .split(std::path::MAIN_SEPARATOR)
        .nth_back(3)
        .unwrap_or_else(|| "unknown")
        .to_string()
}

pub fn cli_app() -> Command {
    Command::new("ssv")
        .version(SHORT_VERSION.as_str())
        .author("Sigma Prime <contact@sigmaprime.io>")
        .styles(get_color_style())
        .next_line_help(true)
        .term_width(80)
        .disable_help_flag(true)
        .about(
            "SSV Validator client. Maintained by Sigma Prime", 
        )
        .long_version(LONG_VERSION.as_str())
        .display_order(0)
        .arg(
            Arg::new("debug-level")
                .long("debug-level")
                .value_name("LEVEL")
                .help("Specifies the verbosity level used when emitting logs to the terminal.")
                .action(ArgAction::Set)
                .value_parser(["info", "debug", "trace", "warn", "error"])
                .global(true)
                .default_value("info")
                .display_order(0)
        )
        .arg(
            Arg::new("datadir")
                .long("datadir")
                .short('d')
                .value_name("DIR")
                .global(true)
                .help(
                    "Used to specify a custom root data directory for lighthouse keys and databases. \
                    Defaults to $HOME/.lighthouse/{network} where network is the value of the `network` flag \
                    Note: Users should specify separate custom datadirs for different networks.")
                .action(ArgAction::Set)
                .display_order(0)
        )
        /* REST API related arguments */
        .arg(
            Arg::new("http")
                .long("http")
                .help("Enable the RESTful HTTP API server. Disabled by default.")
                .action(ArgAction::SetTrue)
                .help_heading(FLAG_HEADER)
                .display_order(0)
        )
        /*
         * Note: The HTTP server is **not** encrypted (i.e., not HTTPS) and therefore it is
         * unsafe to publish on a public network.
         *
         * If the `--http-address` flag is used, the `--unencrypted-http-transport` flag
         * must also be used in order to make it clear to the user that this is unsafe.
         */
         .arg(
             Arg::new("http-address")
                 .long("http-address")
                 .requires("http")
                 .value_name("ADDRESS")
                 .help("Set the address for the HTTP address. The HTTP server is not encrypted \
                        and therefore it is unsafe to publish on a public network. When this \
                        flag is used, it additionally requires the explicit use of the \
                        `--unencrypted-http-transport` flag to ensure the user is aware of the \
                        risks involved. For access via the Internet, users should apply \
                        transport-layer security like a HTTPS reverse-proxy or SSH tunnelling.")
                .requires("unencrypted-http-transport")
                .display_order(0)
         )
         .arg(
             Arg::new("unencrypted-http-transport")
                .long("unencrypted-http-transport")
                .help("This is a safety flag to ensure that the user is aware that the http \
                    transport is unencrypted and using a custom HTTP address is unsafe.")
                .action(ArgAction::SetTrue)
                .help_heading(FLAG_HEADER)
                .requires("http-address")
                .hide(true)
                .display_order(0)
         )
        .arg(
            Arg::new("http-port")
                .long("http-port")
                .requires("http")
                .value_name("PORT")
                .help("Set the listen TCP port for the RESTful HTTP API server.")
                .default_value_if("http", ArgPredicate::IsPresent, "5062")
                .action(ArgAction::Set)
                .display_order(0)
        )
        .arg(
            Arg::new("http-allow-origin")
                .long("http-allow-origin")
                .requires("http")
                .value_name("ORIGIN")
                .help("Set the value of the Access-Control-Allow-Origin response HTTP header. \
                    Use * to allow any origin (not recommended in production). \
                    If no value is supplied, the CORS allowed origin is set to the listen \
                    address of this server (e.g., http://localhost:5062).")
                .action(ArgAction::Set)
                .display_order(0)
        )
        /* Prometheus metrics HTTP server related arguments */
        .arg(
            Arg::new("metrics")
                .long("metrics")
                .help("Enable the Prometheus metrics HTTP server. Disabled by default.")
                .action(ArgAction::SetTrue)
                .help_heading(FLAG_HEADER)
                .display_order(0)
        )
        .arg(
            Arg::new("metrics-address")
                .long("metrics-address")
                .requires("metrics")
                .value_name("ADDRESS")
                .help("Set the listen address for the Prometheus metrics HTTP server.")
                .default_value_if("metrics", ArgPredicate::IsPresent, "127.0.0.1")
                .action(ArgAction::Set)
                .display_order(0)
        )
        .arg(
            Arg::new("metrics-port")
                .long("metrics-port")
                .requires("metrics")
                .value_name("PORT")
                .help("Set the listen TCP port for the Prometheus metrics HTTP server.")
                .default_value_if("metrics", ArgPredicate::IsPresent, "5064")
                .action(ArgAction::Set)
                .display_order(0)
        )
}

pub fn get_color_style() -> Styles {
    Styles::styled()
        .header(AnsiColor::Yellow.on_default())
        .usage(AnsiColor::Green.on_default())
        .literal(AnsiColor::Green.on_default())
        .placeholder(AnsiColor::Green.on_default())
}
