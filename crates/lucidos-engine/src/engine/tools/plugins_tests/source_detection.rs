//! Source-string detection + `short_source` + archive-entry validation tests.

use super::source::Source;
use super::*;
use crate::core::plugins::validate_archive_entry_path;

#[test]
fn detect_source_github_tree_url_with_subpath() {
    let s =
        detect_source("https://github.com/lucidos-dev/plugins/tree/main/browser-learning").unwrap();
    match s {
        Source::Git {
            url,
            branch,
            subpath,
        } => {
            assert_eq!(url, "https://github.com/lucidos-dev/plugins.git");
            assert_eq!(branch.as_deref(), Some("main"));
            assert_eq!(subpath.as_deref(), Some("browser-learning"));
        }
        other => panic!("expected Git, got {:?}", other),
    }
}

#[test]
fn detect_source_github_tree_url_without_subpath() {
    let s = detect_source("https://github.com/lucidos-dev/plugin-x/tree/main").unwrap();
    match s {
        Source::Git {
            url,
            branch,
            subpath,
        } => {
            assert_eq!(url, "https://github.com/lucidos-dev/plugin-x.git");
            assert_eq!(branch.as_deref(), Some("main"));
            assert_eq!(subpath, None);
        }
        other => panic!("expected Git, got {:?}", other),
    }
}

#[test]
fn detect_source_plain_https_repo() {
    let s = detect_source("https://github.com/x/y.git").unwrap();
    match s {
        Source::Git {
            url,
            branch,
            subpath,
        } => {
            assert_eq!(url, "https://github.com/x/y.git");
            assert_eq!(branch, None);
            assert_eq!(subpath, None);
        }
        other => panic!("expected Git, got {:?}", other),
    }
}

#[test]
fn detect_source_ssh() {
    let s = detect_source("git@github.com:x/y.git").unwrap();
    assert!(matches!(s, Source::Git { .. }));
}

#[test]
fn detect_source_archive_missing_file() {
    let err = detect_source("/tmp/no-such-thing.lucidos-plugin").unwrap_err();
    assert!(err.contains("not found"));
}

#[test]
fn detect_source_unknown_shape() {
    let err = detect_source("just-a-name").unwrap_err();
    assert!(err.contains("could not infer"));
}

#[test]
fn short_source_strips_https_and_git_suffix() {
    assert_eq!(short_source("https://github.com/a/b.git"), "github.com/a/b");
    assert_eq!(short_source("https://github.com/a/b/"), "github.com/a/b");
}

#[test]
fn validate_archive_entry_path_is_used() {
    // Smoke: the public function in core::plugins still rejects ../.
    assert!(validate_archive_entry_path("a/../b").is_err());
}
