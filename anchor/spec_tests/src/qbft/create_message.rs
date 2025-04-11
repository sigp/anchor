use serde::{Deserialize, Serialize};

use crate::{QbftSpecTestType, SpecTest, SpecTestType};

impl SpecTest for CreateMessageTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(&self) -> bool {
        true
    }

    fn setup(&mut self) {}

    fn test_type() -> SpecTestType {
        SpecTestType::Qbft(QbftSpecTestType::CreateMessage)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateMessageTest {
    pub name: String,
}
