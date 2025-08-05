//! Encoding, decoding, encryption and decryption of various key formats that appear in the SSV
//! ecosystem.
//!
//! See module docs for a description of the corresponding format.
use std::str::Utf8Error;

use thiserror::Error;

pub mod encrypted;
pub mod public;
pub mod unencrypted;

#[derive(Error, Debug)]
pub enum ConversionError {
    #[error("Unable to decode base64 PEM data: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("Unable to convert decoded PEM data into a String: {0}")]
    NotUtf8(#[from] Utf8Error),
    #[error("Failed to parse PEM: {0}")]
    OpenSSL(#[from] openssl::error::ErrorStack),
}
