use crate::{QbftSpecTestType, SpecTest, SpecTestType};

struct RoundRobinTest {
    name: String,
}
impl SpecTest for RoundRobinTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(&self) -> bool {
        true
    }

    fn setup(&mut self) {}

    fn test_type() -> SpecTestType {
        SpecTestType::Qbft(QbftSpecTestType::RoundRobin)
    }
}
