use std::{ffi::OsStr, fs, io, path::Path};

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
        read_password_from_file(password_file).map_err(FileReadError::PasswordReadError)?
    } else {
        let pw = rpassword::prompt_password("Password for reading encrypted key: ")
            .map_err(FileReadError::PasswordReadError)?;
        Zeroizing::new(pw)
    };
    key.decrypt(password.as_str())
        .map_err(FileReadError::DecryptionError)
}

/// Reads a password from a file, trimming newline characters and wrapping in `Zeroizing`.
pub fn read_password_from_file(password_file: &Path) -> Result<Zeroizing<String>, io::Error> {
    fs::read_to_string(password_file)
        // Zeroize the original allocation
        .map(Zeroizing::new)
        // Also zeroize the allocation for the trimmed String
        .map(|full| Zeroizing::new(full.trim_matches(['\n', '\r']).to_string()))
}

#[cfg(test)]
mod tests {
    use std::io::{ErrorKind, Write};

    use base64::Engine;
    use tempfile::NamedTempFile;

    use super::*;

    // ==================== Test Constants ====================

    const TEST_PASSWORD: &str = "test_password_123";
    const ENCRYPTED_KEY_PASSWORD: &str = "what";
    const RSA_KEY_SIZE: u32 = 2048;

    // ==================== Helper Functions ====================

    /// Creates a temporary file with the given content and extension.
    fn create_temp_file_with_content(content: &[u8], extension: &str) -> NamedTempFile {
        let temp_file = tempfile::Builder::new()
            .suffix(extension)
            .tempfile()
            .expect("Failed to create temp file");

        std::fs::write(temp_file.path(), content).expect("Failed to write to temp file");

        temp_file
    }

    /// Creates a temporary file containing a valid unencrypted key.
    fn create_temp_unencrypted_key_file() -> (NamedTempFile, Rsa<Private>) {
        let key = Rsa::generate(RSA_KEY_SIZE).expect("Failed to generate RSA key");
        let encoded = crate::unencrypted::to_base64(&key).expect("Failed to encode key");

        let temp_file = create_temp_file_with_content(encoded.as_bytes(), ".txt");

        (temp_file, key)
    }

    /// Creates a temporary file containing a valid encrypted key.
    fn create_temp_encrypted_key_file(password: &str) -> (NamedTempFile, Rsa<Private>) {
        let key = Rsa::generate(RSA_KEY_SIZE).expect("Failed to generate RSA key");
        let encrypted =
            crate::encrypted::EncryptedKey::encrypt(&key, password).expect("Failed to encrypt key");
        let json = serde_json::to_string(&encrypted).expect("Failed to serialize encrypted key");

        let temp_file = create_temp_file_with_content(json.as_bytes(), ".json");

        (temp_file, key)
    }

    /// Creates a temporary password file with the given password.
    fn create_temp_password_file(password: &str) -> NamedTempFile {
        let mut temp_file = NamedTempFile::new().expect("Failed to create temp password file");
        temp_file
            .write_all(password.as_bytes())
            .expect("Failed to write password");
        temp_file.flush().expect("Failed to flush password file");
        temp_file
    }

    /// Asserts that two RSA keys are equal by comparing their components.
    fn assert_keys_equal(key1: &Rsa<Private>, key2: &Rsa<Private>) {
        assert_eq!(key1.p(), key2.p(), "RSA key p components differ");
        assert_eq!(key1.q(), key2.q(), "RSA key q components differ");
        assert_eq!(key1.n(), key2.n(), "RSA key n components differ");
        assert_eq!(key1.e(), key2.e(), "RSA key e components differ");
    }

    // ==================== File Existence Tests ====================

    #[test]
    fn test_try_read_from_file_nonexistent_file_returns_does_not_exist() {
        // Arrange
        let nonexistent_path = std::path::Path::new("/tmp/nonexistent_key_file_12345.txt");

        // Act
        let result = try_read_from_file(nonexistent_path, None);

        // Assert
        assert!(
            matches!(result, Err(FileReadError::DoesNotExist)),
            "Expected DoesNotExist error for nonexistent file"
        );
    }

    // ==================== Extension Handling Tests ====================

    #[test]
    fn test_try_read_from_file_unknown_extension_returns_extension_unknown() {
        // Arrange
        let temp_file = create_temp_file_with_content(b"some content", ".pem");

        // Act
        let result = try_read_from_file(temp_file.path(), None);

        // Assert
        match result {
            Err(FileReadError::ExtensionUnknown(ext)) => {
                assert_eq!(ext, "pem", "Expected 'pem' extension in error");
            }
            _ => panic!("Expected ExtensionUnknown error"),
        }
    }

