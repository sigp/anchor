use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{run_test, types};

/// Outcome of dispatching a single fixture to its test type.
enum DispatchOutcome {
    Executed(Result<(), String>),
    SkippedKnown,
    SkippedUnknown,
}

/// Dispatch a single fixture file to its test type based on the exact prefix before the first `_`.
fn dispatch_fixture_by_prefix(prefix: &str, path: &Path, contents: &str) -> DispatchOutcome {
    match prefix {
        // Encoding tests
        "aggregatorcommitteeconsensusdata.EncodingTest" => {
            DispatchOutcome::Executed(run_test::<
                types::AggregatorCommitteeConsensusDataEncodingTest,
            >(path, contents))
        }
        "beaconvote.EncodingTest" => {
            DispatchOutcome::Executed(run_test::<types::BeaconVoteEncodingTest>(path, contents))
        }
        "partialsigmessage.EncodingTest" => DispatchOutcome::Executed(run_test::<
            types::PartialSigMessageEncodingTest,
        >(path, contents)),
        "signedssvmsg.EncodingTest" => DispatchOutcome::Executed(run_test::<
            types::SignedSSVMessageEncodingTest,
        >(path, contents)),
        "ssvmsg.EncodingTest" => {
            DispatchOutcome::Executed(run_test::<types::SSVMessageEncodingTest>(path, contents))
        }
        "proposerconsensusdata.EncodingTest" => DispatchOutcome::Executed(run_test::<
            types::ProposerConsensusDataEncodingTest,
        >(path, contents)),

        // Anchor's `Share` is architecturally different from Go spec's `Share`
        // (different fields, decomposed across multiple types). Not applicable.
        "share.EncodingTest" => {
            eprintln!("SKIP (known-inapplicable): {prefix}");
            DispatchOutcome::SkippedKnown
        }

        // Validation tests
        "signedssvmsg.SignedSSVMessageTest" => {
            DispatchOutcome::Executed(run_test::<types::SignedSSVMessageTest>(path, contents))
        }
        "ssvmsg.SSVMessageTest" => {
            DispatchOutcome::Executed(run_test::<types::SSVMessageTest>(path, contents))
        }
        "partialsigmessage.MsgSpecTest" => {
            DispatchOutcome::Executed(run_test::<types::PartialSigMsgSpecTest>(path, contents))
        }

        // TODO(spec-tests): Add more test types here as they are implemented.
        // This arm will be replaced with panic!() once all test types are added.
        _ => {
            eprintln!("SKIP (not yet implemented): {prefix}");
            DispatchOutcome::SkippedUnknown
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

        match dispatch_fixture_by_prefix(prefix, &path, &contents) {
            DispatchOutcome::Executed(result) => {
                executed_count += 1;
                if let Err(e) = result {
                    failures.push(format!("  {filename}: {e}"));
                }
            }
            DispatchOutcome::SkippedKnown => skipped_known_count += 1,
            DispatchOutcome::SkippedUnknown => skipped_unknown_count += 1,
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
