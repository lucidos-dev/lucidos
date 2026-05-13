use super::*;
use std::fs;

const VALID_MANIFEST: &str = r#"
id = "browser-skills"
version = "0.1.0"
name = "Browser Skills"
description = "Test"
source = "https://github.com/x/y"
"#;

fn write_valid_plugin(root: &std::path::Path) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("manifest.toml"), VALID_MANIFEST).unwrap();
    let kn = root.join("knowhow");
    fs::create_dir_all(&kn).unwrap();
    fs::write(kn.join("a.md"), "---\nname: A\n---\nhi").unwrap();
}

fn tmpdir(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "lucidos_plugins_test_{}_{}",
        name,
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&p).unwrap();
    p
}

// --- parse_manifest ---

#[test]
fn parses_valid_manifest() {
    let m = parse_manifest(VALID_MANIFEST).unwrap();
    assert_eq!(m.id, "browser-skills");
    assert_eq!(m.version, "0.1.0");
    assert_eq!(m.name, "Browser Skills");
    assert_eq!(m.source.as_deref(), Some("https://github.com/x/y"));
    assert_eq!(m.engine, None);
}

#[test]
fn parses_optional_engine() {
    let toml = format!("{}engine = \">=0.5.0\"\n", VALID_MANIFEST);
    let m = parse_manifest(&toml).unwrap();
    assert_eq!(m.engine.as_deref(), Some(">=0.5.0"));
}

#[test]
fn parses_optional_setup_field() {
    let toml = format!(
        "{}setup = \"Set up a daily reflection trigger using `knowhow/browser-learning/reflection.md`. Suggested cron: `0 0 4 * * *`.\"\n",
        VALID_MANIFEST
    );
    let m = parse_manifest(&toml).unwrap();
    assert_eq!(
        m.setup.as_deref(),
        Some("Set up a daily reflection trigger using `knowhow/browser-learning/reflection.md`. Suggested cron: `0 0 4 * * *`.")
    );
}

#[test]
fn setup_is_none_when_absent() {
    let m = parse_manifest(VALID_MANIFEST).unwrap();
    assert_eq!(m.setup, None);
}

#[test]
fn rejects_missing_id() {
    let toml = r#"
version = "0.1.0"
name = "X"
description = "Y"
source = "https://github.com/a/b"
"#;
    assert_eq!(parse_manifest(toml), Err(ValidationError::MissingField("id")));
}

#[test]
fn rejects_invalid_id_uppercase() {
    let toml = r#"
id = "Browser-Skills"
version = "0.1.0"
name = "X"
description = "Y"
source = "https://github.com/a/b"
"#;
    match parse_manifest(toml) {
        Err(ValidationError::InvalidId(_)) => (),
        other => panic!("expected InvalidId, got {:?}", other),
    }
}

#[test]
fn rejects_invalid_id_underscore() {
    let toml = r#"
id = "browser_skills"
version = "0.1.0"
name = "X"
description = "Y"
source = "https://github.com/a/b"
"#;
    assert!(matches!(
        parse_manifest(toml),
        Err(ValidationError::InvalidId(_))
    ));
}

#[test]
fn rejects_too_long_id() {
    let id = "a".repeat(65);
    let toml = format!(
        r#"
id = "{}"
version = "0.1.0"
name = "X"
description = "Y"
source = "https://github.com/a/b"
"#,
        id
    );
    assert!(matches!(
        parse_manifest(&toml),
        Err(ValidationError::InvalidId(_))
    ));
}

#[test]
fn rejects_bad_semver() {
    let toml = r#"
id = "x"
version = "not-a-semver"
name = "X"
description = "Y"
source = "https://github.com/a/b"
"#;
    assert!(matches!(
        parse_manifest(toml),
        Err(ValidationError::InvalidVersion(_))
    ));
}

#[test]
fn accepts_git_at_source() {
    let toml = r#"
id = "x"
version = "0.1.0"
name = "X"
description = "Y"
source = "git@github.com:a/b.git"
"#;
    assert!(parse_manifest(toml).is_ok());
}

#[test]
fn rejects_unparseable_toml() {
    assert!(matches!(
        parse_manifest("this is not = valid toml ["),
        Err(ValidationError::ManifestParseError(_))
    ));
}

#[test]
fn parses_without_source_field() {
    let toml = r#"
id = "x"
version = "0.1.0"
name = "X"
description = "Y"
"#;
    let m = parse_manifest(toml).unwrap();
    assert_eq!(m.source, None);
}

#[test]
fn rejects_bad_source_when_present() {
    let toml = r#"
id = "x"
version = "0.1.0"
name = "X"
description = "Y"
source = "bare-string"
"#;
    assert!(matches!(
        parse_manifest(toml),
        Err(ValidationError::InvalidSource(_))
    ));
}

// --- validate_tree ---

