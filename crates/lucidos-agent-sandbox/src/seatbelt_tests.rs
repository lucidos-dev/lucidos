use super::*;

fn policy() -> Policy {
    Policy {
        hidden: vec!["/h/.lucidos/private".into()],
        open_within_hidden: vec!["/h/gw/workspaces".into()],
        read_only: vec!["/h/ws/.lucidos".into()],
        writable_within_read_only: vec!["/h/ws/.lucidos/worktrees".into()],
    }
}

#[test]
fn each_exception_follows_the_denial_it_carves_into() {
    let text = profile(&policy());
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle}: {text}"))
    };
    let hidden = at("(deny file-read* file-write* (subpath \"/h/.lucidos/private\"))");
    let open = at("(allow file-read* file-write* (subpath \"/h/gw/workspaces\"))");
    let read_only = at("(deny file-write* (subpath \"/h/ws/.lucidos\"))");
    let writable = at("(allow file-write* (subpath \"/h/ws/.lucidos/worktrees\"))");
    assert!(text.starts_with("(version 1)\n(allow default)\n"));
    assert!(hidden < open && open < read_only && read_only < writable);
}

#[test]
fn a_hidden_root_refuses_unix_socket_connects() {
    let text = profile(&policy());
    assert!(text.contains(
        "(deny network-outbound (remote unix-socket (subpath \"/h/.lucidos/private\")))"
    ));
}

/// A quote or backslash in a home path must not end the string literal.
#[test]
fn a_path_cannot_break_out_of_its_literal() {
    let text = profile(&Policy {
        hidden: vec!["/h/a\"b\\c".into()],
        ..Policy::default()
    });
    assert!(text.contains("(subpath \"/h/a\\\"b\\\\c\")"), "{text}");
}

/// A rule binds a path, so a folder above a fenced root must not move.
#[test]
fn every_folder_above_a_fenced_root_is_fixed_in_place() {
    let text = profile(&policy());
    for ancestor in ["/h/.lucidos", "/h", "/h/ws"] {
        assert!(
            text.contains(&format!(
                "(deny file-write-unlink (literal \"{ancestor}\"))"
            )),
            "{ancestor}: {text}"
        );
    }
    assert!(!text.contains("(literal \"/\")"), "{text}");
    assert_eq!(text.matches("(literal \"/h\")").count(), 1, "{text}");
}

#[test]
fn reopened_ancestors_get_only_literal_metadata_once() {
    let text = profile(&Policy {
        hidden: vec![
            "/h/private".into(),
            "/h/private/gateway".into(),
            "/h/token".into(),
        ],
        open_within_hidden: vec!["/h/private/gateway/workspaces".into()],
        ..Policy::default()
    });
    let metadata: Vec<_> = text
        .lines()
        .filter(|line| line.contains("allow file-read-metadata"))
        .collect();
    assert_eq!(
        metadata,
        vec![
            "(allow file-read-metadata (literal \"/h/private/gateway\"))",
            "(allow file-read-metadata (literal \"/h/private\"))",
        ]
    );
    assert!(!text.contains("(allow file-read-data"));
    assert!(!text.contains("(allow file-read-xattr"));
}

#[cfg(unix)]
#[test]
fn resolved_spellings_each_keep_ancestor_metadata() {
    let scratch = tempfile::tempdir().unwrap();
    let real = scratch.path().join("real");
    let alias = scratch.path().join("alias");
    std::fs::create_dir_all(real.join("gateway/workspaces")).unwrap();
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let policy = Policy {
        hidden: vec![alias.join("gateway")],
        open_within_hidden: vec![alias.join("gateway/workspaces")],
        ..Policy::default()
    }
    .resolved();
    let text = profile(&policy);
    for root in &policy.hidden {
        assert_eq!(
            text.matches(&format!(
                "(allow file-read-metadata (literal {}))",
                quoted(root)
            ))
            .count(),
            1,
            "{text}"
        );
    }
}
