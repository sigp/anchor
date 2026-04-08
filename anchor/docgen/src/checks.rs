use std::{fs, path::Path};

use crate::{
    errors::DocGenError,
    render::{CLI_REFERENCE_END, CLI_REFERENCE_START},
};

fn frame_generated_content(content: &str) -> String {
    format!("\n{content}")
}

/// Replace content between sentinel markers in a file.
pub fn update_file(path: &Path, generated_content: &str) -> Result<(), DocGenError> {
    let content = fs::read_to_string(path).map_err(|e| DocGenError::ReadFile {
        path: path.to_path_buf(),
        source: e,
    })?;

    let (before, after) = split_at_markers(&content, path)?;

    let new_content =
        format!("{before}{CLI_REFERENCE_START}{}{CLI_REFERENCE_END}{after}", frame_generated_content(generated_content));

    fs::write(path, new_content).map_err(|e| DocGenError::WriteFile {
        path: path.to_path_buf(),
        source: e,
    })?;

    Ok(())
}

fn start_before_end_idx(start_idx: usize, end_idx: usize, path: &Path) -> Result<(), DocGenError> {
    if start_idx < end_idx {
        Ok(())
    } else {
        Err(DocGenError::InvalidMarkers {
            path: path.to_string_lossy().to_string(),
            source: std::fmt::Error,
        })
    }
}

/// Check if content between sentinel markers matches generated content.
pub fn check_file(path: &Path, generated_content: &str) -> Result<(), DocGenError> {
    let content = fs::read_to_string(path).map_err(|e| DocGenError::ReadFile {
        path: path.to_path_buf(),
        source: e,
    })?;

    let start_idx =
        content
            .find(CLI_REFERENCE_START)
            .ok_or_else(|| DocGenError::MissingMarker {
                path: path.to_path_buf(),
                marker: CLI_REFERENCE_START.to_string(),
            })?;
    let end_idx = content
        .find(CLI_REFERENCE_END)
        .ok_or_else(|| DocGenError::MissingMarker {
            path: path.to_path_buf(),
            marker: CLI_REFERENCE_END.to_string(),
        })?;
    start_before_end_idx(start_idx, end_idx, path)?;

    let existing = &content[start_idx + CLI_REFERENCE_START.len()..end_idx];
    let expected = frame_generated_content(generated_content);

    if existing != expected {
        return Err(DocGenError::OutOfDate(path.to_string_lossy().to_string()));
    }

    Ok(())
}

/// Split file content at the sentinel markers, returning (before_start, after_end).
fn split_at_markers<'a>(content: &'a str, path: &Path) -> Result<(&'a str, &'a str), DocGenError> {
    let start_idx =
        content
            .find(CLI_REFERENCE_START)
            .ok_or_else(|| DocGenError::MissingMarker {
                path: path.to_path_buf(),
                marker: CLI_REFERENCE_START.to_string(),
            })?;
    let end_idx = content
        .find(CLI_REFERENCE_END)
        .ok_or_else(|| DocGenError::MissingMarker {
            path: path.to_path_buf(),
            marker: CLI_REFERENCE_END.to_string(),
        })?;
    start_before_end_idx(start_idx, end_idx, path)?;

    let before = &content[..start_idx];
    let after = &content[end_idx + CLI_REFERENCE_END.len()..];

    Ok((before, after))
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{read_to_string, write as fs_write},
        path::Path,
    };

    use super::{
        CLI_REFERENCE_END, CLI_REFERENCE_START, check_file, split_at_markers, update_file,
    };

    #[test]
    fn test_update_and_check_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mdx");

        let initial = format!(
            "# Header\n\nSome prose.\n\n{CLI_REFERENCE_START}\nold content\n{CLI_REFERENCE_END}\n\n## Examples\n"
        );
        fs_write(&path, &initial).unwrap();

        let generated = "### Options\n\n| Option | Description | Default |\n| --- | --- | --- |\n| `--test` | A test | |\n\n";
        update_file(&path, generated).unwrap();

        check_file(&path, generated).unwrap();

        let updated = read_to_string(&path).unwrap();
        assert!(updated.contains("# Header"));
        assert!(updated.contains("Some prose."));
        assert!(updated.contains("## Examples"));
    }

    #[test]
    fn test_check_file_detects_stale_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mdx");

        let content = format!("{CLI_REFERENCE_START}\nold content\n{CLI_REFERENCE_END}\n");
        fs_write(&path, &content).unwrap();

        let result = check_file(&path, "new content\n");
        assert!(result.is_err());
    }

    #[test]
    fn test_split_at_markers_correctly_splits_content() {
        let content = format!(
            "Intro text\n{CLI_REFERENCE_START}\nGenerated content\n{CLI_REFERENCE_END}\nOutro text"
        );
        let (before, after) = split_at_markers(&content, Path::new("test.mdx")).unwrap();

        assert_eq!(before, "Intro text\n");
        assert_eq!(after, "\nOutro text");
    }
}
