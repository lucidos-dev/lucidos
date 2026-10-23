//! The typed `data/` subdirectories, the *data prefixes*: the one list every
//! surface that turns a caller's path into a `data/` path reads.
//!
//! Everything else under the `data/` root is gitignored config (`.env`,
//! `postgres/`) that no caller-supplied path may reach. A traversal check alone
//! stops a path leaving `data/`, not reaching `.env` inside it. Readers: the HTTP data
//! route, the agent file tools (`normalize_data_path`), email attachments, and,
//! through source-reading pin tests, the `lucidos` CLI and the frontend.

/// Mutable workspace-data prefixes: user-owned trees the API may write and
/// delete.
///
/// The write side cannot tell an app from the shell. So each of these is
/// guarded at the point of USE instead (ADR 0156 decision 2). A new prefix
/// states its answer in this table, and the test below fails if it does not.
///
/// | Prefix | Guard at use |
/// |---|---|
/// | `artifacts/`, `apps/`, `knowhow/`, `triggers/` | none: content the user owns |
/// | `config/` | credential scope, so `apis.json` cannot send a secret off-scope |
/// | `auth-modules/` | the wasmtime sandbox, plus credential scope per handle |
/// | `scripts/` | the handshake approval record: path plus content hash |
/// | `themes/` | theme validation on write, and again at every apply: tokens only, no `url(` |
/// | `fonts/` | font validation on write and at every listing (font files by magic bytes, no URL), then the browser's font sanitiser |
///
/// `config/` and `auth-modules/` are coupled: `config/apis.json` references
/// signers by name from `auth-modules/`, so deleting one without the other
/// leaves a dangling reference.
pub const MUTABLE_PREFIXES: &[&str] = &[
    "artifacts/",
    "apps/",
    "knowhow/",
    "triggers/",
    "config/",
    "auth-modules/",
    "scripts/",
    "themes/",
    "fonts/",
];

/// Engine-shipped reference knowhow served from `<repo>/system-knowhow/`.
/// Readable, never writable.
pub const READ_ONLY_PREFIXES: &[&str] = &[super::knowhow::SYSTEM_KNOWHOW_PREFIX];

/// Every data prefix, mutable first.
pub fn known_data_prefixes() -> impl Iterator<Item = &'static str> {
    MUTABLE_PREFIXES
        .iter()
        .chain(READ_ONLY_PREFIXES.iter())
        .copied()
}

/// Whether a `data/`-relative path names one of the data prefixes.
pub fn is_known_data_prefix(relative_path: &str) -> bool {
    known_data_prefixes().any(|p| relative_path.starts_with(p))
}

/// Whether a data path lies in a tree whose files are checked whole before
/// they are written: `themes/` and `fonts/`. Such a file is written or copied
/// whole, never edited in place, and every change to it is announced, so an
/// open picker refreshes.
pub fn is_checked_whole(data_path: &str) -> bool {
    super::themes::is_theme_path(data_path) || super::workspace_fonts::is_font_path(data_path)
}

/// The check every writer of a `data/` path runs before the bytes reach disk:
/// the data route, `write_file` and `copy_file`. A path outside the checked
/// trees passes. One function, so the surfaces cannot disagree.
pub fn validate_data_write(
    data_dir: &std::path::Path,
    data_path: &str,
    bytes: &[u8],
) -> Result<(), String> {
    super::themes::validate_data_write(data_dir, data_path, bytes)?;
    super::workspace_fonts::validate_data_write(data_dir, data_path, bytes)
}

