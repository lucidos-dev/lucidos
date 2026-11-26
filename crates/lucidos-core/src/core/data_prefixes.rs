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

pub const READ_ONLY_PREFIXES: &[&str] = &[super::knowhow::SYSTEM_KNOWHOW_PREFIX];

pub fn known_data_prefixes() -> impl Iterator<Item = &'static str> {
    MUTABLE_PREFIXES
        .iter()
        .chain(READ_ONLY_PREFIXES.iter())
        .copied()
}

pub fn is_known_data_prefix(relative_path: &str) -> bool {
    known_data_prefixes().any(|p| relative_path.starts_with(p))
}

pub fn is_checked_whole(data_path: &str) -> bool {
    super::themes::is_theme_path(data_path) || super::workspace_fonts::is_font_path(data_path)
}

pub fn validate_data_write(
    data_dir: &std::path::Path,
    data_path: &str,
    bytes: &[u8],
) -> Result<(), String> {
    super::themes::validate_data_write(data_dir, data_path, bytes)?;
    super::workspace_fonts::validate_data_write(data_dir, data_path, bytes)
}

/// INVARIANT: Where `data_path` really lands once every symlink on it resolves, as a
/// path relative to `data_dir`.
///
/// The prefix allowlist reads a string, and a symlink makes the string lie: a
/// link under `artifacts/` can point at `.env`, `postgres/` or anywhere on the
/// host. A caller runs its own prefix rule again on the answer.
///
/// The part of the path that does not exist yet stays unresolved. So a new
/// file under a linked directory still resolves. An entry that exists but
/// cannot resolve, such as a dangling link, is an error: a write through it
/// would create its target, wherever that is.
pub fn resolved_data_path(data_dir: &std::path::Path, data_path: &str) -> Result<String, String> {
    let Ok(root) = data_dir.canonicalize() else {
        return Ok(data_path.to_string());
    };
    let full = data_dir.join(data_path);
    let existing = full
        .ancestors()
        .find(|p| p.symlink_metadata().is_ok())
        .unwrap_or(data_dir);
    let missing = full
        .strip_prefix(existing)
        .expect("an ancestor is a prefix of its path");
    let real = existing
        .canonicalize()
        .map_err(|e| format!("'{data_path}' cannot be resolved: {e}"))?
        .join(missing);
    let relative = real
        .strip_prefix(&root)
        .map_err(|_| format!("'{data_path}' resolves outside data/ through a symlink"))?;
    relative
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| format!("'{data_path}' resolves to a path that is not UTF-8"))
}

pub fn known_data_prefixes_text() -> String {
    known_data_prefixes().collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUARD_AT_USE: &[(&str, &str)] = &[
        ("artifacts/", "none: content the user owns"),
        ("apps/", "none: content the user owns"),
        ("knowhow/", "none: content the user owns"),
        ("triggers/", "none: content the user owns"),
        (
            "config/",
            "credential scope, so `apis.json` cannot send a secret off-scope",
        ),
        (
            "auth-modules/",
            "the wasmtime sandbox, plus credential scope per handle",
        ),
        (
            "scripts/",
            "the handshake approval record: path plus content hash",
        ),
        (
            "themes/",
            "theme validation on write, and again at every apply: tokens only, no `url(`",
        ),
        (
            "fonts/",
            "font validation on write and at every listing (font files by magic bytes, no URL), \
             then the browser's font sanitiser",
        ),
    ];

    #[test]
    fn every_mutable_prefix_states_what_guards_it_at_use() {
        let guarded: Vec<&str> = GUARD_AT_USE.iter().map(|(prefix, _)| *prefix).collect();
        assert_eq!(
            guarded, MUTABLE_PREFIXES,
            "each prefix writable over the API has one row saying what refuses it at use"
        );
        for (prefix, guard) in GUARD_AT_USE {
            assert!(!guard.trim().is_empty(), "{prefix} names no guard");
        }
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

    fn linked_tree() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let ws = tempfile::tempdir().unwrap();
        let data = ws.path().join("data");
        std::fs::create_dir_all(data.join("artifacts")).unwrap();
        std::fs::write(data.join("artifacts/real.md"), "mine").unwrap();
        std::fs::write(data.join(".env"), "KEY=1").unwrap();
        let outside = ws.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "private").unwrap();
        (ws, data, outside)
    }

    #[test]
    fn a_plain_path_resolves_to_itself_whether_or_not_it_exists() {
        let (_ws, data, _) = linked_tree();
        for path in ["artifacts/real.md", "artifacts/new/deep/file.md"] {
            assert_eq!(resolved_data_path(&data, path).unwrap(), path);
        }
    }

    #[test]
    fn a_symlink_resolves_to_the_file_it_reaches() {
        use std::os::unix::fs::symlink;
        let (_ws, data, outside) = linked_tree();
        let artifacts = data.join("artifacts");
        symlink(artifacts.join("real.md"), artifacts.join("alias.md")).unwrap();
        symlink(data.join(".env"), artifacts.join("env")).unwrap();
        symlink(outside.join("secret.txt"), artifacts.join("file-out")).unwrap();
        symlink(&outside, artifacts.join("dir-out")).unwrap();
        symlink(outside.join("absent"), artifacts.join("dangling")).unwrap();

        assert_eq!(
            resolved_data_path(&data, "artifacts/alias.md").unwrap(),
            "artifacts/real.md"
        );
        assert_eq!(resolved_data_path(&data, "artifacts/env").unwrap(), ".env");
        for path in [
            "artifacts/file-out",
            "artifacts/dir-out/secret.txt",
            "artifacts/dir-out/new.md",
            "artifacts/dangling",
        ] {
            assert!(resolved_data_path(&data, path).is_err(), "{path}");
        }
    }

    #[test]
    fn a_data_dir_behind_a_symlink_still_resolves_its_own_files() {
        let (ws, data, _) = linked_tree();
        let linked = ws.path().join("linked-data");
        std::os::unix::fs::symlink(&data, &linked).unwrap();
        assert_eq!(
            resolved_data_path(&linked, "artifacts/real.md").unwrap(),
            "artifacts/real.md"
        );
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
