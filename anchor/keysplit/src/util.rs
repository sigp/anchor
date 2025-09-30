use std::{fs, io::BufRead, path::Path, str::FromStr};

use base64::prelude::*;
use openssl::{pkey::Public, rsa::Rsa};
use serde::Serializer;
use types::Address;
use zeroize::Zeroizing;

// Serde deserialization and serialization helper functions
pub(crate) fn parse_address(s: &str) -> Result<Address, String> {
    Address::from_str(s).map_err(|e| e.to_string())
}

pub(crate) fn serialize_rsa<S>(key: &Rsa<Public>, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let serialized_key = key.public_key_to_pem().map_err(serde::ser::Error::custom)?;

    // Convert the decoded data to a string
    let mut pem_string = String::from_utf8(serialized_key).map_err(serde::ser::Error::custom)?;

    // Fix the header - replace PKCS8 header with PKCS1 header
    pem_string = pem_string
        .replace(
            "-----BEGIN PUBLIC KEY-----",
            "-----BEGIN RSA PUBLIC KEY-----",
        )
        .replace("-----END PUBLIC KEY-----", "-----END RSA PUBLIC KEY-----");

    let encoded = BASE64_STANDARD.encode(pem_string.clone());
    s.serialize_str(&encoded)
}

/// Reads a password from either a file or stdin.
///
/// If `file` is `Some`, reads the password from the file at the given path,
/// trimming trailing newlines and carriage returns.
///
/// If `file` is `None`, prompts the user interactively for a password via stdin.
///
/// Returns an error if the file cannot be read, the password is empty, or stdin read fails.
pub(crate) fn read_password(file: Option<&Path>) -> Result<Zeroizing<String>, String> {
    if let Some(path) = file {
        read_password_from_file(path)
    } else {
        read_password_from_stdin(&mut std::io::stdin().lock())
    }
}

fn read_password_from_file(path: &Path) -> Result<Zeroizing<String>, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("Unable to read password file: {e}"))?;
    process_password(raw)
}

fn read_password_from_stdin<R: BufRead>(stdin: &mut R) -> Result<Zeroizing<String>, String> {
    let raw = rpassword::read_password_from_bufread(stdin)
        .map_err(|e| format!("Unable to read password from stdin: {e}"))?;
    process_password(raw)
}

fn process_password(raw: String) -> Result<Zeroizing<String>, String> {
    let trimmed = raw.trim_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return Err("Password cannot be empty".to_string());
    }
    Ok(Zeroizing::new(trimmed.to_string()))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Cursor, Write},
        path::PathBuf,
    };

    use tempfile::NamedTempFile;

    use super::*;

    // Helper function for stdin tests
    fn test_read_password_from_stdin(input: &[u8]) -> Result<Zeroizing<String>, String> {
        let mut cursor = Cursor::new(input);
        read_password_from_stdin(&mut cursor)
    }

    // Helper function to create a temp file with content
    fn create_password_file(content: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("Failed to create temp file");
        file.write_all(content.as_bytes())
            .expect("Failed to write to temp file");
        file
    }

    // === File Path Tests ===

    #[test]
    fn test_read_password_from_file_success() {
        let file = create_password_file("hunter2");
        let result = read_password(Some(file.path()));

        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "hunter2");
    }

    #[test]
    fn test_read_password_from_file_with_trailing_newline() {
        let file = create_password_file("hunter2\n");
        let result = read_password(Some(file.path()));

        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "hunter2");
    }

    #[test]
    fn test_read_password_from_file_with_trailing_crlf() {
        let file = create_password_file("hunter2\r\n");
        let result = read_password(Some(file.path()));

        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "hunter2");
    }

    #[test]
    fn test_read_password_from_file_with_multiple_trailing_newlines() {
        let file = create_password_file("hunter2\n\n\n");
        let result = read_password(Some(file.path()));

        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "hunter2");
    }

    #[test]
    fn test_read_password_from_file_missing_file() {
        let missing_path = PathBuf::from("/tmp/nonexistent_password_file_12345.txt");
        let result = read_password(Some(&missing_path));

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unable to read password file"));
    }

    #[test]
    fn test_read_password_from_file_empty_password() {
        let file = create_password_file("");
        let result = read_password(Some(file.path()));

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Password cannot be empty");
    }

    #[test]
    fn test_read_password_from_file_only_whitespace() {
        let file = create_password_file("\n\n\r\n");
        let result = read_password(Some(file.path()));

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Password cannot be empty");
    }

    #[test]
    fn test_read_password_from_file_with_spaces() {
        let file = create_password_file("pass word\n");
        let result = read_password(Some(file.path()));

        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "pass word");
    }

    // === Stdin Tests ===

    #[test]
    fn test_read_password_from_stdin_success() {
        let result = test_read_password_from_stdin(b"hunter2\n");

        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "hunter2");
    }

    #[test]
    fn test_read_password_from_stdin_no_newline() {
        // rpassword reads until newline, so without one it reaches EOF
        let result = test_read_password_from_stdin(b"hunter2");

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("Unable to read password from stdin")
        );
    }

    #[test]
    fn test_read_password_from_stdin_with_crlf() {
        let result = test_read_password_from_stdin(b"hunter2\r\n");

        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "hunter2");
    }

    #[test]
    fn test_read_password_from_stdin_empty_input() {
        let result = test_read_password_from_stdin(b"");

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("Unable to read password from stdin")
        );
    }

    #[test]
    fn test_read_password_from_stdin_only_newline() {
        let result = test_read_password_from_stdin(b"\n");

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Password cannot be empty");
    }

    #[test]
    fn test_read_password_from_stdin_only_whitespace() {
        let result = test_read_password_from_stdin(b"\r\n");

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Password cannot be empty");
    }

    #[test]
    fn test_read_password_from_stdin_with_spaces() {
        let result = test_read_password_from_stdin(b"pass word\n");

        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "pass word");
    }

    #[test]
    fn test_read_password_from_stdin_special_characters() {
        let result = test_read_password_from_stdin(b"p@ssw0rd!#$%\n");

        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "p@ssw0rd!#$%");
    }

    #[test]
    fn test_read_password_from_stdin_unicode() {
        let result = test_read_password_from_stdin("パスワード\n".as_bytes());

        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "パスワード");
    }
}
