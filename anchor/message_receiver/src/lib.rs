mod manager;

use thiserror::Error;

pub use crate::manager::*;
pub use crate::MessageReceiver;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Processor error: {0}")]
    Processor(#[from] processor::Error),
}
