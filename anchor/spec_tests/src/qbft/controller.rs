use crate::{QbftSpecTestType, SpecTest, SpecTestType};

struct ControllerTest {
    name: String,
}

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
