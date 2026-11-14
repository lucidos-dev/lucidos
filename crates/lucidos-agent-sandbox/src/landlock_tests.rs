use super::*;

/// A tree shaped like a home: a private subtree, a gateway data dir holding a
/// workspace, and a cache beside them.
fn home() -> (tempfile::TempDir, PathBuf, Policy) {
    let scratch = tempfile::tempdir().unwrap();
    let top = std::fs::canonicalize(scratch.path()).unwrap();
    for dir in [
        ".lucidos/private/run",
        ".lucidos/venvs",
        ".lucidos/gateway/pgdata",
        ".lucidos/gateway/workspaces/ws/data",
        ".lucidos/gateway/workspaces/ws/.lucidos/worktrees",
        "projects",
    ] {
        std::fs::create_dir_all(top.join(dir)).unwrap();
    }
    std::fs::write(top.join(".lucidos/local-token"), "t").unwrap();
    std::fs::write(
        top.join(".lucidos/gateway/workspaces/ws/.lucidos/ports"),
        "1",
    )
    .unwrap();
    let ws = top.join(".lucidos/gateway/workspaces/ws");
    let policy = Policy {
        hidden: vec![
            top.join(".lucidos/private"),
            top.join(".lucidos/local-token"),
            top.join(".lucidos/gateway"),
        ],
        open_within_hidden: vec![top.join(".lucidos/gateway/workspaces")],
        read_only: vec![ws.join(".lucidos")],
        writable_within_read_only: vec![ws.join(".lucidos/worktrees")],
    };
    (scratch, top, policy)
}

fn grant_of(grants: &[(PathBuf, Grant)], path: &Path) -> Option<Grant> {
    grants.iter().find(|(p, _)| p == path).map(|(_, g)| *g)
}

#[test]
fn hidden_roots_get_no_rule_and_their_siblings_get_everything() {
    let (_scratch, top, policy) = home();
    let grants = grants_from(&top, &policy).unwrap();
    let g = |rel: &str| grant_of(&grants, &top.join(rel));
    assert_eq!(g(".lucidos/private"), None);
    assert_eq!(g(".lucidos/private/run"), None);
    assert_eq!(g(".lucidos/local-token"), None);
    assert_eq!(g(".lucidos/gateway"), None);
    assert_eq!(g(".lucidos/gateway/pgdata"), None);
    assert_eq!(g(".lucidos/venvs"), Some(Grant::Full));
    assert_eq!(g("projects"), Some(Grant::Full));
}

/// A folder on the way down may only list, so `ls ~` works and nothing
/// beneath it inherits a write.
#[test]
fn a_folder_above_a_hidden_root_may_only_list() {
    let (_scratch, top, policy) = home();
    let grants = grants_from(&top, &policy).unwrap();
    assert_eq!(grant_of(&grants, &top), Some(Grant::ListOnly));
    assert_eq!(
        grant_of(&grants, &top.join(".lucidos")),
        Some(Grant::ListOnly)
    );
}

#[test]
fn a_workspace_inside_a_hidden_data_dir_stays_usable() {
    let (_scratch, top, policy) = home();
    let grants = grants_from(&top, &policy).unwrap();
    let ws = top.join(".lucidos/gateway/workspaces/ws");
    assert_eq!(grant_of(&grants, &ws.join("data")), Some(Grant::Full));
    assert_eq!(
        grant_of(&grants, &ws.join(".lucidos/ports")),
        Some(Grant::Read)
    );
    assert_eq!(
        grant_of(&grants, &ws.join(".lucidos/worktrees")),
        Some(Grant::Full)
    );
    assert_eq!(grant_of(&grants, &ws.join(".lucidos")), Some(Grant::Read));
}

#[test]
fn a_link_gets_no_rule() {
    let (_scratch, top, policy) = home();
    std::os::unix::fs::symlink(top.join(".lucidos/private"), top.join("sneaky")).unwrap();
    let grants = grants_from(&top, &policy).unwrap();
    assert_eq!(grant_of(&grants, &top.join("sneaky")), None);
}

#[test]
fn each_abi_handles_what_it_defines() {
    assert_eq!(handled_rights(1), ABI1_RIGHTS);
    assert_eq!(handled_rights(2) & REFER, REFER);
    assert_eq!(handled_rights(4) & IOCTL_DEV, 0);
    assert_eq!(
        handled_rights(8),
        ABI1_RIGHTS | REFER | TRUNCATE | IOCTL_DEV
    );
    assert_eq!(Grant::ListOnly.rights(handled_rights(8)), READ_DIR);
}
