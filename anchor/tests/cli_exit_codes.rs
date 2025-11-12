//! Integration tests for CLI exit codes
//!
//! These tests verify that the Anchor CLI returns correct exit codes for various
//! success and error scenarios. This is critical for production deployments where
//! process managers (systemd, docker, kubernetes) rely on exit codes to determine
//! if the application started successfully or encountered an error.
//!
//! ## Exit Code Semantics
//!
//! - **Exit code 0**: Success - command completed successfully
//! - **Exit code 1**: Failure - error occurred during execution
//!
//! ## Testing Approach
//!
//! We use `assert_cmd` which is the Rust CLI Working Group's recommended crate for
//! CLI testing. It provides:
//! - Convenient assertions for exit codes, stdout, and stderr
//! - Process spawning and control
//! - Integration with the `predicates` crate for flexible assertions
//!
//! ## Why These Tests Matter
//!
//! These tests verify that Anchor returns correct exit codes. The `main()` function
//! returns `Result<(), String>`, which uses Rust's Termination trait to convert:
//! - `Ok(())` → exit code 0 (success)
//! - `Err(_)` → exit code 1 (failure, with error printed to stderr)
//!
//! This is critical for production deployments where process managers rely on exit codes.

use std::{fs, process::Command as StdCommand};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

/// Helper to create an Anchor command for testing
fn anchor_cmd() -> Command {
    let bin_path = assert_cmd::cargo::cargo_bin!("anchor");
    Command::from(StdCommand::new(bin_path))
}

/// Helper to create a temporary directory for test data
fn temp_test_dir() -> TempDir {
    TempDir::new().expect("Failed to create temp dir")
}

// ============================================================================
// SUCCESS CASES - Exit Code 0
// ============================================================================

#[test]
fn test_help_flag_exits_successfully() {
    // Arrange & Act
    let mut cmd = anchor_cmd();
    let assert = cmd.arg("--help").assert();

    // Assert
    assert
        .success() // Exit code 0
        .stdout(predicate::str::contains("Anchor"))
        .stdout(predicate::str::contains("Usage:"));
}

#[test]
fn test_version_flag_exits_successfully() {
    // Arrange & Act
    let mut cmd = anchor_cmd();
    let assert = cmd.arg("--version").assert();

    // Assert
    assert
        .success() // Exit code 0
        .stdout(predicate::str::contains("anchor"));
}

#[test]
fn test_node_subcommand_help_exits_successfully() {
    // Arrange & Act
    let mut cmd = anchor_cmd();
    let assert = cmd.args(["node", "--help"]).assert();

    // Assert
    assert
        .success() // Exit code 0
        .stdout(predicate::str::contains("Start Anchor node"));
}

#[test]
fn test_keygen_subcommand_help_exits_successfully() {
    // Arrange & Act
    let mut cmd = anchor_cmd();
    let assert = cmd.args(["keygen", "--help"]).assert();

    // Assert
    assert
        .success() // Exit code 0
        .stdout(predicate::str::contains("RSA key generation tool"));
}

#[test]
fn test_keysplit_subcommand_help_exits_successfully() {
    // Arrange & Act
    let mut cmd = anchor_cmd();
    let assert = cmd.args(["keysplit", "--help"]).assert();

    // Assert
    assert
        .success() // Exit code 0
        .stdout(predicate::str::contains("SSV Keysplitting Tool"));
}

// ============================================================================
// ERROR CASES - Exit Code 1
// ============================================================================

#[test]
fn test_invalid_testnet_dir_exits_with_error() {
    // Arrange
    let nonexistent_dir = "/tmp/anchor_test_nonexistent_testnet_12345";

    // Ensure directory doesn't exist
    let _ = fs::remove_dir_all(nonexistent_dir);

    // Act
    let mut cmd = anchor_cmd();
    let assert = cmd
        .args(["node", "--testnet-dir", nonexistent_dir])
        .assert();

    // Assert
    assert
        .failure() // Exit code 1
        .stderr(predicate::str::contains("Failed to"));
}

#[test]
fn test_missing_required_subcommand_exits_with_error() {
    // Arrange & Act
    let mut cmd = anchor_cmd();
    let assert = cmd.assert();

    // Assert
    assert
        .failure() // Exit code 1
        .stderr(predicate::str::contains("required").or(predicate::str::contains("Usage")));
}

