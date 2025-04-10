mod qbft;
use std::{collections::HashMap, fmt, fs, path::Path, sync::LazyLock};

use qbft::QbftSpecTestType;
use serde::de::DeserializeOwned;
use walkdir::WalkDir;

use crate::qbft::*;

// All spec test variants
#[derive(Eq, PartialEq, Hash)]
enum SpecTestType {
    Qbft(QbftSpecTestType),
}

// Impl display for path construction. Do not change
impl fmt::Display for SpecTestType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SpecTestType::Qbft(qbft_type) => write!(f, "src/qbft/tests/{}", qbft_type),
        }
    }
}

trait SpecTest {
    // Retrieve the name of the test
    fn name(&self) -> &str;

    // Setup a runner for the specific test
    fn setup(&mut self);

    // Run the the test and return a boolean indicating success
    fn run(&self) -> bool;

    fn test_type() -> SpecTestType
    where
        Self: Sized;
}

fn register_test<T: SpecTest + DeserializeOwned + 'static>(map: &mut Loaders) {
    map.insert(T::test_type(), |path| {
        let contents = fs::read_to_string(path)
            .unwrap_or_else(|_| panic!("Failed to read test file: {}", path));
        let test: T = serde_json::from_str(&contents)
            .unwrap_or_else(|e| panic!("Failed to parse test {}: {}", path, e));
        Box::new(test)
    });
}

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

fn run_tests(test_type: SpecTestType) -> bool {
    let dir_name = test_type.to_string();
    let test_dir = Path::new(&dir_name);

    let tests: Vec<Box<dyn SpecTest>> = WalkDir::new(test_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.is_file() {
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
