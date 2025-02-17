/// Errors that may occur during the process of splitting a validator key
#[derive(Debug)]
pub enum KeygenError {
    Password,
    KeystorePath,
    Operator(u32),
    RpcEndpoint,
}