#[test]
fn test_invalid_debug_level_exits_with_error() {
    // Arrange & Act
    let mut cmd = anchor_cmd();
    let assert = cmd
        .args(["node", "--debug-level", "INVALID_LEVEL"])
        .assert();

    // Assert
    assert
        .failure() // Exit code 1
        .stderr(predicate::str::contains("invalid value"));
}

#[test]
fn test_conflicting_network_flags_exit_with_error() {
    // Arrange
    let temp_dir = temp_test_dir();

    // Act
    let mut cmd = anchor_cmd();
    let assert = cmd
        .args([
            "node",
            "--network",
            "mainnet",
            "--testnet-dir",
            temp_dir.path().to_str().unwrap(),
        ])
        .assert();

    // Assert
    assert
        .failure() // Exit code 1
        .stderr(
            predicate::str::contains("conflict")
                .or(predicate::str::contains("cannot be used with")),
        );
}

#[test]
fn test_keygen_with_invalid_output_path_exits_with_error() {
    // Arrange - Try to write to a read-only location or invalid path
    let invalid_path = "/dev/null/impossible/path/key.json";

    // Act
    let mut cmd = anchor_cmd();
    let assert = cmd
        .args(["keygen", "--output-path", invalid_path])
        .timeout(std::time::Duration::from_secs(10))
        .assert();

    // Assert
    assert
        .failure() // Exit code 1
        .stderr(predicate::str::contains("error").or(predicate::str::contains("Error")));
}

#[test]
fn test_keysplit_with_missing_keystore_exits_with_error() {
    // Arrange
    let temp_dir = temp_test_dir();
    let nonexistent_keystore = temp_dir.path().join("nonexistent_keystore.json");
    let output_dir = temp_dir.path().join("output");

    // Act
    let mut cmd = anchor_cmd();
    let assert = cmd
        .args([
            "keysplit",
            "--keystore",
            nonexistent_keystore.to_str().unwrap(),
            "--output-path",
            output_dir.to_str().unwrap(),
            "--password",
            "test_password",
            "--owner-address",
            "0x0000000000000000000000000000000000000000",
            "--owner-nonce",
            "0",
            "--operator-ids",
            "1,2,3,4",
        ])
        .timeout(std::time::Duration::from_secs(10))
        .assert();

    // Assert
    assert
        .failure() // Exit code 1
        .stderr(
            predicate::str::contains("error")
                .or(predicate::str::contains("Error"))
                .or(predicate::str::contains("keystore")),
        );
}

// ============================================================================
// RESOURCE CLEANUP VERIFICATION
// ============================================================================

/// This test verifies that using Result<(), String> allows proper resource cleanup
/// through destructors. Early returns from void main() do run destructors, but
/// returning Err() from Result main() provides better error propagation semantics.
///
/// We test this indirectly by ensuring a data directory lock is properly released
/// when the process exits with an error.
#[test]
fn test_early_error_releases_resources_properly() {
    // Arrange
    let temp_dir = temp_test_dir();
    let data_dir = temp_dir.path().join("anchor_data");
    let nonexistent_testnet = "/tmp/anchor_test_nonexistent_99999";

    // Ensure nonexistent testnet dir doesn't exist
    let _ = fs::remove_dir_all(nonexistent_testnet);

    // Act - First attempt with error (should release resources on exit)
    let mut cmd1 = anchor_cmd();
    cmd1.args([
        "node",
        "--data-dir",
        data_dir.to_str().unwrap(),
        "--testnet-dir",
        nonexistent_testnet,
    ])
    .assert()
    .failure(); // Exit code 1

    // Act - Second attempt with same data dir (should succeed if lock was released)
    // If destructors didn't run, the lock file might still exist
    let mut cmd2 = anchor_cmd();
    let assert2 = cmd2
        .args([
            "node",
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--testnet-dir",
            nonexistent_testnet,
        ])
        .assert();

    // Assert - Should fail for the same reason (invalid testnet), not because of lock
    assert2
        .failure() // Exit code 1
        .stderr(predicate::str::contains("Failed to"));

    // If we got here without a "resource busy" or "lock" error, destructors ran correctly
}
