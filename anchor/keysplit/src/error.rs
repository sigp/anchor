/// Errors that may occur during the process of splitting a validator key
#[derive(Debug)]
pub enum KeysplitError {
    Keystore(String),
    InvalidKeyLen(String),
    InvalidOperator(String),
    Password(String),
    Output(String),
    Operator(u32),
    RpcEndpoint,
    Database(String),
    SplitFailure(String),
    Misc(String),
    Scrypt(String),
    Pbkdf2(String),
}
