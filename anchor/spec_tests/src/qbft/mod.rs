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

// Impl display for path construct. Do not change
impl std::fmt::Display for QbftSpecTestType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            QbftSpecTestType::Timeout => write!(f, "Timeout"),
            QbftSpecTestType::QbftMessage => write!(f, "Qbft-Message"),
            QbftSpecTestType::MessageProcessing => write!(f, "Message-Processing"),
            QbftSpecTestType::CreateMessage => write!(f, "Create-Message"),
            QbftSpecTestType::Controller => write!(f, "Controller"),
            QbftSpecTestType::RoundRobin => write!(f, "RoundRobin"),
        }
    }
}
