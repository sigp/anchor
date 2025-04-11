// QBFT test variants
// Modules contain parsing logic
mod controller;
mod create_message;
mod message_processing;
mod qbft_message;
mod round_robin;
mod timeout;

pub use timeout::TimeoutTest;

#[derive(Eq, PartialEq, Hash)]
pub(crate) enum QbftSpecTestType {
    Timeout,
    QbftMessage,
    MessageProcessing,
    CreateMessage,
    Controller,
    RoundRobin,
}

// Contains specific identifier for the test file
impl std::fmt::Display for QbftSpecTestType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            QbftSpecTestType::Timeout => write!(f, "timeout"),
            QbftSpecTestType::QbftMessage => write!(f, "MsgSpecTest"),
            QbftSpecTestType::MessageProcessing => write!(f, "MsgProcessingSpecTest"),
            QbftSpecTestType::CreateMessage => write!(f, "CreateMsgSpecTest"),
            QbftSpecTestType::Controller => write!(f, "ControllerSpecTest"),
            QbftSpecTestType::RoundRobin => write!(f, "RoundRobinSpecTest"),
        }
    }
}
