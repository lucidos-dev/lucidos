use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const NEXT_BOOT_FILE: &str = ".next-boot.json";

#[derive(Deserialize, Default)]
struct NextBoot {
    #[serde(default)]
    quit: bool,
    #[serde(default)]
    restore: Vec<String>,
}

pub fn record_path(app_data: &Path) -> PathBuf {
    app_data.join(NEXT_BOOT_FILE)
}

pub fn take(app_data: &Path) -> Vec<String> {
    let path = record_path(app_data);
    let raw = std::fs::read_to_string(&path);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => crate::log_legacy!("[Gateway] could not clear {}: {e}", path.display()),
    }
    let raw = match raw {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            crate::log_legacy!("[Gateway] could not read {}: {e}", path.display());
            return Vec::new();
        }
    };
    match serde_json::from_str::<NextBoot>(&raw) {
        Ok(record) if record.quit => Vec::new(),
        Ok(record) => record.restore,
        Err(e) => {
            crate::log_legacy!("[Gateway] ignoring unreadable {}: {e}", path.display());
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_record(app_data: &Path, body: &str) {
        std::fs::write(record_path(app_data), body).expect("write the record");
    }

    #[test]
    fn a_restore_record_names_the_workspaces_to_bring_back() {
        let tmp = tempfile::tempdir().unwrap();
        write_record(tmp.path(), r#"{"restore":["myws","dev"]}"#);
        assert_eq!(take(tmp.path()), vec!["myws", "dev"]);
    }

    #[test]
    fn a_quit_record_restores_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        write_record(tmp.path(), r#"{"quit":true}"#);
        assert!(take(tmp.path()).is_empty());
    }

    #[test]
    fn taking_the_record_consumes_it() {
        let tmp = tempfile::tempdir().unwrap();
        write_record(tmp.path(), r#"{"restore":["myws"]}"#);
        assert_eq!(take(tmp.path()), vec!["myws"]);
        assert!(!record_path(tmp.path()).exists(), "the record must be gone");
        assert!(
            take(tmp.path()).is_empty(),
            "a second take restores nothing"
        );
    }

    #[test]
    fn a_missing_or_unreadable_record_restores_nothing_and_is_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(take(tmp.path()).is_empty(), "no record at all");

        for body in [r#"{ not json"#, "", r#"{"restore":"myws"}"#] {
            write_record(tmp.path(), body);
            assert!(take(tmp.path()).is_empty(), "unreadable: {body:?}");
            assert!(
                !record_path(tmp.path()).exists(),
                "an unreadable record must not be retried forever: {body:?}",
            );
        }
    }

    #[test]
    fn next_boot_record_shape_matches_the_writer() {
        let tmp = tempfile::tempdir().unwrap();
        write_record(tmp.path(), "{\"restore\":[\"myws\"]}");
        assert_eq!(take(tmp.path()), vec!["myws"]);
        write_record(tmp.path(), "{\"quit\":true}");
        assert!(take(tmp.path()).is_empty());
        assert_eq!(NEXT_BOOT_FILE, ".next-boot.json");
    }
}
