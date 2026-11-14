use super::*;

fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, "x").unwrap();
}

/// A home with nothing in it, so a test names every entry it plants.
fn empty_home() -> (tempfile::TempDir, PathBuf) {
    let scratch = tempfile::tempdir().unwrap();
    let home = scratch.path().join(".lucidos");
    std::fs::create_dir(&home).unwrap();
    (scratch, home)
}

#[test]
fn a_fresh_home_puts_every_entry_in_the_subtree() {
    let (_scratch, home) = empty_home();
    for name in legacy_entries() {
        assert_eq!(entry_in(&home, &name), path_in(&home).join(&name), "{name}");
    }
}

#[test]
fn an_older_entry_stays_where_it_is() {
    let (_scratch, home) = empty_home();
    touch(&home.join(LOCAL_TOKEN));
    assert_eq!(entry_in(&home, LOCAL_TOKEN), home.join(LOCAL_TOKEN));
}

/// An older binary creates a missing entry in the home. A newer one creates
/// it there too, or the two would split one setting in two.
#[test]
fn an_older_home_gets_new_entries_beside_its_old_ones() {
    let (_scratch, home) = empty_home();
    touch(&home.join(LOCAL_TOKEN));
    for name in [NETWORK_TOML, RUN_DIR, crate::GATEWAY_DIR_NAME] {
        assert_eq!(entry_in(&home, name), home.join(name), "{name}");
    }
}

#[test]
fn an_entry_already_in_the_subtree_stays_there_in_an_older_home() {
    let (_scratch, home) = empty_home();
    touch(&home.join(LOCAL_TOKEN));
    touch(&path_in(&home).join(NETWORK_TOML));
    assert_eq!(
        entry_in(&home, NETWORK_TOML),
        path_in(&home).join(NETWORK_TOML)
    );
}

#[test]
fn the_old_copy_wins_when_both_exist() {
    let (_scratch, home) = empty_home();
    touch(&home.join(NETWORK_TOML));
    touch(&path_in(&home).join(NETWORK_TOML));
    assert_eq!(entry_in(&home, NETWORK_TOML), home.join(NETWORK_TOML));
}

/// A lock file alone marks a home an older release used.
#[test]
fn a_mint_lock_alone_marks_an_older_home() {
    let (_scratch, home) = empty_home();
    touch(&home.join(mint_lock_name(LOCAL_TOKEN)));
    assert!(holds_legacy_entries(&home));
    assert_eq!(entry_in(&home, LOCAL_TOKEN), home.join(LOCAL_TOKEN));
}

#[test]
fn a_dangling_link_counts_as_an_entry() {
    let (_scratch, home) = empty_home();
    std::os::unix::fs::symlink("nowhere", home.join(RUN_DIR)).unwrap();
    assert_eq!(entry_in(&home, RUN_DIR), home.join(RUN_DIR));
}

/// A name no older release used never lands in the home.
#[test]
fn a_new_name_always_resolves_to_the_subtree() {
    let (_scratch, home) = empty_home();
    touch(&home.join(LOCAL_TOKEN));
    assert_eq!(
        entry_in(&home, "socket-token"),
        path_in(&home).join("socket-token")
    );
}

/// Unrelated files in the home, such as caches, do not make it an older one.
#[test]
fn caches_do_not_mark_an_older_home() {
    let (_scratch, home) = empty_home();
    std::fs::create_dir(home.join("venvs")).unwrap();
    touch(&home.join("port-registry"));
    assert!(!holds_legacy_entries(&home));
}

#[test]
fn resolving_writes_nothing() {
    let (_scratch, home) = empty_home();
    touch(&home.join(LOCAL_TOKEN));
    for name in legacy_entries() {
        entry_in(&home, &name);
    }
    let listed: Vec<_> = std::fs::read_dir(&home)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(listed, vec![std::ffi::OsString::from(LOCAL_TOKEN)]);
}

#[test]
fn an_empty_variable_relocates_nothing() {
    assert_eq!(relocated_from(Some("".into())), None);
    assert_eq!(relocated_from(None), None);
    assert_eq!(
        relocated_from(Some("/tmp/p".into())),
        Some(PathBuf::from("/tmp/p"))
    );
}
