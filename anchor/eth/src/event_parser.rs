use crate::error::ExecutionError;
use crate::gen::SSVContract;
use alloy::{rpc::types::Log, sol_types::SolEvent};

// Standardized event decoding via common Decoder trait.
pub trait EventDecoder {
    type Output;
    fn decode_from_log(log: &Log) -> Result<Self::Output, ExecutionError>;
}

macro_rules! impl_event_decoder {
    ($($event_type:ty),* $(,)?) => {
        $(
            impl EventDecoder for $event_type {
                type Output = $event_type;

                fn decode_from_log(log: &Log) -> Result<Self::Output, ExecutionError> {
                    let decoded = Self::decode_log(&log.inner, true)
                        .map_err(|e| {
                            ExecutionError::DecodeError(
                                format!("Failed to decode {} event: {}", stringify!($event_type), e)
                            )
                        })?;
                    Ok(decoded.data)
                }
            }
        )*
    };
}

impl_event_decoder! {
    SSVContract::OperatorAdded,
    SSVContract::OperatorRemoved,
    SSVContract::ValidatorAdded,
    SSVContract::ValidatorRemoved,
    SSVContract::ClusterLiquidated,
    SSVContract::ClusterReactivated,
    SSVContract::FeeRecipientAddressUpdated,
    SSVContract::ValidatorExited
}
