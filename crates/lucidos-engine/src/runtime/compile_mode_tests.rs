use super::*;

const VERSION: &str = "300.6.1+3.6.3";

fn lockfile(version: &str) -> String {
    format!(
        "[[package]]\nname = \"openssl\"\nversion = \"0.10.81\"\n\n\
         [[package]]\nname = \"openssl-src\"\nversion = \"{version}\"\n\
         source = \"registry+https://github.com/rust-lang/crates.io-index\"\n"
    )
}

/// A worktree that reads as a Lucidos source tree, with a lockfile.
fn lucidos_tree(root: &Path) -> PathBuf {
    let tree = root.join("worktree");
    std::fs::create_dir_all(tree.join("crates/lucidos-engine")).unwrap();
    std::fs::write(tree.join("crates/lucidos-engine/Cargo.toml"), "[package]\n").unwrap();
    std::fs::write(tree.join("Cargo.lock"), lockfile(VERSION)).unwrap();
    tree
}

fn complete_prebuilt(cache: &Path) -> PathBuf {
    let dir = prebuilt_dir(cache, VERSION);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(PREBUILT_COMPLETE), VERSION).unwrap();
    dir
}

fn pin(tree: &Path, mode: &CompileMode) {
    std::fs::create_dir_all(tree.join("target")).unwrap();
    std::fs::write(tree.join("target").join(PIN_FILE), mode.to_pin()).unwrap();
}

fn prebuilt(dir: &Path) -> CompileMode {
    CompileMode {
        openssl: OpenSslSource::Prebuilt(dir.to_path_buf()),
    }
}

fn vendored_openssl_output(tree: &Path) {
    std::fs::create_dir_all(tree.join("target/debug/build/openssl-sys-0123abcd/out/openssl-build"))
        .unwrap();
}

#[test]
fn the_lockfile_names_the_openssl_src_version() {
    assert_eq!(
        lockfile_openssl_src_version(&lockfile(VERSION)).as_deref(),
        Some(VERSION)
    );
    assert_eq!(
        lockfile_openssl_src_version("[[package]]\nname = \"openssl\"\n"),
        None
    );
}

#[test]
fn a_pin_reads_back_as_the_mode_it_records() {
    let dir = PathBuf::from("/cache/lucidos/openssl/x");
    for mode in [CompileMode::vendored(), prebuilt(&dir)] {
        assert_eq!(CompileMode::from_pin(&mode.to_pin()), Some(mode));
    }
    assert_eq!(CompileMode::from_pin("openssl=sometimes\n"), None);
    assert_eq!(CompileMode::from_pin("openssl=prebuilt\n"), None, "no dir");
}

#[test]
fn the_prebuilt_key_keeps_the_whole_version_and_names_the_host() {
    let dir = prebuilt_dir(Path::new("/c"), VERSION);
    let name = dir.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with(VERSION), "{name}");
    assert!(name.ends_with(std::env::consts::OS), "{name}");
    assert_eq!(
        with_suffix(&dir, ".lock")
            .file_name()
            .unwrap()
            .to_string_lossy(),
        format!("{name}.lock")
    );
}

#[test]
fn a_fresh_worktree_takes_the_prebuilt_when_it_is_complete() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tree = lucidos_tree(tmp.path());
    let dir = complete_prebuilt(&tmp.path().join("cache"));
    let r = resolve(&tree, &tmp.path().join("cache"));
    assert_eq!(r.mode, prebuilt(&dir));
    assert_eq!(r.pin_to_write, Some(prebuilt(&dir).to_pin()));
    assert_eq!(r.seed, None);
}

#[test]
fn a_fresh_worktree_vendors_and_seeds_when_no_prebuilt_exists() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tree = lucidos_tree(tmp.path());
    let cache = tmp.path().join("cache");
    let r = resolve(&tree, &cache);
    assert_eq!(r.mode, CompileMode::vendored());
    assert_eq!(r.pin_to_write, Some(CompileMode::vendored().to_pin()));
    assert_eq!(
        r.seed,
        Some(Seed {
            version: VERSION.to_string(),
            dest: prebuilt_dir(&cache, VERSION),
        })
    );
}

/// A worktree built before pins existed already holds vendored artifacts.
/// Switching it to the prebuilt would rebuild its whole tree.
#[test]
fn an_unpinned_target_with_vendored_output_stays_vendored() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tree = lucidos_tree(tmp.path());
    complete_prebuilt(&tmp.path().join("cache"));
    vendored_openssl_output(&tree);
    let r = resolve(&tree, &tmp.path().join("cache"));
    assert_eq!(r.mode, CompileMode::vendored());
    assert_eq!(r.pin_to_write, Some(CompileMode::vendored().to_pin()));
}

/// After `cargo clean` and a prebuilt-mode rebuild, the target has no pin and
/// no vendored output. Reading it as legacy would flip the next build.
#[test]
fn an_unpinned_target_without_vendored_output_takes_the_prebuilt() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tree = lucidos_tree(tmp.path());
    let dir = complete_prebuilt(&tmp.path().join("cache"));
    std::fs::create_dir_all(tree.join("target/debug/build/libgit2-sys-1/out")).unwrap();
    assert_eq!(
        resolve(&tree, &tmp.path().join("cache")).mode,
        prebuilt(&dir)
    );
}

#[test]
fn a_vendored_pin_never_flips_when_a_prebuilt_appears() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tree = lucidos_tree(tmp.path());
    pin(&tree, &CompileMode::vendored());
    complete_prebuilt(&tmp.path().join("cache"));
    let r = resolve(&tree, &tmp.path().join("cache"));
    assert_eq!(r.mode, CompileMode::vendored());
    assert_eq!(r.pin_to_write, None);
}

