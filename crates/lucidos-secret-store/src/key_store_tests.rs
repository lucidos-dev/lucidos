use super::*;

#[test]
fn the_mac_app_bundle_uses_the_keychain_and_everything_else_a_file() {
    assert_eq!(kind_for(None, true, true).unwrap(), KeyStoreKind::Keychain);
    assert_eq!(kind_for(None, false, true).unwrap(), KeyStoreKind::File);
    assert_eq!(kind_for(None, true, false).unwrap(), KeyStoreKind::File);
    assert_eq!(
        kind_for(Some(" "), true, true).unwrap(),
        KeyStoreKind::Keychain
    );
}

#[test]
fn the_override_wins_but_cannot_ask_for_a_keychain_that_is_not_there() {
    assert_eq!(
        kind_for(Some("file"), true, true).unwrap(),
        KeyStoreKind::File
    );
    assert_eq!(
        kind_for(Some("keychain"), false, true).unwrap(),
        KeyStoreKind::Keychain
    );
    assert!(kind_for(Some("keychain"), false, false).is_err());
    assert!(kind_for(Some("vault"), false, true).is_err());
}

#[test]
fn a_key_file_round_trips_owner_only_and_replaces_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileKeyStore::new(dir.path().join(KEY_DIR_NAME));
    assert_eq!(store.get("lucidos_ws").unwrap(), None);

    let first = SecretKey::generate();
    store.put("lucidos_ws", &first).unwrap();
    assert_eq!(store.get("lucidos_ws").unwrap(), Some(first));

    let second = SecretKey::generate();
    store.put("lucidos_ws", &second).unwrap();
    assert_eq!(store.get("lucidos_ws").unwrap(), Some(second));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&store.path("lucidos_ws")), 0o600);
        assert_eq!(mode(&dir.path().join(KEY_DIR_NAME)), 0o700);
    }
    #[cfg(target_os = "macos")]
    assert_eq!(
        lucidos_file_backup_exclusion::ensure_excluded(&dir.path().join(KEY_DIR_NAME)).unwrap(),
        lucidos_file_backup_exclusion::Exclusion::AlreadySet,
        "the key directory must stay out of Time Machine"
    );

    store.delete("lucidos_ws").unwrap();
    store.delete("lucidos_ws").unwrap();
    assert_eq!(store.get("lucidos_ws").unwrap(), None);
}

#[test]
fn a_damaged_key_file_is_an_error_not_a_missing_key() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileKeyStore::new(dir.path().to_path_buf());
    std::fs::write(store.path("lucidos_ws"), "not a key").unwrap();
    assert!(store.get("lucidos_ws").is_err());
}

#[test]
fn a_name_that_could_escape_the_directory_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileKeyStore::new(dir.path().to_path_buf());
    for bad in ["", "../x", "a/b", ".hidden", "a b"] {
        assert!(store.get(bad).is_err(), "{bad:?}");
    }
    assert_eq!(backup_key_name("lucidos_ws"), "lucidos_ws.backup");
    assert!(check_name(&backup_key_name("lucidos_ws")).is_ok());
}

#[test]
fn only_an_engine_inside_an_app_bundle_counts_as_one() {
    assert!(is_in_app_bundle(Path::new(
        "/Applications/Lucidos.app/Contents/MacOS/lucidos-engine"
    )));
    assert!(!is_in_app_bundle(Path::new(
        "/usr/local/bin/lucidos-engine"
    )));
    assert!(!is_in_app_bundle(Path::new(
        "/home/u/.app/bin/lucidos-engine"
    )));
}

#[test]
fn put_new_never_replaces_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileKeyStore::new(dir.path().to_path_buf());
    let (first, second) = (SecretKey::generate(), SecretKey::generate());

    assert!(store.put_new("lucidos_ws", &first).unwrap());
    assert!(!store.put_new("lucidos_ws", &second).unwrap());
    assert_eq!(store.get("lucidos_ws").unwrap(), Some(first));
}

#[test]
fn a_replaced_key_is_parked_under_its_id() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileKeyStore::new(dir.path().to_path_buf());
    let (old, new) = (SecretKey::generate(), SecretKey::generate());

    assert_eq!(replace_keeping(&store, "lucidos_ws", &old).unwrap(), None);
    assert_eq!(replace_keeping(&store, "lucidos_ws", &old).unwrap(), None);
    let parked = replace_keeping(&store, "lucidos_ws", &new)
        .unwrap()
        .expect("a different key must be parked");
    assert_eq!(parked, format!("lucidos_ws.{}", old.id()));
    assert_eq!(store.get(&parked).unwrap(), Some(old));
    assert_eq!(store.get("lucidos_ws").unwrap(), Some(new));
}
