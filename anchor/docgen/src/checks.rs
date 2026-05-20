use std::{fs, io::ErrorKind, path::Path};

use crate::errors::DocGenError;

/// Read the content of a file and return as a string.
fn read_content(path: &Path) -> Result<String, DocGenError> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            Err(DocGenError::OutOfDate(path.to_string_lossy().to_string()))
        }
        Err(e) => Err(DocGenError::ReadFile {
            path: path.to_path_buf(),
            source: e,
        }),
    }
}

/// Write generated content to a machine-owned page.
pub fn update_file(path: &Path, generated_content: &str) -> Result<(), DocGenError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| DocGenError::WriteFile {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }

    fs::write(path, generated_content).map_err(|e| DocGenError::WriteFile {
        path: path.to_path_buf(),
        source: e,
    })?;

    Ok(())
}

pub fn append_to_out_of_date_files(
    path: &Path,
    file: &str,
    generated_content: &str,
    out_of_date_files: &mut Vec<String>,
) -> Result<(), DocGenError> {
    match check_file(path, generated_content) {
        Ok(_) => Ok(()),
        Err(DocGenError::OutOfDate(_)) => {
            out_of_date_files.push(file.to_string());
            Ok(())
        }
        Err(e) => Err(e),
    }
}

pub fn check_out_of_date_files(out_of_date_files: &[String]) -> Result<(), DocGenError> {
    if out_of_date_files.is_empty() {
        return Ok(());
    }

    Err(DocGenError::OutOfDate(out_of_date_files.join(", ")))
}

/// Check if a machine-owned page matches generated content.
pub fn check_file(path: &Path, generated_content: &str) -> Result<(), DocGenError> {
    let content = read_content(path)?;
    if content != generated_content {
        return Err(DocGenError::OutOfDate(path.to_string_lossy().to_string()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs::read_to_string, path::Path};

    use super::{append_to_out_of_date_files, check_file, check_out_of_date_files, update_file};
    use crate::errors::DocGenError;

    #[test]
    fn test_update_and_check_file_round_trip_for_generated_page() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mdx");

        let generated = "```text\nUsage: test [OPTIONS]\n```\n";
        update_file(&path, generated).unwrap();

        check_file(&path, generated).unwrap();

        let updated = read_to_string(&path).unwrap();
        assert_eq!(updated, generated);
    }

    #[test]
    fn test_check_file_detects_stale_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mdx");

        update_file(&path, "old content\n").unwrap();

        let result = check_file(&path, "new content\n");
        assert!(result.is_err());
    }

    #[test]
    fn test_check_file_with_missing_path_returns_out_of_date() {
        let path = Path::new("/tmp/docgen_test_invalid_file.mdx");

        let result = check_file(path, "generated");
        assert!(
            matches!(result, Err(DocGenError::OutOfDate(_))),
            "Expected OutOfDate error, got: {result:?}"
        );
    }

    #[test]
    fn test_update_file_with_directory_path_returns_write_file_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        let result = update_file(path, "generated");
        assert!(
            matches!(result, Err(DocGenError::WriteFile { .. })),
            "Expected WriteFile error, got: {result:?}"
        );
    }

    #[test]
    fn test_append_to_out_of_date_files_does_not_append_when_content_matches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mdx");
        let content = "```text\nhelp text\n```\n";
        update_file(&path, content).unwrap();

        let mut out_of_date = Vec::new();
        append_to_out_of_date_files(&path, "test.mdx", content, &mut out_of_date).unwrap();

        assert!(out_of_date.is_empty());
    }

    #[test]
    fn test_append_to_out_of_date_files_appends_filename_when_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mdx");
        update_file(&path, "old content").unwrap();

        let mut out_of_date = Vec::new();
        append_to_out_of_date_files(&path, "test.mdx", "new content", &mut out_of_date).unwrap();

        assert_eq!(out_of_date, vec!["test.mdx"]);
    }

    #[test]
    fn test_append_to_out_of_date_files_appends_filename_when_file_missing() {
        let path = Path::new("/tmp/docgen_nonexistent_file.mdx");

        let mut out_of_date = Vec::new();
        append_to_out_of_date_files(path, "missing.mdx", "content", &mut out_of_date).unwrap();

        assert_eq!(out_of_date, vec!["missing.mdx"]);
    }

    #[test]
    fn test_check_out_of_date_files_returns_ok_when_empty() {
        let result = check_out_of_date_files(&[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_out_of_date_files_returns_error_listing_files() {
        let files = vec![
            "cli-node-options.mdx".to_string(),
            "cli-keygen-options.mdx".to_string(),
        ];

        let result = check_out_of_date_files(&files);
        match result {
            Err(DocGenError::OutOfDate(msg)) => {
                assert!(msg.contains("cli-node-options.mdx"));
                assert!(msg.contains("cli-keygen-options.mdx"));
            }
            other => panic!("Expected OutOfDate error, got: {other:?}"),
        }
    }
}
