use std::{fs, fs::File, io::Write, path::Path};

use global_config::data_dir::DataDir;
use openssl::{pkey::Private, rsa::Rsa};
use operator_key::{encrypted::EncryptedKey, util::FileReadError};
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
        let Some(key) = try_read(key_file, password_file) else {
            return Err(format!(
                "Explicitly passed key file does not exist, generate one with `anchor keygen`: {}",
                key_file.display()
            ));
        };
        key
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

// Tries to read a key file. Returns None if the file doesn't exist, Some(Err) on read errors,
// or Some(Ok) with the key on success.
fn try_read(key_file: &Path, password_file: Option<&Path>) -> Option<Result<Rsa<Private>, String>> {
    debug!(file = %key_file.display(), "Reading private key");
    let convert_other_errs = |e| format!("Unable to read {}: {e}", key_file.display());
    let result = match operator_key::util::try_read_from_file(key_file, password_file) {
        Err(FileReadError::DoesNotExist) => {
            return None;
        }
        Err(FileReadError::UnnecessaryPasswordFile) => {
            warn!("Provided password file, but unencrypted key is present");
            // Use key anyway.
            operator_key::util::try_read_from_file(key_file, None).map_err(convert_other_errs)
        }
        other => other.map_err(convert_other_errs),
    };
    Some(result)
}

fn generate_key(dir: &DataDir, password_file: Option<&Path>) -> Result<Rsa<Private>, String> {
    info!("Creating private key");
    let key = Rsa::generate(2048).map_err(|e| format!("Unable to generate key: {e}"))?;
    // Encrypt the fresh key if a password key file was provided. For interactive password
    // input, the user should use the keygen tool.
    let password = password_file
        .map(|pf| operator_key::util::read_password_from_file(pf).map_err(|e| e.to_string()))
        .transpose()?;
    save_key(&key, password.as_ref(), dir)?;
    Ok(key)
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
