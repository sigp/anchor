pub mod adapters;
mod create_message;
mod message_processing;
mod qbft_message;
mod round_robin;
mod timeout;

// Export test types
pub use create_message::CreateMessageTest;
pub use message_processing::MessageProcessingTest;
pub use qbft_message::QbftMessageTest;
pub use round_robin::RoundRobinTest;
pub use timeout::TimeoutTest;

#[derive(Eq, PartialEq, Hash, Debug)]
pub(crate) enum QbftSpecTestType {
    QbftMessage,
    CreateMessage,
    MsgProcessing,
    RoundRobin,
    Timeout,
}

// Contains specific identifier for the test file
impl std::fmt::Display for QbftSpecTestType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            QbftSpecTestType::QbftMessage => write!(f, "MsgSpecTest"),
            QbftSpecTestType::CreateMessage => write!(f, "CreateMsgSpecTest"),
            QbftSpecTestType::MsgProcessing => write!(f, "MsgProcessingSpecTest"),
            QbftSpecTestType::RoundRobin => write!(f, "RoundRobinSpecTest"),
            QbftSpecTestType::Timeout => write!(f, "timeout"),
        }
    }
}
