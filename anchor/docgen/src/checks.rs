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

    use super::{check_file, update_file};

    #[test]
    fn test_update_and_check_file_roundtrip_for_generated_page() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mdx");

        let generated = "# Generated Page\n\n| Option | Description | Default |\n| --- | --- | --- |\n| `--test` | A test | |\n";
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
        let path = Path::new("/tmp/docgen_test_invalid_file.mdx"); // Does not exist.

        let result = check_file(path, "generated");
        assert!(
            matches!(result, Err(crate::errors::DocGenError::OutOfDate(_))),
            "Expected OutOfDate error, got: {result:?}"
        );
    }

    #[test]
    fn test_update_file_with_directory_path_returns_write_file_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        let result = update_file(path, "generated");
        assert!(
            matches!(result, Err(crate::errors::DocGenError::WriteFile { .. })),
            "Expected WriteFile error, got: {result:?}"
        );
    }
}
