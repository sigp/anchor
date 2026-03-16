//! Integration test for CLI documentation generator docgen.
//!
//! These tests ensure that the subcommand defintiions specified in the docgen crate match those in
//! the src/main.rs anchor executable. This aims to be lightweight and are part of checks that
//! ensure maintainers are updating project documentation when making changes to the Anchor CLI.

use std::process::Command as StdCommand;

use assert_cmd::Command;
use docgen::construct_anchor_cli_tree;

/// Helper to create an Anchor command for testing
fn anchor_cmd() -> Command {
    let bin_path = assert_cmd::cargo::cargo_bin!("anchor");
    Command::from(StdCommand::new(bin_path))
}

#[test]
fn test_docgen_subcommands_match_anchor_cli() {
    let docgen_cli_tree = construct_anchor_cli_tree();
    let docgen_subcommands = docgen_cli_tree
        .get_subcommands()
        .map(|sc| sc.get_name())
        .collect::<Vec<_>>();

    let anchor_binary_help = anchor_cmd()
        .arg("--help")
        .output()
        .expect("Failed to execute anchor --help");
    let help_content = String::from_utf8(anchor_binary_help.stdout).unwrap();

    for subcommand in &docgen_subcommands {
        assert!(
            help_content.contains(&format!("  {subcommand}\n")),
            "Subcommand '{subcommand}' defined in docgen not found in anchor --help output"
        );
    }
}
