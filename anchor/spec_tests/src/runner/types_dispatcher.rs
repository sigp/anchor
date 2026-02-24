use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{run_test, types};

/// Dispatch a single fixture file to its test type based on the exact prefix before the first `_`.
///
/// Returns `Some(result)` if the prefix matched an implemented test type,
/// or `None` if the fixture was skipped (known-inapplicable or not-yet-implemented).
fn dispatch_fixture_by_prefix(
    prefix: &str,
    path: &Path,
    contents: &str,
    skipped_known_count: &mut usize,
    skipped_unknown_count: &mut usize,
) -> Option<Result<(), String>> {
    match prefix {
        // Encoding tests
        "aggregatorcommitteeconsensusdata.EncodingTest" => Some(run_test::<
            types::AggregatorCommitteeConsensusDataEncodingTest,
        >(path, contents)),
        "beaconvote.EncodingTest" => {
            Some(run_test::<types::BeaconVoteEncodingTest>(path, contents))
        }
        "partialsigmessage.EncodingTest" => Some(run_test::<types::PartialSigMessageEncodingTest>(
            path, contents,
        )),
        "signedssvmsg.EncodingTest" => Some(run_test::<types::SignedSSVMessageEncodingTest>(
            path, contents,
        )),
        "ssvmsg.EncodingTest" => Some(run_test::<types::SSVMessageEncodingTest>(path, contents)),
        "proposerconsensusdata.EncodingTest" => Some(run_test::<
            types::ProposerConsensusDataEncodingTest,
        >(path, contents)),

        // Anchor's `Share` is architecturally different from Go spec's `Share`
        // (different fields, decomposed across multiple types). Not applicable.
        "share.EncodingTest" => {
            eprintln!("SKIP (known-inapplicable): {prefix}");
            *skipped_known_count += 1;
            None
        }

        // TODO(spec-tests): Add more test types here as they are implemented.
        // This arm will be replaced with panic!() once all test types are added.
        _ => {
            eprintln!("SKIP (not yet implemented): {prefix}");
            *skipped_unknown_count += 1;
            None
        }
    }
}

/// Run all type spec tests from the fixture directory.
///
/// Iterates over every `.json` fixture, dispatches each to the appropriate test type,
/// and reports failures at the end.
pub fn run_all_type_fixtures() {
    let dir: PathBuf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("ssv-spec/types/spectest/generate/tests");
    assert!(
        dir.exists(),
        "Fixture directory not found: {}",
        dir.display()
    );

    let mut failures = Vec::new();
    let mut executed_count = 0;
    let mut skipped_known_count = 0;
    let mut skipped_unknown_count = 0;

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

        let Some(result) = dispatch_fixture_by_prefix(
            prefix,
            &path,
            &contents,
            &mut skipped_known_count,
            &mut skipped_unknown_count,
        ) else {
            continue;
        };

        executed_count += 1;
        if let Err(e) = result {
            failures.push(format!("  {filename}: {e}"));
        }
    }

    assert!(
        executed_count > 0,
        "No type spec test fixtures found in {}",
        dir.display()
    );

    eprintln!(
        "{executed_count} executed, {skipped_known_count} skipped (known-inapplicable), {skipped_unknown_count} skipped (not yet implemented)"
    );

    if !failures.is_empty() {
        panic!(
            "\n{} of {executed_count} type spec tests failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
