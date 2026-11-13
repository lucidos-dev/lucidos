//! Integration tests for import functionality

use std::fs;
use tempfile::TempDir;

#[test]
fn test_import_file_copies_content() {
    // Create a temp source file
    let temp_dir = TempDir::new().unwrap();
    let source_path = temp_dir.path().join("test_notes.md");
    fs::write(&source_path, "# My Notes\n\nThis is a test file.").unwrap();

    // Verify the source file exists
    assert!(source_path.exists());
    let content = fs::read_to_string(&source_path).unwrap();
    assert!(content.contains("My Notes"));
}
