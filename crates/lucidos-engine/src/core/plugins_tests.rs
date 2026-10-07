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
}

// --- check_engine_requirement ---

fn manifest_with_engine(engine_line: &str) -> serde_json::Value {
    parse_manifest(&format!("{VALID_MANIFEST}{engine_line}\n"))
        .unwrap()
        .raw
}

fn v(s: &str) -> Result<semver::Version, String> {
    Ok(semver::Version::parse(s).unwrap())
}

#[test]
fn engine_requirement_met_by_the_same_and_newer_releases() {
    let m = manifest_with_engine(r#"engine = ">=0.46.1""#);
    assert_eq!(check_engine_requirement(&m, &v("0.46.1")), Ok(()));
    assert_eq!(check_engine_requirement(&m, &v("0.99.0")), Ok(()));
}

#[test]
fn engine_requirement_unmet_names_plugin_requirement_and_running_release() {
    let m = manifest_with_engine(r#"engine = ">=0.46.1""#);
    let mismatch = check_engine_requirement(&m, &v("0.46.0")).unwrap_err();
    assert_eq!(
        mismatch,
        EngineMismatch::Unsatisfied {
            requirement: ">=0.46.1".into(),
            running: semver::Version::new(0, 46, 0),
        }
    );
    assert_eq!(mismatch.short_reason(), "Needs Lucidos 0.46.1 or later");
    assert_eq!(
        mismatch.refusal("Theme Studio", "0.1.0"),
        "Theme Studio 0.1.0 needs Lucidos 0.46.1 or later. This is Lucidos 0.46.0. \
         Update Lucidos first."
    );
}

#[test]
fn a_requirement_that_is_not_a_floor_is_quoted_as_written() {
    let m = manifest_with_engine(r#"engine = ">=0.40, <0.46""#);
    let mismatch = check_engine_requirement(&m, &v("0.46.1")).unwrap_err();
    assert_eq!(mismatch.short_reason(), "Needs Lucidos >=0.40, <0.46");
}

#[test]
fn an_invalid_engine_string_is_refused_with_the_value_quoted() {
    let m = manifest_with_engine(r#"engine = "the latest one""#);
    let mismatch = check_engine_requirement(&m, &v("0.46.1")).unwrap_err();
    assert_eq!(
        mismatch,
        EngineMismatch::Invalid {
            value: "\"the latest one\"".into()
        }
    );
    let refusal = mismatch.refusal("Theme Studio", "0.1.0");
    assert!(refusal.contains("engine = \"the latest one\""), "{refusal}");
    assert!(refusal.starts_with("Theme Studio 0.1.0 "), "{refusal}");
}

#[test]
fn an_empty_or_non_string_engine_is_refused() {
    for line in [r#"engine = """#, "engine = 46", r#"engine = [">=0.46.1"]"#] {
        let m = manifest_with_engine(line);
        assert!(
            matches!(
                check_engine_requirement(&m, &v("0.46.1")),
                Err(EngineMismatch::Invalid { .. })
            ),
            "{line} must be refused"
        );
    }
}

#[test]
fn a_manifest_without_engine_takes_any_release_even_an_unreadable_one() {
    let m = parse_manifest(VALID_MANIFEST).unwrap().raw;
    assert_eq!(check_engine_requirement(&m, &v("0.1.0")), Ok(()));
    assert_eq!(
        check_engine_requirement(&m, &Err("no RELEASE".into())),
        Ok(())
    );
    assert_eq!(engine_requirement_of(&m), None);
}

#[test]
fn a_requirement_against_an_unreadable_release_is_refused() {
    let m = manifest_with_engine(r#"engine = ">=0.46.1""#);
    let mismatch = check_engine_requirement(&m, &Err("no RELEASE".into())).unwrap_err();
    assert_eq!(
        mismatch,
        EngineMismatch::UnknownRelease {
            error: "no RELEASE".into()
        }
    );
    assert!(mismatch.refusal("P", "1.0.0").ends_with("no RELEASE"));
}

#[test]
fn a_prerelease_running_version_compares_as_its_release() {
    let m = manifest_with_engine(r#"engine = ">=0.46.1""#);
    assert_eq!(check_engine_requirement(&m, &v("0.46.2-dev")), Ok(()));
    assert_eq!(check_engine_requirement(&m, &v("0.46.1-rc.1+abc")), Ok(()));
    let mismatch = check_engine_requirement(&m, &v("0.46.0-dev")).unwrap_err();
    assert!(mismatch
        .refusal("P", "1.0.0")
        .contains("This is Lucidos 0.46.0."));
}

/// A dirty build of RELEASE 0.46.1 is the dev build of 0.46.2, so it meets a
/// 0.46.2 floor. `reported_release` is the rule `running_release` applies.
#[test]
fn a_dirty_dev_build_counts_as_the_release_it_is_becoming() {
    let m = manifest_with_engine(r#"engine = ">=0.46.2""#);
    let clean = crate::engine::release_notices::reported_release("0.46.1", false);
    let dirty = crate::engine::release_notices::reported_release("0.46.1", true);
    assert!(check_engine_requirement(&m, &clean).is_err());
    assert_eq!(check_engine_requirement(&m, &dirty), Ok(()));
}

#[test]
fn engine_requirement_of_returns_the_value_as_authored() {
    let m = manifest_with_engine(r#"engine = ">=0.46.1""#);
    assert_eq!(engine_requirement_of(&m).as_deref(), Some(">=0.46.1"));
    let m = manifest_with_engine("engine = 46");
    assert_eq!(engine_requirement_of(&m).as_deref(), Some("46"));
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
    assert_eq!(
        parse_manifest(toml),
        Err(ValidationError::MissingField("id"))
    );
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
    let id = "a".repeat(super::MAX_ID_LEN + 1);
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

/// Themes were called looks, and a plugin built then ships `looks/`. It is
/// refused like any unknown folder, but told which name to use instead.
#[test]
fn a_legacy_looks_folder_is_refused_naming_themes() {
    let dir = tmpdir("legacylooks");
    write_valid_plugin(&dir);
    fs::create_dir(dir.join("looks")).unwrap();
    fs::write(dir.join("looks/harbour.json"), "{}").unwrap();
    let err = validate_tree(&dir).expect_err("a looks/ folder must be refused");
    assert_eq!(
        err,
        ValidationError::UnexpectedTopLevelEntry("looks".into())
    );
    assert!(err.to_string().contains("themes/"), "{err}");
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
fn rejects_a_tree_holding_only_build_output() {
    let dir = tmpdir("only_build_output");
    fs::write(dir.join("manifest.toml"), VALID_MANIFEST).unwrap();
    fs::create_dir_all(dir.join("scripts/__pycache__")).unwrap();
    fs::write(dir.join("scripts/__pycache__/run.cpython-314.pyc"), "x").unwrap();
    fs::write(dir.join("scripts/run.pyc"), "x").unwrap();
    fs::create_dir_all(dir.join("apps/a/node_modules")).unwrap();
    fs::write(dir.join("apps/a/node_modules/y.js"), "x").unwrap();
    assert_eq!(validate_tree(&dir), Err(ValidationError::EmptyTree));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn rejects_an_item_named_like_build_output() {
    for item in ["apps/dist", "triggers/build", "fonts/out"] {
        let dir = tmpdir("item_named_build_output");
        write_valid_plugin(&dir);
        fs::create_dir_all(dir.join(item)).unwrap();
        fs::write(dir.join(item).join("index.html"), "x").unwrap();
        assert_eq!(
            validate_tree(&dir),
            Err(ValidationError::ItemNamedLikeBuildOutput(item.into()))
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

#[test]
fn accepts_build_output_below_an_item_or_hidden() {
    let dir = tmpdir("build_output_below_item");
    write_valid_plugin(&dir);
    fs::create_dir_all(dir.join("apps/a/dist")).unwrap();
    fs::write(dir.join("apps/a/index.html"), "x").unwrap();
    fs::write(dir.join("apps/a/dist/bundle.js"), "x").unwrap();
    fs::create_dir_all(dir.join("apps/.venv")).unwrap();
    let (_, planned) = validate_tree(&dir).unwrap();
    let paths: Vec<&str> = planned.iter().map(|p| p.data_relative.as_str()).collect();
    assert_eq!(paths, vec!["apps/a/index.html", "knowhow/a.md"]);
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

fn plugin_with_theme(name: &str, file: &str, body: &str) -> std::path::PathBuf {
    let dir = tmpdir(name);
    fs::write(dir.join("manifest.toml"), VALID_MANIFEST).unwrap();
    let themes = dir.join("themes");
    fs::create_dir_all(&themes).unwrap();
    fs::write(themes.join(file), body).unwrap();
    dir
}

#[test]
fn validates_tree_with_only_themes() {
    let dir = plugin_with_theme(
        "themes",
        "lagoon.json",
        r##"{"name":"Lagoon","dark":{"--accent":"#3aa3c9"}}"##,
    );
    let (_, planned) = validate_tree(&dir).unwrap();
    let paths: Vec<&str> = planned.iter().map(|p| p.data_relative.as_str()).collect();
    assert_eq!(paths, vec!["themes/lagoon.json"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn rejects_a_theme_that_fails_validation() {
    for (file, body) in [
        (
            "leaky.json",
            r#"{"name":"Leaky","dark":{"--bg-primary":"url(https://example.com)"}}"#,
        ),
        ("nord.json", r#"{"name":"Shadow"}"#),
        ("typo.json", r#"{"name":"Typo","fonts":{"ui":"geyst"}}"#),
        (
            "serif-code.json",
            r#"{"name":"Serif Code","fonts":{"mono":"lora"}}"#,
        ),
        ("Bad Id.json", r#"{"name":"Bad"}"#),
        ("notes.txt", "not a theme"),
    ] {
        let dir = plugin_with_theme("badtheme", file, body);
        match validate_tree(&dir) {
            Err(ValidationError::InvalidTheme(path, _)) => {
                assert_eq!(path, format!("themes/{file}"))
            }
            other => panic!("{file}: expected InvalidTheme, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }
}

/// ADR 0309: a plugin's hostile theme fails staging with the reason, so the
/// user sees why at install time rather than a theme that never shows.
#[test]
fn rejects_a_hostile_theme_with_its_reason() {
    for (file, body, reason) in [
        (
            "blank.json",
            r##"{"name":"Blank","dark":{"--bg-primary":"#101010","--text-primary":"#101010"}}"##,
            "--text-primary on --bg-primary",
        ),
        (
            "swapped.json",
            r##"{"name":"Swapped","dark":{"--accent-green":"#f85149","--accent-red":"#3fb950"}}"##,
            "--accent-green reads as red",
        ),
        (
            "stacked.json",
            r#"{"name":"Stacked","tokens":{"--z-modal":"0"}}"#,
            "not a theme token",
        ),
        (
            "glare.json",
            r#"{"name":"Glare","dark":{"parts":{"chat-text":{"text-shadow":"0 0 2em red"}}}}"#,
            "dark.parts.chat-text.text-shadow: blur 2em is over the 0.6em cap.",
        ),
        (
            "cover.json",
            r#"{"name":"Cover","parts":{"question-card":{"color":"red"}}}"#,
            "parts.question-card: a protected surface, so a theme cannot style it.",
        ),
    ] {
        let dir = plugin_with_theme("hostile", file, body);
        let err = validate_tree(&dir).expect_err(file).to_string();
        assert!(
            err.contains(&format!("themes/{file} is not a valid theme")),
            "{err}"
        );
        assert!(err.contains(reason), "{file}: {err}");
        let _ = fs::remove_dir_all(&dir);
    }
}

/// A plugin shipping `fonts/<slug>/` with the given manifest and files.
fn plugin_with_font(name: &str, manifest: &str, files: &[(&str, &[u8])]) -> std::path::PathBuf {
    let dir = tmpdir(name);
    fs::write(dir.join("manifest.toml"), VALID_MANIFEST).unwrap();
    let font = dir.join("fonts/brand");
    fs::create_dir_all(&font).unwrap();
    fs::write(font.join("font.json"), manifest).unwrap();
    for (file, bytes) in files {
        fs::write(font.join(file), bytes).unwrap();
    }
    dir
}

const MONO_FONT: &str = r#"{"label":"Brand Mono","group":"mono","faces":[{"file":"a.woff2"}]}"#;

#[test]
fn validates_a_plugin_that_ships_a_font_and_a_theme_naming_it() {
    let dir = plugin_with_font("font", MONO_FONT, &[("a.woff2", b"wOF2 bytes")]);
    fs::create_dir_all(dir.join("themes")).unwrap();
    fs::write(
        dir.join("themes/branded.json"),
        r#"{"name":"Branded","fonts":{"ui":"ws-brand","mono":"ws-brand"}}"#,
    )
    .unwrap();
    let (_, planned) = validate_tree(&dir).unwrap();
    let paths: Vec<&str> = planned.iter().map(|p| p.data_relative.as_str()).collect();
    assert!(paths.contains(&"fonts/brand/font.json"), "{paths:?}");
    assert!(paths.contains(&"fonts/brand/a.woff2"), "{paths:?}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn rejects_a_font_that_fails_validation() {
    for (name, files) in [
        ("missing-face", vec![]),
        ("not-a-font", vec![("a.woff2", b"<html>".as_slice())]),
        (
            "stray-file",
            vec![
                ("a.woff2", b"wOF2 bytes".as_slice()),
                ("readme.html", b"<p>"),
            ],
        ),
    ] {
        let dir = plugin_with_font(name, MONO_FONT, &files);
        match validate_tree(&dir) {
            Err(ValidationError::InvalidFont(path, _)) => {
                assert!(path.starts_with("fonts/brand"), "{name}: {path}")
            }
            other => panic!("{name}: expected InvalidFont, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }
}

/// A plugin is self-contained: it cannot lean on a font the user installed.
#[test]
fn rejects_a_theme_naming_a_workspace_font_the_plugin_does_not_ship() {
    let dir = plugin_with_theme(
        "foreignfont",
        "branded.json",
        r#"{"name":"Branded","fonts":{"ui":"ws-brand"}}"#,
    );
    match validate_tree(&dir) {
        Err(ValidationError::InvalidTheme(_, reason)) => {
            assert!(
                reason.contains("not an installed workspace font"),
                "{reason}"
            )
        }
        other => panic!("expected InvalidTheme, got {other:?}"),
    }
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
fn plan_files_skips_build_output() {
    let dir = tmpdir("plan_build_output");
    write_valid_plugin(&dir);
    for (rel, body) in [
        ("scripts/run.py", "print(1)"),
        ("scripts/__pycache__/x.pyc", "x"),
        ("scripts/.venv/lib/site.py", "x"),
        ("apps/a/index.html", "<h1>a</h1>"),
        ("apps/a/node_modules/y.js", "x"),
        ("knowhow/foo.pyc", "x"),
        ("knowhow/bar.pyo", "x"),
    ] {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    let planned = plan_files(&dir);
    let paths: Vec<&str> = planned.iter().map(|p| p.data_relative.as_str()).collect();
    assert_eq!(
        paths,
        vec!["apps/a/index.html", "knowhow/a.md", "scripts/run.py"]
    );
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

/// Install copies a plugin's files past the write-time check, so the font
/// cap is asked here, against the fonts already installed.
#[test]
fn refuses_a_plugin_font_that_would_pass_the_workspace_cap() {
    use crate::core::workspace_fonts::MAX_FONTS;
    let dir = plugin_with_font("roomy", MONO_FONT, &[("a.woff2", b"wOF2 bytes")]);
    let (_, planned) = validate_tree(&dir).unwrap();
    let data_dir = tmpdir("full_fonts");
    for i in 0..MAX_FONTS {
        let font = data_dir.join(format!("fonts/f{i:03}"));
        fs::create_dir_all(&font).unwrap();
        fs::write(font.join("font.json"), MONO_FONT).unwrap();
        fs::write(font.join("a.woff2"), b"wOF2 bytes").unwrap();
    }
    let err = check_font_room(&planned, &data_dir).unwrap_err();
    assert!(err.contains("at most"), "{err}");
    // Replacing a font the workspace already has needs no room.
    fs::rename(data_dir.join("fonts/f000"), data_dir.join("fonts/brand")).unwrap();
    assert!(check_font_room(&planned, &data_dir).is_ok());
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&data_dir);
}

// --- symlink safety ---

#[cfg(unix)]
#[test]
fn plan_files_never_follows_a_symlink_out_of_the_tree() {
    use std::os::unix::fs::symlink;
    // A host secret and a host directory the plugin's symlinks would reach.
    let outside = tmpdir("plan_symlink_outside");
    let secret = outside.join("id_rsa");
    fs::write(&secret, "PRIVATE KEY").unwrap();
    let outside_dir = outside.join("etc");
    fs::create_dir_all(&outside_dir).unwrap();
    fs::write(outside_dir.join("passwd"), "root:x:0:0").unwrap();

    let dir = tmpdir("plan_symlink");
    write_valid_plugin(&dir);
    // A file symlink and a dir symlink inside a content dir, both aimed outside.
    symlink(&secret, dir.join("knowhow/leak.md")).unwrap();
    symlink(&outside_dir, dir.join("knowhow/escape")).unwrap();

    // Only the real file is planned. Neither symlink is, and the dir symlink is
    // not walked, so nothing behind them can be copied into `data/`.
    let planned = plan_files(&dir);
    let paths: Vec<&str> = planned.iter().map(|p| p.data_relative.as_str()).collect();
    assert_eq!(paths, vec!["knowhow/a.md"]);

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&outside);
}

#[cfg(unix)]
#[test]
fn validate_tree_rejects_a_top_level_symlink_named_as_a_content_dir() {
    use std::os::unix::fs::symlink;
    let outside = tmpdir("validate_symlink_outside");
    fs::write(outside.join("x.md"), "x").unwrap();

    let dir = tmpdir("validate_symlink");
    fs::write(dir.join("manifest.toml"), VALID_MANIFEST).unwrap();
    // A symlink named `knowhow` (a content dir) pointing at a real directory
    // outside the tree. `is_dir()` would follow and accept it; no-follow rejects.
    symlink(&outside, dir.join("knowhow")).unwrap();

    match validate_tree(&dir) {
        Err(ValidationError::UnexpectedTopLevelEntry(name)) => assert_eq!(name, "knowhow"),
        other => panic!("expected UnexpectedTopLevelEntry, got {other:?}"),
    }

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&outside);
}

// --- compare_versions ---

#[test]
fn compare_versions_update_when_remote_newer() {
    assert_eq!(compare_versions("0.1.0", "0.2.0"), UpdateDecision::Update);
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
    assert_eq!(compare_versions("garbage", "1.0.0"), UpdateDecision::Update);
}

// --- categories (controlled vocabulary) ---

#[test]
fn parses_categories_normalized_and_deduped() {
    let toml = format!(
        "{}categories = [\"Finance\", \"  health \", \"finance\", \"made-up\"]\n",
        VALID_MANIFEST
    );
    let m = parse_manifest(&toml).unwrap();
    // Lowercased + trimmed + de-duplicated; unknown values are NOT rejected at
    // parse (they're filtered + flagged at scan time).
    assert_eq!(m.categories, vec!["finance", "health", "made-up"]);
}

#[test]
fn parse_manifest_without_categories_is_empty() {
    let m = parse_manifest(VALID_MANIFEST).unwrap();
    assert!(m.categories.is_empty());
}

#[test]
fn partition_categories_splits_known_from_unknown() {
    let input = vec![
        "finance".to_string(),
        "made-up".to_string(),
        "health".to_string(),
        "finance".to_string(), // dup known
        "nonsense".to_string(),
    ];
    let (known, unknown) = partition_categories(&input);
    assert_eq!(known, vec!["finance", "health"]);
    assert_eq!(unknown, vec!["made-up", "nonsense"]);
}

#[test]
fn every_known_category_passes_is_valid() {
    for c in PLUGIN_CATEGORIES {
        assert!(is_valid_category(c), "{c} should be a valid category");
    }
    assert!(!is_valid_category("not-a-category"));
}
