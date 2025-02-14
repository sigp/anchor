/// Errors that may occur during the process of splitting a validator key
#[derive(Debug)]
pub enum KeygenError {
    InvalidPassword,
    InvalidKeystorePath,
    InvalidOperator(u32),
    InvalidRpcEndpoint,
}