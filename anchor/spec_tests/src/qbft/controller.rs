use serde::{Deserialize, Serialize};

use crate::{QbftSpecTestType, SpecTest, SpecTestType};

impl SpecTest for ControllerTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(&self) -> bool {
        true
    }

    fn setup(&mut self) {}

    fn test_type() -> SpecTestType {
        SpecTestType::Qbft(QbftSpecTestType::Controller)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ControllerTest {
    pub name: String,
}