/// The data prefixes as one comma-separated line, for a refusal to name.
pub fn known_data_prefixes_text() -> String {
    known_data_prefixes().collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every mutable prefix names what stops it at the point of use.
    ///
    /// No header tells an app from the shell (ADR 0156 decision 1). So a
    /// prefix is safe because of what refuses it downstream, never because of
    /// who wrote it. `scripts/` and `config/` sat here for a long time with
    /// that answer written down nowhere, and a write-then-execute chain grew
    /// in the gap. Adding a prefix now costs one table row.
    #[test]
    fn every_mutable_prefix_states_what_guards_it_at_use() {
        // The doc block immediately above the const, read out of this file's
        // own source. A table in a comment nothing checks is a table that goes
        // stale the first time someone is in a hurry.
        let source = include_str!("data_prefixes.rs");
        let (before, _) = source
            .split_once("pub const MUTABLE_PREFIXES")
            .expect("the const this test is about");
        let doc: String = before
            .lines()
            .rev()
            .take_while(|l| l.trim_start().starts_with("///"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            doc.contains("Guard at use"),
            "the doc block above MUTABLE_PREFIXES must carry the guard table"
        );
        for prefix in MUTABLE_PREFIXES {
            assert!(
                doc.contains(&format!("`{prefix}`")),
                "{prefix} is writable over the API and the table does not say \
                 what refuses it at the point of use"
            );
        }
        // The check can say no. Without this, a long table would pass the
        // loop above whatever it actually listed.
        assert!(
            !doc.contains("`postgres/`"),
            "a prefix absent from the table must not read as present"
        );
    }

    #[test]
    fn the_data_root_config_is_not_a_data_prefix() {
        for path in [".env", "postgres/pg.conf", "blobs/ab/x.png", "themesy.json"] {
            assert!(!is_known_data_prefix(path), "{path}");
        }
        assert!(is_known_data_prefix("themes/harbour.json"));
        assert!(is_known_data_prefix("fonts/brand/font.json"));
        assert!(is_known_data_prefix("system-knowhow/themes.md"));
    }

    #[test]
    fn themes_and_fonts_are_the_trees_checked_whole() {
        for path in [
            "themes/harbour.json",
            "fonts/brand/font.json",
            "fonts/brand/a.woff2",
        ] {
            assert!(is_checked_whole(path), "{path}");
        }
        for path in [
            "artifacts/fonts/a.woff2",
            "fontsy/a",
            "themesy.json",
            "fonts",
        ] {
            assert!(!is_checked_whole(path), "{path}");
        }
    }

    #[test]
    fn a_font_write_is_validated_and_other_paths_are_not() {
        let dir = std::path::Path::new("/nonexistent");
        let manifest = br#"{"label":"Brand","group":"sans","faces":[{"file":"a.woff2"}]}"#;
        assert!(validate_data_write(dir, "fonts/brand/font.json", manifest).is_ok());
        assert!(validate_data_write(dir, "fonts/brand/a.woff2", b"wOF2 bytes").is_ok());
        for (path, body) in [
            ("fonts/brand/index.html", b"<script>".as_slice()),
            ("fonts/brand/a.woff2", b"<html>".as_slice()),
            ("fonts/brand/font.json", b"{}".as_slice()),
            ("fonts/loose.woff2", b"wOF2 bytes".as_slice()),
        ] {
            assert!(
                validate_data_write(dir, path, body).is_err(),
                "accepted {path}"
            );
        }
        assert!(validate_data_write(dir, "artifacts/a.woff2", b"<html>").is_ok());
    }

    #[test]
    fn a_theme_may_name_only_an_installed_workspace_font_that_fits() {
        let dir = tempfile::tempdir().unwrap();
        let font = dir.path().join("fonts/brand");
        std::fs::create_dir_all(&font).unwrap();
        std::fs::write(
            font.join("font.json"),
            r#"{"label":"Brand","group":"sans","faces":[{"file":"a.woff2"}]}"#,
        )
        .unwrap();
        std::fs::write(font.join("a.woff2"), b"wOF2 bytes").unwrap();
        let write =
            |theme: &str| validate_data_write(dir.path(), "themes/mine.json", theme.as_bytes());

        assert!(write(r#"{"name":"x","fonts":{"ui":"ws-brand"}}"#).is_ok());
        let missing = write(r#"{"name":"x","fonts":{"ui":"ws-gone"}}"#).unwrap_err();
        assert!(
            missing.contains("not an installed workspace font"),
            "{missing}"
        );
        let unfit = write(r#"{"name":"x","fonts":{"mono":"ws-brand"}}"#).unwrap_err();
        assert!(unfit.contains("cannot be the code font"), "{unfit}");
    }
}
