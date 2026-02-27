#![cfg(test)]

mod runner;
mod types;
mod utils;

use std::path::Path;

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

#[cfg(test)]
mod spec_tests {
    use super::*;

    #[test]
    fn types_spec_tests() {
        runner::run_all_type_fixtures();
    }
}
