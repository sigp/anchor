/// Errors that may occur during the process of splitting a validator key
#[derive(Debug)]
pub enum KeysplitError {
    Keystore(String),
    InvalidKeyLen(String),
    InvalidOperator(String),
    Output(String),
    Database(String),
    SplitFailure(String),
    Misc(String),
}
