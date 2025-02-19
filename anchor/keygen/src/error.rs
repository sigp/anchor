/// Errors that may occur during the process of splitting a validator key
#[derive(Debug)]
pub enum KeygenError {
    Keystore(String),
    InvalidKeyLen(String),
    InvalidOperator(String),
    Operator(u32),
    RpcEndpoint,
    Database(String),
    SplitFailure(String),
    Misc(String),
    Scrypt(String),
}