    #[test]
    fn test_try_read_from_file_no_extension_returns_extension_unknown() {
        // Arrange
        let temp_file = NamedTempFile::new().expect("Failed to create temp file");
        std::fs::write(temp_file.path(), b"some content").expect("Failed to write");

        // Act
        let result = try_read_from_file(temp_file.path(), None);

        // Assert
        match result {
            Err(FileReadError::ExtensionUnknown(ext)) => {
                assert_eq!(ext, "", "Expected empty string for no extension");
            }
            _ => panic!("Expected ExtensionUnknown error with empty string"),
        }
    }

    #[test]
    fn test_try_read_from_file_unknown_key_extension_returns_extension_unknown() {
        // Arrange
        let temp_file = create_temp_file_with_content(b"some content", ".key");

        // Act
        let result = try_read_from_file(temp_file.path(), None);

        // Assert
        match result {
            Err(FileReadError::ExtensionUnknown(ext)) => {
                assert_eq!(ext, "key", "Expected 'key' extension in error");
            }
            _ => panic!("Expected ExtensionUnknown error for .key extension"),
        }
    }

    #[test]
    fn test_try_read_from_file_case_insensitive_txt_extension() {
        // Arrange
        let (temp_file, expected_key) = create_temp_unencrypted_key_file();
        let uppercase_path = temp_file.path().with_extension("TXT");
        std::fs::rename(temp_file.path(), &uppercase_path).expect("Failed to rename");

        // Act
        let result = try_read_from_file(&uppercase_path, None);

        // Assert
        let decoded_key = result.expect("Should successfully read .TXT file");
        assert_keys_equal(&decoded_key, &expected_key);

        // Cleanup
        std::fs::remove_file(uppercase_path).ok();
    }

    #[test]
    fn test_try_read_from_file_case_insensitive_json_extension() {
        // Arrange
        let (temp_file, expected_key) = create_temp_encrypted_key_file(TEST_PASSWORD);
        let password_file = create_temp_password_file(TEST_PASSWORD);
        let uppercase_path = temp_file.path().with_extension("JSON");
        std::fs::rename(temp_file.path(), &uppercase_path).expect("Failed to rename");

        // Act
        let result = try_read_from_file(&uppercase_path, Some(password_file.path()));

        // Assert
        let decoded_key = result.expect("Should successfully read .JSON file");
        assert_keys_equal(&decoded_key, &expected_key);

        // Cleanup
        std::fs::remove_file(uppercase_path).ok();
    }

    // ==================== Unencrypted Key Tests ====================

    #[test]
    fn test_try_read_from_file_unencrypted_key_success() {
        // Arrange
        let (temp_file, expected_key) = create_temp_unencrypted_key_file();

        // Act
        let result = try_read_from_file(temp_file.path(), None);

        // Assert
        let decoded_key = result.expect("Should successfully read unencrypted key");
        assert_keys_equal(&decoded_key, &expected_key);
    }

    #[test]
    fn test_try_read_from_file_unencrypted_key_with_password_file_returns_unnecessary_password() {
        // Arrange
        let (temp_file, _) = create_temp_unencrypted_key_file();
        let password_file = create_temp_password_file(TEST_PASSWORD);

        // Act
        let result = try_read_from_file(temp_file.path(), Some(password_file.path()));

        // Assert
        assert!(
            matches!(result, Err(FileReadError::UnnecessaryPasswordFile)),
            "Expected UnnecessaryPasswordFile error when providing password for unencrypted key"
        );
    }

    #[test]
    fn test_try_read_from_file_invalid_base64_returns_conversion_error() {
        // Arrange
        let temp_file = create_temp_file_with_content(b"not valid base64!@#$", ".txt");

        // Act
        let result = try_read_from_file(temp_file.path(), None);

        // Assert
        assert!(
            matches!(result, Err(FileReadError::ConversionError(_))),
            "Expected ConversionError for invalid base64"
        );
    }

    #[test]
    fn test_try_read_from_file_invalid_pem_format_returns_conversion_error() {
        // Arrange
        let invalid_pem = base64::prelude::BASE64_STANDARD.encode(b"not a valid PEM format");
        let temp_file = create_temp_file_with_content(invalid_pem.as_bytes(), ".txt");

        // Act
        let result = try_read_from_file(temp_file.path(), None);

        // Assert
        assert!(
            matches!(result, Err(FileReadError::ConversionError(_))),
            "Expected ConversionError for invalid PEM format"
        );
    }

