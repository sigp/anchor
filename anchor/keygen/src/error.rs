/// Errors that may occur during the process of splitting a validator key
#[derive(Debug)]
pub enum KeygenError {
    Keystore(String),
    Operator(u32),
    RpcEndpoint,
    SplitFailure(String),
    Misc(String),
    Scrypt(String),
}
