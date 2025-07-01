use std::{fmt::Display, fs, fs::File, io::Write, path::Path};

use openssl::{pkey::Private, rsa::Rsa};
use operator_key::encrypted::EncryptedKey;
use tracing::{debug, info, warn};
use zeroize::Zeroizing;

pub(crate) fn read_or_generate_private_key(
    data_dir: &Path,
    password_file: Option<&Path>,
) -> Result<Rsa<Private>, String> {
    // First, we have to read a file and decide what to do.
    // TODO: do not hardcode paths here: https://github.com/sigp/anchor/issues/403
    let unencrypted_key_file = data_dir.join("unencrypted_private_key.txt");
    let encrypted_key_file = data_dir.join("encrypted_private_key.json");
    let legacy_key_file = data_dir.join("key.pem");
    let public_key_file = data_dir.join("public_key.txt");

    let key = if let Some(key) = try_read(&unencrypted_key_file)? {
        // Try to read as an unencrypted key
        if password_file.is_some() {
            warn!("Provided password file, but unencrypted key is present");
        }
        convert(&key, operator_key::unencrypted::from_base64)?
    } else if let Some(key) = try_read(&encrypted_key_file)? {
        // Try to read as an encrypted key
        let key = convert(&key, EncryptedKey::try_from)?;
        let password = if let Some(password_file) = password_file {
            read_password_from_file(password_file)
        } else {
            read_password_from_user()
        }?;
        key.decrypt(password.as_str()).map_err(|e| e.to_string())?
    } else if let Some(key) = try_read(&legacy_key_file)? {
        info!("Converting legacy key file");
        // Get the password file always, as we will want to encrypt the key if it was provided
        let mut password = password_file.map(read_password_from_file).transpose()?;
        // First, try to read the key as unencrypted...
        let key = convert(&key, operator_key::legacy::from_unencrypted_pem).or_else(|_| {
            // ...and fall back to encrypted, reading the PW from the console if no file read above
            let password = match &password {
                Some(password) => password,
                None => password.insert(read_password_from_user()?),
            };
            convert(&key, |k| {
                operator_key::legacy::decrypt(password.as_str(), k)
            })
        })?;
        // Save the key, encrypting it if a password was provided via file or console.
        save_key(&key, password.as_ref(), data_dir)?;
        // At this point, we have successfully written the key, so we can safely delete the legacy
        // key file to avoid redundancy.
        fs::remove_file(legacy_key_file)
            .map_err(|e| format!("Unable to remove legacy key file: {e}"))?;
        key
    } else {
        info!("Creating private key");
        let key = Rsa::generate(2048).map_err(|e| format!("Unable to generate key: {e}"))?;
        // Encrypt the fresh key if a password key file was provided. For interactive password
        // input, the user should use the keygen tool.
        let password = password_file.map(read_password_from_file).transpose()?;
        save_key(&key, password.as_ref(), data_dir)?;
        key
    };

    // Write public key so that the user can use it to register the operator. We intentionally
    // always do this and overwrite outdated values.
    let pubkey = operator_key::public::to_base64(&key).map_err(|e| e.to_string())?;
    fs::write(&public_key_file, pubkey).map_err(|e| format!("Unable to write public key: {e}"))?;

    Ok(key)
}

/// Try to read a file.
///
/// Returns `Ok(None)` if the file does not exists.
fn try_read(file: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    file.exists()
        .then(|| {
            debug!(file = %file.display(), "Reading private key");
            fs::read(file)
                .map(Zeroizing::new)
                .map_err(|e| format!("Unable to read {}: {e}", file.display()))
        })
        .transpose()
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
        .map(Zeroizing::new)
        .map_err(|e| format!("Unable to read password file: {e}"))
}

fn read_password_from_user() -> Result<Zeroizing<String>, String> {
    keygen::read_password_from_user(false)
        .map_err(|e| format!("Unable to read password interactively: {e}"))
}

fn save_key(
    key: &Rsa<Private>,
    password: Option<&Zeroizing<String>>,
    data_dir: &Path,
) -> Result<(), String> {
    // TODO: do not hardcode paths here: https://github.com/sigp/anchor/issues/403
    if let Some(password) = password {
        let file = data_dir.join("encrypted_private_key.json");
        info!(file = %file.display(), "Saving encrypted private key");
        let encrypted_key =
            EncryptedKey::encrypt(key, password.as_str()).map_err(|e| e.to_string())?;
        let serialized_key = String::try_from(encrypted_key)
            .map_err(|e| format!("Unable to serialize encrypted key: {e}"))?;
        File::create_new(file)
            .and_then(|mut file| file.write_all(serialized_key.as_ref()))
            .map_err(|e| format!("Unable to write encrypted private key: {e}"))
    } else {
        let file = data_dir.join("unencrypted_private_key.txt");
        info!(file = %file.display(), "Saving unencrypted private key");
        let serialized_key = operator_key::unencrypted::to_base64(key)
            .map_err(|e| format!("Unable to serialize unencrypted key: {e}"))?;
        File::create_new(file)
            .and_then(|mut file| file.write_all(serialized_key.as_ref()))
            .map_err(|e| format!("Unable to write unencrypted private key: {e}"))
    }
}
