#![allow(dead_code)]

mod qbft;
use std::{collections::HashMap, fmt, fs, path::Path, sync::LazyLock};

use qbft::QbftSpecTestType;
use serde::de::DeserializeOwned;
use walkdir::WalkDir;

use crate::qbft::*;

// All Spec Test Variants. Maps to an inner variant type that describes specific tests
#[derive(Eq, PartialEq, Hash)]
enum SpecTestType {
    Qbft(QbftSpecTestType),
}

// Impl display for path construction. Do not change
impl fmt::Display for SpecTestType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SpecTestType::Qbft(_) => write!(f, "src/ssv-spec/qbft/spectest/generate/tests"),
        }
    }
}

// Core trait to orchestrate setting up and running spec tests. The spec tests are broken up into
// different categories with different file strucutres. For each file structure, implementing the
// required functions allows for a smooth testing process
trait SpecTest {
    // Retrieve the name of the test
    fn name(&self) -> &str;

    // Setup a runner for the test. This will configure and construct eveything required to
    // execute the test
    fn setup(&mut self);

    // Run the test and verify that the output is what we were expecting.
    fn run(&self) -> bool;

    // Return the type of this test. Used as a Key for the loaders and path construction
    fn test_type() -> SpecTestType
    where
        Self: Sized;
}

// Abstract away repeated logic for registering a test type with the loader
macro_rules! register_test_loaders {
    ($($test_type:ty),* $(,)?) => {
        LazyLock::new(|| {
            let mut loaders = HashMap::new();
            $(
                register_test::<$test_type>(&mut loaders);
            )*
            loaders
        })
    };
}

type Loaders = HashMap<SpecTestType, fn(&str) -> Box<dyn SpecTest>>;
static TEST_LOADERS: LazyLock<Loaders> = register_test_loaders!(TimeoutTest);

// Register a test in the loader. This inserts a mapping from SpecTestType -> loading closure
// into a map for later access. This is needed to that we can parse from an arbitrary test file to a
// specific test type T
fn register_test<T: SpecTest + DeserializeOwned + 'static>(map: &mut Loaders) {
    map.insert(T::test_type(), |path| {
        let contents = fs::read_to_string(path)
            .unwrap_or_else(|_| panic!("Failed to read test file: {}", path));
        let test: T = serde_json::from_str(&contents)
            .unwrap_or_else(|e| panic!("Failed to parse test {}: {}", path, e));
        Box::new(test)
    });
}

// Core function to run the tests. Given a SpecTestType, it will navigate to the proper directory,
// read in all of the tests, make sure they are all setup, and then run each one
fn run_tests(test_type: SpecTestType) -> bool {
    let dir_name = test_type.to_string();
    let test_dir = Path::new(&dir_name);

    let tests: Vec<Box<dyn SpecTest>> = WalkDir::new(test_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();

            // Get the inner variant string to check in filenames
            let variant = match &test_type {
                SpecTestType::Qbft(inner) => inner.to_string(),
            };

            if path.is_file()
                && path
                    .file_name()
                    .map(|name| name.to_string_lossy().contains(&variant))
                    .unwrap_or(false)
            {
                let loader = TEST_LOADERS
                    .get(&test_type)
                    .unwrap_or_else(|| panic!("No loader registered for:{}", test_type));
                Some(loader(&path.to_string_lossy()))
            } else {
                None
            }
        })
        .collect();
    // todo!() do the setup
    let mut result = true;
    for test in tests {
        result &= test.run();
    }
    result
}

#[cfg(test)]
mod spec_tests {
    use super::*;

    #[test]
    fn test_qbft_timeout() {
        assert!(run_tests(SpecTestType::Qbft(QbftSpecTestType::Timeout)))
    }
}
