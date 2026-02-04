use std::{ffi::OsStr, fs, path::Path};

use openssl::{pkey::Private, rsa::Rsa};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::{
    encrypted::{DecryptionError, EncryptedKey},
    unencrypted,
};

/// Try to read a key file, using an optional password file.
///
/// If the file extension is `txt`, the file will be read as unencrypted key.
/// If the file extension is `json`, the file will be read as encrypted key.
/// Else, an error is returned.
///
/// If no password file is specified and an encrypted file is encountered, the user will be prompted
/// to enter a password. If a password file is specified, but the file is unencrypted, an error is
/// returned.
pub fn try_read_from_file(
    key_file: &Path,
    password_file: Option<&Path>,
) -> Result<Rsa<Private>, FileReadError> {
    if !key_file.exists() {
        return Err(FileReadError::DoesNotExist);
    }

    let file_contents = Zeroizing::new(fs::read(key_file).map_err(FileReadError::KeyReadError)?);

    let extension = key_file
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase);

    match extension.as_deref() {
        Some("txt") => parse_unencrypted(&file_contents, password_file),
        Some("json") => parse_encrypted(&file_contents, password_file),
        ext => Err(FileReadError::ExtensionUnknown(
            ext.unwrap_or_default().to_string(),
        )),
    }
}

#[derive(Debug, Error)]
pub enum FileReadError {
    #[error("Key file does not exist")]
    DoesNotExist,
    #[error("Unknown file extension: {0}")]
    ExtensionUnknown(String),
    #[error("Unable to read key file: {0}")]
    KeyReadError(#[source] std::io::Error),
    #[error("Unable to read password: {0}")]
    PasswordReadError(#[source] std::io::Error),
    #[error("Unable to parse key: {0}")]
    ConversionError(#[from] crate::ConversionError),
    #[error("Unable to parse encrypted key json: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("Unable to use password file for unencrypted key")]
    UnnecessaryPasswordFile,
    #[error("Decryption failed: {0}")]
    DecryptionError(#[from] DecryptionError),
}

fn parse_unencrypted(
    key: &Zeroizing<Vec<u8>>,
    password_file: Option<&Path>,
) -> Result<Rsa<Private>, FileReadError> {
    // Try to read as an unencrypted key
    if password_file.is_some() {
        return Err(FileReadError::UnnecessaryPasswordFile);
    }
    unencrypted::from_base64(key).map_err(FileReadError::ConversionError)
}

fn parse_encrypted(
    key: &Zeroizing<Vec<u8>>,
    password_file: Option<&Path>,
) -> Result<Rsa<Private>, FileReadError> {
    // Try to read as an encrypted key
    let key = EncryptedKey::try_from(key.as_slice())?;
    let password = if let Some(password_file) = password_file {
        read_password_from_file(password_file)?
    } else {
        let pw = rpassword::prompt_password("Password for reading encrypted key: ")
            .map_err(FileReadError::PasswordReadError)?;
        Zeroizing::new(pw)
    };
    key.decrypt(password.as_str())
        .map_err(FileReadError::DecryptionError)
}

/// Reads a password from a file, trimming newline characters and wrapping in `Zeroizing`.
pub fn read_password_from_file(password_file: &Path) -> Result<Zeroizing<String>, FileReadError> {
    fs::read_to_string(password_file)
        // Zeroize the original allocation
        .map(Zeroizing::new)
        // Also zeroize the allocation for the trimmed String
        .map(|full| Zeroizing::new(full.trim_matches(['\n', '\r']).to_string()))
        .map_err(FileReadError::PasswordReadError)
}
