#![cfg(test)]

mod types;
mod utils;

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::de::DeserializeOwned;

/// Core trait for spec tests.
/// Each test type deserializes from JSON and runs assertions.
trait SpecTest: DeserializeOwned {
    fn run(&self) -> Result<(), String>;
}

/// Generic test runner. Deserializes JSON into concrete type `T`, then runs the test.
fn run_test<T: SpecTest>(path: &Path, contents: &str) -> Result<(), String> {
    let test: T = serde_json::from_str(contents)
        .map_err(|e| format!("Failed to parse {}: {e}", path.display()))?;
    test.run()
}

/// Run all type spec tests from the fixture directory.
/// Dispatches each JSON file to the correct test type based on exact prefix match.
fn run_types_tests() {
    let dir: PathBuf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("ssv-spec/types/spectest/generate/tests");
    assert!(
        dir.exists(),
        "Fixture directory not found: {}",
        dir.display()
    );

    let mut failures = Vec::new();
    let mut count = 0;

    for entry in fs::read_dir(&dir).expect("Failed to read fixture directory") {
        let entry = entry.expect("Failed to read directory entry");
        let path = entry.path();

        if path.extension() != Some("json".as_ref()) {
            continue;
        }

        let filename = path.file_name().unwrap().to_string_lossy().to_string();
        let contents = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()));

        // Extract exact type prefix
        let prefix = filename.split('_').next().unwrap_or("");

        let result = match prefix {
            // Encoding tests
            "aggregatorcommitteeconsensusdata.EncodingTest" => {
                run_test::<types::AggregatorCommitteeConsensusDataEncodingTest>(&path, &contents)
            }
            "beaconvote.EncodingTest" => {
                run_test::<types::BeaconVoteEncodingTest>(&path, &contents)
            }
            "partialsigmessage.EncodingTest" => {
                run_test::<types::PartialSigMessageEncodingTest>(&path, &contents)
            }
            "signedssvmsg.EncodingTest" => {
                run_test::<types::SignedSSVMessageEncodingTest>(&path, &contents)
            }
            "ssvmsg.EncodingTest" => run_test::<types::SSVMessageEncodingTest>(&path, &contents),

            // TODO(spec-tests): Add more test types here as they are implemented.
            // This arm will be replaced with panic!() once all test types are added.
            _ => {
                eprintln!("SKIP (not yet implemented): {prefix}");
                continue;
            }
        };

        count += 1;
        if let Err(e) = result {
            failures.push(format!("  {filename}: {e}"));
        }
    }

    assert!(
        count > 0,
        "No type spec test fixtures found in {}",
        dir.display()
    );

    if !failures.is_empty() {
        panic!(
            "\n{} of {count} type spec tests failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

#[cfg(test)]
mod spec_tests {
    use super::*;

    #[test]
    fn types_spec_tests() {
        run_types_tests();
    }
}
