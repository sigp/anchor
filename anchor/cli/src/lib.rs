use std::sync::LazyLock;

use clap::{
    Parser,
    builder::{Styles, styling::AnsiColor},
};
pub use cli::{NetworkOptions, Node};
use ethereum_hashing::have_sha_extensions;
use global_config::GlobalFlags;
use keygen::Keygen;
use keysplit::Keysplit;
use version::VERSION;

mod cli;

pub static SHORT_VERSION: LazyLock<String> = LazyLock::new(|| VERSION.replace("Anchor/", ""));
pub static LONG_VERSION: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{}\n\
         SHA256 hardware acceleration: {}\n\
         Profile: {}",
        SHORT_VERSION.as_str(),
        have_sha_extensions(),
        build_profile_name(),
    )
});

fn build_profile_name() -> &'static str {
    // Nice hack from https://stackoverflow.com/questions/73595435/how-to-get-profile-from-cargo-toml-in-build-rs-or-at-runtime
    // The profile name is always the 3rd last part of the path (with 1 based indexing).
    // e.g. /code/core/target/cli/build/my-build-info-9f91ba6f99d7a061/out
    env!("OUT_DIR")
        .split(std::path::MAIN_SEPARATOR)
        .nth_back(3)
        .unwrap_or("unknown")
}

pub fn get_color_style() -> Styles {
    Styles::styled()
        .header(AnsiColor::Yellow.on_default())
        .usage(AnsiColor::Green.on_default())
        .literal(AnsiColor::Green.on_default())
        .placeholder(AnsiColor::Green.on_default())
}

#[derive(Parser, Clone, Debug)]
#[clap(
    name = "anchor",
    about = "SSV Validator client. Maintained by Sigma Prime.",
    author = "Sigma Prime <contact@sigmaprime.io>",
    long_version = LONG_VERSION.as_str(),
    version = SHORT_VERSION.as_str(),
    styles = get_color_style(),
    next_line_help = true,
    term_width = 80,
    display_order = 0,
)]
pub struct Cli {
    #[clap(flatten)]
    pub global_flags: GlobalFlags,

    #[clap(subcommand)]
    pub subcommand: AnchorSubcommands,
}

#[derive(Parser, Clone, Debug)]
pub enum AnchorSubcommands {
    Node(Box<Node>),
    Keysplit(Keysplit),
    Keygen(Keygen),
}
