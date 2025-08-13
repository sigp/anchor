use std::{ffi::OsStr, fmt::Display, fs, fs::File, io::Write, path::Path};

use global_config::data_dir::DataDir;
use openssl::{pkey::Private, rsa::Rsa};
use operator_key::encrypted::EncryptedKey;
use tracing::{debug, info, warn};
use zeroize::Zeroizing;

pub(crate) fn read_or_generate_private_key(
    data_dir: &DataDir,
    key_file: Option<&Path>,
    password_file: Option<&Path>,
) -> Result<Rsa<Private>, String> {
    // First, we have to read a file and decide what to do.
    let public_key_file = data_dir.public_key_file();

    let key = if let Some(key_file) = key_file {
        try_read(key_file, password_file).unwrap_or_else(|| {
            Err(format!(
                "Explicitly passed key file does not exist, generate one with `anchor keygen`: {}",
                key_file.display()
            ))
        })
    } else {
        // Read key from data dir
        let unencrypted_key_file = data_dir.unencrypted_private_key_file();
        let encrypted_key_file = data_dir.encrypted_private_key_file();

        try_read(&unencrypted_key_file, password_file)
            .or_else(|| try_read(&encrypted_key_file, password_file))
            .unwrap_or_else(|| generate_key(data_dir, password_file))
    }?;

    // Write public key so that the user can use it to register the operator. We intentionally
    // always do this and overwrite outdated values.
    let pubkey = operator_key::public::to_base64(&key).map_err(|e| e.to_string())?;
    fs::write(&public_key_file, pubkey).map_err(|e| format!("Unable to write public key: {e}"))?;

    Ok(key)
}

/// Try to read a key file, using an optional password file
///
/// Returns `None` if the file does not exists.
fn try_read(key_file: &Path, password_file: Option<&Path>) -> Option<Result<Rsa<Private>, String>> {
    if !key_file.exists() {
        return None;
    }

    debug!(file = %key_file.display(), "Reading private key");
    let file_contents = match fs::read(key_file) {
        Ok(contents) => Zeroizing::new(contents),
        Err(e) => return Some(Err(format!("Unable to read {}: {e}", key_file.display()))),
    };

    let extension = key_file
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase);

    Some(match extension.as_deref() {
        Some("txt") => parse_unencrypted(&file_contents, password_file),
        Some("json") => parse_encrypted(&file_contents, password_file),
        _ => Err(format!(
            "Unknown key file extension: {}",
            key_file.display()
        )),
    })
}

fn parse_unencrypted(
    key: &Zeroizing<Vec<u8>>,
    password_file: Option<&Path>,
) -> Result<Rsa<Private>, String> {
    // Try to read as an unencrypted key
    if password_file.is_some() {
        warn!("Provided password file, but unencrypted key is present");
    }
    convert(key, operator_key::unencrypted::from_base64)
}

fn parse_encrypted(
    key: &Zeroizing<Vec<u8>>,
    password_file: Option<&Path>,
) -> Result<Rsa<Private>, String> {
    // Try to read as an encrypted key
    let key = convert(key, EncryptedKey::try_from)?;
    let password = if let Some(password_file) = password_file {
        read_password_from_file(password_file)
    } else {
        read_password_from_user()
    }?;
    key.decrypt(password.as_str())
        .map_err(|_| "Key decryption failed".to_string())
}

fn generate_key(dir: &DataDir, password_file: Option<&Path>) -> Result<Rsa<Private>, String> {
    info!("Creating private key");
    let key = Rsa::generate(2048).map_err(|e| format!("Unable to generate key: {e}"))?;
    // Encrypt the fresh key if a password key file was provided. For interactive password
    // input, the user should use the keygen tool.
    let password = password_file.map(read_password_from_file).transpose()?;
    save_key(&key, password.as_ref(), dir)?;
    Ok(key)
}

/// Helper to convert a key to avoid repetition of `map_err` in main logic
fn convert<'a: 'b, 'b, T, E: Display>(
    key: &'a Zeroizing<Vec<u8>>,
    f: impl FnOnce(&'b [u8]) -> Result<T, E>,
) -> Result<T, String> {
    f(key.as_slice()).map_err(|e| format!("Unable to parse key: {e}"))
}

fn read_password_from_file(password_file: &Path) -> Result<Zeroizing<String>, String> {
    fs::read_to_string(password_file)
        // Zeroize the original allocation
        .map(Zeroizing::new)
        // Also zeroize the allocation for the trimmed String
        .map(|full| Zeroizing::new(full.trim_matches(['\n', '\r']).to_string()))
        .map_err(|e| format!("Unable to read password file: {e}"))
}

fn read_password_from_user() -> Result<Zeroizing<String>, String> {
    keygen::read_password_from_user(false)
        .map_err(|e| format!("Unable to read password interactively: {e}"))
}

fn save_key(
    key: &Rsa<Private>,
    password: Option<&Zeroizing<String>>,
    data_dir: &DataDir,
) -> Result<(), String> {
    if let Some(password) = password {
        let file = data_dir.encrypted_private_key_file();
        info!(file = %file.display(), "Saving encrypted private key");
        let encrypted_key =
            EncryptedKey::encrypt(key, password.as_str()).map_err(|_| "Unable to encrypt key")?;
        let serialized_key = String::try_from(encrypted_key)
            .map_err(|e| format!("Unable to serialize encrypted key: {e}"))?;
        File::create_new(file)
            .and_then(|mut file| {
                file.write_all(serialized_key.as_ref())?;
                file.sync_all()
            })
            .map_err(|e| format!("Unable to write encrypted private key: {e}"))
    } else {
        let file = data_dir.unencrypted_private_key_file();
        info!(file = %file.display(), "Saving unencrypted private key");
        let serialized_key = operator_key::unencrypted::to_base64(key)
            .map_err(|_| "Unable to serialize unencrypted key".to_string())?;
        File::create_new(file)
            .and_then(|mut file| {
                file.write_all(serialized_key.as_ref())?;
                file.sync_all()
            })
            .map_err(|e| format!("Unable to write unencrypted private key: {e}"))
    }
}