#[test]
fn a_prebuilt_pin_holds_while_its_library_exists() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tree = lucidos_tree(tmp.path());
    let dir = complete_prebuilt(&tmp.path().join("cache"));
    pin(&tree, &prebuilt(&dir));
    let r = resolve(&tree, &tmp.path().join("cache"));
    assert_eq!(r.mode, prebuilt(&dir));
    assert_eq!(r.pin_to_write, None);
}

/// Pointing a build at a deleted library fails it, so a lost prebuilt is the
/// one case that flips, once, to vendoring.
#[test]
fn a_prebuilt_pin_whose_library_is_gone_falls_back_to_vendoring() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tree = lucidos_tree(tmp.path());
    pin(&tree, &prebuilt(&tmp.path().join("gone")));
    let r = resolve(&tree, &tmp.path().join("cache"));
    assert_eq!(r.mode, CompileMode::vendored());
    assert_eq!(r.pin_to_write, Some(CompileMode::vendored().to_pin()));
}

/// A pin to the old version's library would link an OpenSSL the lockfile no
/// longer names. The bump rebuilds every OpenSSL dependent anyway.
#[test]
fn a_prebuilt_pin_follows_the_lockfile_to_a_new_version() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tree = lucidos_tree(tmp.path());
    let cache = tmp.path().join("cache");
    let old = complete_prebuilt(&cache);
    pin(&tree, &prebuilt(&old));
    let next = "300.7.0+3.7.0";
    std::fs::write(tree.join("Cargo.lock"), lockfile(next)).unwrap();

    let r = resolve(&tree, &cache);
    assert_eq!(
        r.mode,
        CompileMode::vendored(),
        "no prebuilt for {next} yet"
    );
    assert_eq!(r.pin_to_write, Some(CompileMode::vendored().to_pin()));

    let new_dir = prebuilt_dir(&cache, next);
    std::fs::create_dir_all(&new_dir).unwrap();
    std::fs::write(new_dir.join(PREBUILT_COMPLETE), next).unwrap();
    pin(&tree, &prebuilt(&old));
    assert_eq!(resolve(&tree, &cache).mode, prebuilt(&new_dir));
}

/// Pins the wire contract with the seed script, which shell cannot import:
/// the marker it writes, the lock beside the destination, and its arguments.
#[test]
fn the_seed_script_writes_the_marker_and_lock_this_module_reads() {
    let script = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(SEED_SCRIPT),
    )
    .expect("the seed script");
    for contract in [
        format!("\"$DEST/{PREBUILT_COMPLETE}\""),
        format!("\"$STAGE/{PREBUILT_COMPLETE}\""),
        "LOCK=\"$DEST.lock\"".to_string(),
        "VERSION=\"${1:?$USAGE}\"".to_string(),
        "FEATURES=\"${2?$USAGE}\"".to_string(),
        "DEST=\"${3:?$USAGE}\"".to_string(),
    ] {
        assert!(
            script.contains(&contract),
            "the seed script lost {contract}"
        );
    }
}

#[test]
fn the_prebuilt_key_names_the_features_it_was_built_with() {
    let name = prebuilt_dir(Path::new("/c"), VERSION)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(name.contains(&format!("-{PREBUILT_FEATURES}-")), "{name}");
}

#[test]
fn a_seed_is_not_restarted_while_its_last_log_is_fresh() {
    let tmp = tempfile::TempDir::new().unwrap();
    let log = tmp.path().join("v.seed.log");
    assert!(!seed_ran_recently(&log), "no seed has run");
    std::fs::write(&log, "failed").unwrap();
    assert!(seed_ran_recently(&log));
}

#[test]
fn the_prebuilt_mode_links_it_statically_and_vendoring_sets_nothing() {
    let dir = PathBuf::from("/cache/openssl/v");
    assert_eq!(
        prebuilt(&dir).env(),
        vec![
            ("OPENSSL_NO_VENDOR", "1".to_string()),
            ("OPENSSL_DIR", "/cache/openssl/v".to_string()),
            ("OPENSSL_STATIC", "1".to_string()),
        ]
    );
    assert!(CompileMode::vendored().env().is_empty());
}

/// External-repo and app worktrees share the agent compile env. They must
/// get no OpenSSL variables and no pin in their tree.
#[test]
fn a_tree_that_is_not_lucidos_source_gets_no_mode_and_no_pin() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tree = tmp.path().join("other-repo");
    std::fs::create_dir_all(&tree).unwrap();
    std::fs::write(tree.join("Cargo.lock"), lockfile(VERSION)).unwrap();
    complete_prebuilt(&tmp.path().join("cache"));
    assert!(env_for_worktree(&tree, Some(&tmp.path().join("cache"))).is_empty());
    assert!(!tree.join("target").exists());
}

#[test]
fn a_lucidos_tree_with_a_prebuilt_gets_its_variables_and_a_pin() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tree = lucidos_tree(tmp.path());
    let dir = complete_prebuilt(&tmp.path().join("cache"));
    assert_eq!(
        env_for_worktree(&tree, Some(&tmp.path().join("cache"))),
        prebuilt(&dir).env()
    );
    assert_eq!(
        std::fs::read_to_string(tree.join("target").join(PIN_FILE)).unwrap(),
        prebuilt(&dir).to_pin()
    );
}

#[test]
fn with_no_cache_root_there_is_no_mode() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tree = lucidos_tree(tmp.path());
    assert!(env_for_worktree(&tree, None).is_empty());
}