#[test]
fn validates_well_formed_tree() {
    let dir = tmpdir("ok");
    write_valid_plugin(&dir);
    assert!(validate_tree(&dir).is_ok());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn rejects_missing_manifest() {
    let dir = tmpdir("nomanifest");
    let kn = dir.join("knowhow");
    fs::create_dir_all(&kn).unwrap();
    fs::write(kn.join("a.md"), "x").unwrap();
    assert_eq!(validate_tree(&dir), Err(ValidationError::MissingManifest));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn rejects_unexpected_top_level_dir() {
    let dir = tmpdir("badtoplevel");
    write_valid_plugin(&dir);
    fs::create_dir(dir.join("__MACOSX")).unwrap();
    fs::write(dir.join("__MACOSX/junk"), "").unwrap();
    match validate_tree(&dir) {
        Err(ValidationError::UnexpectedTopLevelEntry(name)) => {
            assert_eq!(name, "__MACOSX");
        }
        other => panic!("expected UnexpectedTopLevelEntry, got {:?}", other),
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn rejects_unexpected_top_level_file() {
    let dir = tmpdir("rootreadme");
    write_valid_plugin(&dir);
    fs::write(dir.join("README.md"), "hi").unwrap();
    assert!(matches!(
        validate_tree(&dir),
        Err(ValidationError::UnexpectedTopLevelEntry(_))
    ));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn rejects_empty_tree() {
    let dir = tmpdir("empty");
    fs::write(dir.join("manifest.toml"), VALID_MANIFEST).unwrap();
    fs::create_dir_all(dir.join("knowhow")).unwrap();
    assert_eq!(validate_tree(&dir), Err(ValidationError::EmptyTree));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn validates_tree_with_only_auth_modules() {
    let dir = tmpdir("authonly");
    fs::write(dir.join("manifest.toml"), VALID_MANIFEST).unwrap();
    let am = dir.join("auth-modules");
    fs::create_dir_all(&am).unwrap();
    fs::write(am.join("acme.wasm"), b"\0asm").unwrap();
    fs::write(am.join("acme.manifest.json"), "{}").unwrap();
    let (_, planned) = validate_tree(&dir).unwrap();
    let paths: Vec<&str> = planned.iter().map(|p| p.data_relative.as_str()).collect();
    assert!(paths.contains(&"auth-modules/acme.wasm"));
    assert!(paths.contains(&"auth-modules/acme.manifest.json"));
    let _ = fs::remove_dir_all(&dir);
}

// --- validate_archive_entry_path (zip-slip) ---

#[test]
fn rejects_parent_traversal() {
    assert!(matches!(
        validate_archive_entry_path("foo/../../etc/passwd"),
        Err(ValidationError::UnsafePath(_))
    ));
}

#[test]
fn rejects_absolute_unix() {
    assert!(matches!(
        validate_archive_entry_path("/etc/passwd"),
        Err(ValidationError::UnsafePath(_))
    ));
}

#[test]
fn rejects_absolute_windows() {
    assert!(matches!(
        validate_archive_entry_path("\\windows\\system32"),
        Err(ValidationError::UnsafePath(_))
    ));
}

#[test]
fn accepts_safe_relative() {
    assert!(validate_archive_entry_path("knowhow/a.md").is_ok());
}

#[test]
fn rejects_empty_path() {
    // Empty inner names produce an opaque "not found" downstream — reject at the
    // validator instead of expecting every caller to add a separate guard.
    assert!(matches!(
        validate_archive_entry_path(""),
        Err(ValidationError::UnsafePath(_))
    ));
}

// --- plan_files / detect_conflicts ---

#[test]
fn plans_files_under_known_dirs_only() {
    let dir = tmpdir("plan");
    write_valid_plugin(&dir);
    let triggers = dir.join("triggers/morning");
    fs::create_dir_all(&triggers).unwrap();
    fs::write(triggers.join("morning.md"), "x").unwrap();
    // Hidden files skipped
    fs::write(dir.join("knowhow/.DS_Store"), "x").unwrap();

    let planned = plan_files(&dir);
    let paths: Vec<&str> = planned.iter().map(|p| p.data_relative.as_str()).collect();
    assert_eq!(paths, vec!["knowhow/a.md", "triggers/morning/morning.md"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn detects_conflicts_against_data_dir() {
    let plugin_dir = tmpdir("plan_conflict");
    write_valid_plugin(&plugin_dir);
    let planned = plan_files(&plugin_dir);

    let data_dir = tmpdir("data_conflict");
    fs::create_dir_all(data_dir.join("knowhow")).unwrap();
    fs::write(data_dir.join("knowhow/a.md"), "existing").unwrap();

    let conflicts = detect_conflicts(&planned, &data_dir);
    assert_eq!(conflicts, vec!["knowhow/a.md"]);

    let empty_data = tmpdir("data_no_conflict");
    let conflicts2 = detect_conflicts(&planned, &empty_data);
    assert!(conflicts2.is_empty());

    let _ = fs::remove_dir_all(&plugin_dir);
    let _ = fs::remove_dir_all(&data_dir);
    let _ = fs::remove_dir_all(&empty_data);
}

// --- compare_versions ---

#[test]
fn compare_versions_update_when_remote_newer() {
    assert_eq!(
        compare_versions("0.1.0", "0.2.0"),
        UpdateDecision::Update
    );
}

#[test]
fn compare_versions_already_when_equal() {
    assert_eq!(
        compare_versions("1.4.0", "1.4.0"),
        UpdateDecision::AlreadyLatest
    );
}

#[test]
fn compare_versions_already_when_remote_older() {
    assert_eq!(
        compare_versions("2.0.0", "1.0.0"),
        UpdateDecision::AlreadyLatest
    );
}

#[test]
fn compare_versions_treats_garbage_as_update() {
    assert_eq!(
        compare_versions("garbage", "1.0.0"),
        UpdateDecision::Update
    );
}
