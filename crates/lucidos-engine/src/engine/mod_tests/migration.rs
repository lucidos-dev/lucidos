fn unique_workspace(label: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "lucidos_authmod_migration_{}_{}",
        label,
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn migration_renames_legacy_auth_modules_when_new_absent() {
    let ws = unique_workspace("rename");
    let legacy = ws.join("data/artifacts/auth-modules");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("foo.wasm"), b"\0asm").unwrap();

    super::migrate_legacy_auth_modules_dir(&ws);

    let new_dir = ws.join("data/auth-modules");
    assert!(new_dir.is_dir(), "new dir should exist after migration");
    assert!(new_dir.join("foo.wasm").is_file());
    assert!(!legacy.exists(), "legacy dir should be gone after rename");

    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn migration_no_op_when_legacy_absent() {
    let ws = unique_workspace("none");
    super::migrate_legacy_auth_modules_dir(&ws);
    assert!(!ws.join("data/auth-modules").exists());
    assert!(!ws.join("data/artifacts/auth-modules").exists());
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn migration_does_not_clobber_when_both_exist() {
    let ws = unique_workspace("both");
    let legacy = ws.join("data/artifacts/auth-modules");
    let new_dir = ws.join("data/auth-modules");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("legacy.wasm"), b"legacy").unwrap();
    std::fs::create_dir_all(&new_dir).unwrap();
    std::fs::write(new_dir.join("new.wasm"), b"new").unwrap();

    super::migrate_legacy_auth_modules_dir(&ws);

    assert!(
        legacy.join("legacy.wasm").is_file(),
        "legacy must be untouched"
    );
    assert!(new_dir.join("new.wasm").is_file(), "new must be untouched");
    assert!(!new_dir.join("legacy.wasm").exists(), "must not merge");

    let _ = std::fs::remove_dir_all(&ws);
}