    #[test]
    fn test_try_read_from_file_empty_txt_file_returns_conversion_error() {
        // Arrange
        let temp_file = create_temp_file_with_content(b"", ".txt");

        // Act
        let result = try_read_from_file(temp_file.path(), None);

        // Assert
        assert!(
            matches!(result, Err(FileReadError::ConversionError(_))),
            "Expected ConversionError for empty txt file"
        );
    }

    // ==================== Encrypted Key Tests ====================

    #[test]
    fn test_try_read_from_file_encrypted_key_with_password_file_success() {
        // Arrange
        let (temp_file, expected_key) = create_temp_encrypted_key_file(TEST_PASSWORD);
        let password_file = create_temp_password_file(TEST_PASSWORD);

        // Act
        let result = try_read_from_file(temp_file.path(), Some(password_file.path()));

        // Assert
        let decoded_key = result.expect("Should successfully decrypt key with password file");
        assert_keys_equal(&decoded_key, &expected_key);
    }

    #[test]
    fn test_try_read_from_file_encrypted_key_incorrect_password_returns_decryption_error() {
        // Arrange
        let (temp_file, _) = create_temp_encrypted_key_file(TEST_PASSWORD);
        let password_file = create_temp_password_file("wrong_password");

        // Act
        let result = try_read_from_file(temp_file.path(), Some(password_file.path()));

        // Assert
        assert!(
            matches!(result, Err(FileReadError::DecryptionError(_))),
            "Expected DecryptionError for incorrect password"
        );
    }

    #[test]
    fn test_try_read_from_file_malformed_json_returns_json_error() {
        // Arrange
        let temp_file = create_temp_file_with_content(b"{invalid json content", ".json");

        // Act
        let result = try_read_from_file(temp_file.path(), None);

        // Assert
        assert!(
            matches!(result, Err(FileReadError::JsonError(_))),
            "Expected JsonError for malformed JSON"
        );
    }

    #[test]
    fn test_try_read_from_file_empty_json_file_returns_json_error() {
        // Arrange
        let temp_file = create_temp_file_with_content(b"", ".json");

        // Act
        let result = try_read_from_file(temp_file.path(), None);

        // Assert
        assert!(
            matches!(result, Err(FileReadError::JsonError(_))),
            "Expected JsonError for empty JSON file"
        );
    }

    #[test]
    fn test_try_read_from_file_invalid_encrypted_key_structure_returns_json_error() {
        // Arrange - JSON that parses but doesn't match EncryptedKey structure
        let invalid_json = r#"{"wrong": "structure"}"#;
        let temp_file = create_temp_file_with_content(invalid_json.as_bytes(), ".json");

        // Act
        let result = try_read_from_file(temp_file.path(), None);

        // Assert
        assert!(
            matches!(result, Err(FileReadError::JsonError(_))),
            "Expected JsonError for invalid encrypted key structure"
        );
    }

    #[test]
    fn test_try_read_from_file_existing_encrypted_test_key() {
        // Arrange - use the existing test key from test_keys directory
        let key_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test_keys")
            .join("encrypted_private_key.json");
        let password_file = create_temp_password_file(ENCRYPTED_KEY_PASSWORD);

        // Act
        let result = try_read_from_file(&key_path, Some(password_file.path()));

        // Assert
        result.expect("Should successfully read existing test encrypted key");
    }

    #[test]
    fn test_try_read_from_file_existing_legacy_encrypted_test_key() {
        // Arrange - use the existing legacy test key from test_keys directory
        let key_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test_keys")
            .join("encrypted_private_key_legacy.json");
        let password_file = create_temp_password_file(ENCRYPTED_KEY_PASSWORD);

        // Act
        let result = try_read_from_file(&key_path, Some(password_file.path()));

        // Assert
        result.expect("Should successfully read existing legacy test encrypted key");
    }

    // ==================== Password File Reading Tests ====================

    #[test]
    fn test_read_password_from_file_success() {
        // Arrange
        let password_file = create_temp_password_file(TEST_PASSWORD);

        // Act
        let result = read_password_from_file(password_file.path());

        // Assert
        let password = result.expect("Should successfully read password");
        assert_eq!(password.as_str(), TEST_PASSWORD);
    }

    #[test]
    fn test_read_password_from_file_trims_newline() {
        // Arrange
        let password_with_newline = format!("{}\n", TEST_PASSWORD);
        let password_file = create_temp_password_file(&password_with_newline);

        // Act
        let result = read_password_from_file(password_file.path());

        // Assert
        let password = result.expect("Should successfully read password");
        assert_eq!(
            password.as_str(),
            TEST_PASSWORD,
            "Password should have newline trimmed"
        );
    }

    #[test]
    fn test_read_password_from_file_trims_carriage_return_newline() {
        // Arrange
        let password_with_crlf = format!("{}\r\n", TEST_PASSWORD);
        let password_file = create_temp_password_file(&password_with_crlf);

        // Act
        let result = read_password_from_file(password_file.path());

        // Assert
        let password = result.expect("Should successfully read password");
        assert_eq!(
            password.as_str(),
            TEST_PASSWORD,
            "Password should have \\r\\n trimmed"
        );
    }

    #[test]
    fn test_read_password_from_file_trims_multiple_newlines() {
        // Arrange
        let password_with_multiple_newlines = format!("{}\n\n\n", TEST_PASSWORD);
        let password_file = create_temp_password_file(&password_with_multiple_newlines);

        // Act
        let result = read_password_from_file(password_file.path());

        // Assert
        let password = result.expect("Should successfully read password");
        assert_eq!(
            password.as_str(),
            TEST_PASSWORD,
            "Password should have all trailing newlines trimmed"
        );
    }

    #[test]
    fn test_read_password_from_file_preserves_spaces() {
        // Arrange
        let password_with_spaces = format!("  {}  ", TEST_PASSWORD);
        let password_file = create_temp_password_file(&password_with_spaces);

        // Act
        let result = read_password_from_file(password_file.path());

        // Assert
        let password = result.expect("Should successfully read password");
        assert_eq!(
            password.as_str(),
            password_with_spaces,
            "Password should preserve spaces"
        );
    }

    #[test]
    fn test_read_password_from_file_preserves_spaces_with_newline() {
        // Arrange
        let password_with_spaces_and_newline = format!("  {}  \n", TEST_PASSWORD);
        let password_file = create_temp_password_file(&password_with_spaces_and_newline);

        // Act
        let result = read_password_from_file(password_file.path());

        // Assert
        let password = result.expect("Should successfully read password");
        assert_eq!(
            password.as_str(),
            format!("  {}  ", TEST_PASSWORD),
            "Password should preserve spaces but trim newline"
        );
    }

    #[test]
    fn test_read_password_from_file_nonexistent_file_returns_password_read_error() {
        // Arrange
        let nonexistent_path = std::path::Path::new("/tmp/nonexistent_password_file_12345.txt");

        // Act
        let result = read_password_from_file(nonexistent_path);

        let err = result.expect_err("Expected io::Error for nonexistent password file");

        // Assert
        assert!(
            matches!(err.kind(), ErrorKind::NotFound),
            "Expected NotFound for nonexistent password file"
        );
    }

    #[test]
    fn test_read_password_from_file_empty_password() {
        // Arrange
        let password_file = create_temp_password_file("");

        // Act
        let result = read_password_from_file(password_file.path());

        // Assert
        let password = result.expect("Should successfully read empty password");
        assert_eq!(password.as_str(), "", "Should handle empty password");
    }

    #[test]
    fn test_read_password_from_file_special_characters() {
        // Arrange
        let special_password = "p@ssw0rd!#$%^&*()";
        let password_file = create_temp_password_file(special_password);

        // Act
        let result = read_password_from_file(password_file.path());

        // Assert
        let password = result.expect("Should successfully read password with special characters");
        assert_eq!(password.as_str(), special_password);
    }

    #[test]
    fn test_read_password_from_file_unicode_characters() {
        // Arrange
        let unicode_password = "пароль密码🔑";
        let password_file = create_temp_password_file(unicode_password);

        // Act
        let result = read_password_from_file(password_file.path());

        // Assert
        let password = result.expect("Should successfully read password with unicode characters");
        assert_eq!(password.as_str(), unicode_password);
    }

    // ==================== Integration Tests ====================

    #[test]
    fn test_encrypted_key_with_password_containing_newline_integration() {
        // Arrange
        let (key_file, expected_key) = create_temp_encrypted_key_file(TEST_PASSWORD);
        let password_with_newline = format!("{}\n", TEST_PASSWORD);
        let password_file = create_temp_password_file(&password_with_newline);

        // Act
        let result = try_read_from_file(key_file.path(), Some(password_file.path()));

        // Assert
        let decoded_key = result.expect("Should decrypt key with password file containing newline");
        assert_keys_equal(&decoded_key, &expected_key);
    }

    #[test]
    fn test_encrypted_key_with_spaces_in_password() {
        // Arrange
        let password_with_spaces = "  my password  ";
        let (key_file, expected_key) = create_temp_encrypted_key_file(password_with_spaces);
        let password_file = create_temp_password_file(password_with_spaces);

        // Act
        let result = try_read_from_file(key_file.path(), Some(password_file.path()));

        // Assert
        let decoded_key = result.expect("Should decrypt key with password containing spaces");
        assert_keys_equal(&decoded_key, &expected_key);
    }
}
