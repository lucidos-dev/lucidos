use super::*;
use lucidos_core::core::event_hooks::{HookListing, HOOKS_DIR, HOOK_DEFINITION_FILE};
use lucidos_core::core::DATA_DIR;
use lucidos_core::engine::event_bus::SystemEvent;
use lucidos_core::engine::thread_lifecycle::classify_event;
use std::path::{Path, PathBuf};

fn hooks_root(workspace: &Path) -> PathBuf {
    workspace.join(DATA_DIR).join(HOOKS_DIR)
}

fn write_hook(workspace: &Path, slug: &str, toml: &str, files: &[&str]) {
    let dir = hooks_root(workspace).join(slug);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(HOOK_DEFINITION_FILE), toml).unwrap();
    for file in files {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "exit 0\n").unwrap();
    }
}

fn hook_toml(event_type: &str, extra: &str) -> String {
    format!("name = \"Gate\"\nevent_type = \"{event_type}\"\nscript = \"check.py\"\n{extra}")
}

fn list(workspace: &Path) -> Vec<HookListing> {
    WorkspaceHooks.list_definitions(workspace).unwrap()
}

fn only_problems(workspace: &Path) -> Vec<String> {
    match list(workspace).as_slice() {
        [HookListing::Invalid { problems, .. }] => problems.clone(),
        other => panic!("expected one invalid hook, got {other:?}"),
    }
}

fn system_only_event() -> &'static str {
    SystemEvent::PERSISTED_TYPE_NAMES
        .iter()
        .find(|name| classify_event(name).is_none())
        .expect("a persisted system event that is not a thread event")
}

fn transient_event() -> &'static str {
    SystemEvent::RESERVED_TYPE_NAMES
        .iter()
        .find(|name| {
            !SystemEvent::is_persisted_type_name(name)
                && classify_event(name).is_none()
                && !["DomainEvent", "ThreadEvent"].contains(name)
        })
        .expect("a transient system frame")
}

#[test]
fn a_workspace_with_no_hooks_folder_lists_nothing() {
    let ws = tempfile::tempdir().unwrap();
    assert_eq!(list(ws.path()), vec![]);
}

#[test]
fn a_thread_event_hook_lists_as_valid() {
    let ws = tempfile::tempdir().unwrap();
    write_hook(
        ws.path(),
        "first-message",
        &hook_toml(
            "MessageReceived",
            "timeout_secs = 5\ncondition = { role = \"user\" }\n",
        ),
        &["check.py"],
    );
    match list(ws.path()).as_slice() {
        [HookListing::Valid {
            slug,
            event_kind,
            definition,
            content_hash,
        }] => {
            assert_eq!(content_hash.len(), 64, "{content_hash}");
            assert_eq!(slug, "first-message");
            assert_eq!(*event_kind, HookedEventKind::Thread);
            assert_eq!(definition.event_type, "MessageReceived");
            assert_eq!(definition.timeout_secs, Some(5));
            assert_eq!(
                definition.condition,
                Some(serde_json::json!({ "role": "user" }))
            );
        }
        other => panic!("expected one valid hook, got {other:?}"),
    }
}

#[test]
fn a_persisted_system_event_hook_lists_as_a_system_hook() {
    let ws = tempfile::tempdir().unwrap();
    write_hook(
        ws.path(),
        "sys",
        &hook_toml(system_only_event(), ""),
        &["check.py"],
    );
    assert!(matches!(
        list(ws.path()).as_slice(),
        [HookListing::Valid {
            event_kind: HookedEventKind::System,
            ..
        }]
    ));
}

#[test]
fn an_unknown_key_is_refused_by_name() {
    let ws = tempfile::tempdir().unwrap();
    write_hook(
        ws.path(),
        "typo",
        &hook_toml("MessageReceived", "timout_secs = 5\n"),
        &["check.py"],
    );
    match list(ws.path()).as_slice() {
        [HookListing::Invalid {
            definition: None,
            problems,
            ..
        }] => assert!(problems[0].contains("timout_secs"), "{problems:?}"),
        other => panic!("expected an unparsed invalid hook, got {other:?}"),
    }
}

#[test]
fn a_transient_event_is_not_hookable() {
    let ws = tempfile::tempdir().unwrap();
    let transient = transient_event();
    write_hook(ws.path(), "t", &hook_toml(transient, ""), &["check.py"]);
    let problems = only_problems(ws.path());
    assert!(
        problems
            .iter()
            .any(|p| p.contains(transient) && p.contains("transient")),
        "{problems:?}"
    );
}

#[test]
fn an_arming_event_is_not_hookable() {
    for name in ARMING_EVENT_TYPES {
        let ws = tempfile::tempdir().unwrap();
        write_hook(ws.path(), "gate", &hook_toml(name, ""), &["check.py"]);
        let problems = only_problems(ws.path());
        assert!(
            problems
                .iter()
                .any(|p| p.contains(name) && p.contains("no hook can stop")),
            "{problems:?}"
        );
    }
}

#[test]
fn a_valid_hook_lists_the_digest_of_its_content() {
    let ws = tempfile::tempdir().unwrap();
    write_hook(
        ws.path(),
        "gate",
        &hook_toml("MessageReceived", ""),
        &["check.py"],
    );
    let digest = |ws: &Path| list(ws)[0].content_hash().map(str::to_owned);
    let armed = digest(ws.path()).expect("a valid hook has a digest");
    std::fs::write(hooks_root(ws.path()).join("gate/check.py"), "exit 1\n").unwrap();
    assert_ne!(Some(armed), digest(ws.path()));
}

#[cfg(unix)]
#[test]
fn a_script_that_cannot_be_read_makes_the_hook_invalid() {
    use std::os::unix::fs::PermissionsExt;
    let ws = tempfile::tempdir().unwrap();
    write_hook(
        ws.path(),
        "gate",
        &hook_toml("MessageReceived", ""),
        &["check.py"],
    );
    let script = hooks_root(ws.path()).join("gate/check.py");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&script).is_ok() {
        return;
    }
    let problems = only_problems(ws.path());
    assert!(
        problems.iter().any(|p| p.contains("cannot be read")),
        "{problems:?}"
    );
}

#[test]
fn a_name_lucidos_never_records_is_refused_with_suggestions() {
    let ws = tempfile::tempdir().unwrap();
    write_hook(
        ws.path(),
        "m",
        &hook_toml("MessageRecieved", ""),
        &["check.py"],
    );
    let problems = only_problems(ws.path());
    assert!(
        problems
            .iter()
            .any(|p| p.contains("MessageRecieved") && p.contains("MessageReceived")),
        "{problems:?}"
    );
}

#[test]
fn an_event_type_with_spaces_at_either_end_is_refused() {
    let ws = tempfile::tempdir().unwrap();
    write_hook(
        ws.path(),
        "padded",
        &hook_toml("MessageReceived ", ""),
        &["check.py"],
    );
    let problems = only_problems(ws.path());
    assert!(
        problems.iter().any(|p| p.starts_with("event_type")),
        "{problems:?}"
    );
}

#[test]
fn a_condition_that_could_never_match_is_refused() {
    let ws = tempfile::tempdir().unwrap();
    write_hook(
        ws.path(),
        "c",
        &hook_toml("MessageReceived", "condition = { \"$and\" = [] }\n"),
        &["check.py"],
    );
    let problems = only_problems(ws.path());
    assert!(problems.iter().any(|p| p.contains("$and")), "{problems:?}");
}

#[test]
fn the_script_must_exist_inside_the_hook_folder() {
    let ws = tempfile::tempdir().unwrap();
    write_hook(ws.path(), "missing", &hook_toml("MessageReceived", ""), &[]);
    let problems = only_problems(ws.path());
    assert!(
        problems.iter().any(|p| p.contains("check.py")),
        "{problems:?}"
    );

    let ws = tempfile::tempdir().unwrap();
    let escaping = "name = \"Gate\"\nevent_type = \"MessageReceived\"\nscript = \"../check.py\"\n";
    write_hook(ws.path(), "escape", escaping, &[]);
    let problems = only_problems(ws.path());
    assert!(
        problems.iter().any(|p| p.contains("../check.py")),
        "{problems:?}"
    );
}

#[test]
fn a_script_in_a_subfolder_is_found() {
    let ws = tempfile::tempdir().unwrap();
    let nested = "name = \"Gate\"\nevent_type = \"MessageReceived\"\nscript = \"bin/check.sh\"\n";
    write_hook(ws.path(), "nested", nested, &["bin/check.sh"]);
    assert!(matches!(
        list(ws.path()).as_slice(),
        [HookListing::Valid { .. }]
    ));
}

#[cfg(unix)]
#[test]
fn a_script_that_links_out_of_the_hook_folder_is_refused() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("check.py");
    std::fs::write(&target, "exit 0\n").unwrap();
    write_hook(ws.path(), "linked", &hook_toml("MessageReceived", ""), &[]);
    std::os::unix::fs::symlink(&target, hooks_root(ws.path()).join("linked/check.py")).unwrap();
    let problems = only_problems(ws.path());
    assert!(
        problems.iter().any(|p| p.contains("check.py")),
        "{problems:?}"
    );
}

#[test]
fn the_script_takes_a_supported_extension() {
    let ws = tempfile::tempdir().unwrap();
    let toml = "name = \"Gate\"\nevent_type = \"MessageReceived\"\nscript = \"check.rb\"\n";
    write_hook(ws.path(), "ruby", toml, &["check.rb"]);
    let problems = only_problems(ws.path());
    assert!(problems.iter().any(|p| p.contains(".rb")), "{problems:?}");
}

#[test]
fn the_timeout_stays_between_one_second_and_the_ceiling() {
    for timeout in [0, TIMEOUT_CEILING_SECS + 1] {
        let ws = tempfile::tempdir().unwrap();
        write_hook(
            ws.path(),
            "slow",
            &hook_toml("MessageReceived", &format!("timeout_secs = {timeout}\n")),
            &["check.py"],
        );
        let problems = only_problems(ws.path());
        assert!(
            problems.iter().any(|p| p.contains("timeout_secs")),
            "{timeout}: {problems:?}"
        );
    }
}

#[test]
fn a_blank_name_is_refused() {
    let ws = tempfile::tempdir().unwrap();
    let toml = "name = \" \"\nevent_type = \"MessageReceived\"\nscript = \"check.py\"\n";
    write_hook(ws.path(), "blank", toml, &["check.py"]);
    let problems = only_problems(ws.path());
    assert!(problems.iter().any(|p| p.contains("name")), "{problems:?}");
}

#[test]
fn a_folder_name_that_is_not_a_slug_is_refused() {
    let ws = tempfile::tempdir().unwrap();
    write_hook(
        ws.path(),
        "My Gate",
        &hook_toml("MessageReceived", ""),
        &["check.py"],
    );
    let problems = only_problems(ws.path());
    assert!(
        problems.iter().any(|p| p.contains("My Gate")),
        "{problems:?}"
    );
}

#[test]
fn every_problem_is_listed_at_once() {
    let ws = tempfile::tempdir().unwrap();
    let toml = "name = \"Gate\"\nevent_type = \"MessageRecieved\"\nscript = \"check.py\"\ntimeout_secs = 0\n";
    write_hook(ws.path(), "many", toml, &[]);
    match list(ws.path()).as_slice() {
        [HookListing::Invalid {
            definition: Some(_),
            problems,
            ..
        }] => assert_eq!(problems.len(), 3, "{problems:?}"),
        other => panic!("expected a parsed invalid hook, got {other:?}"),
    }
}

#[test]
fn a_folder_with_no_definition_and_a_stray_file_both_list_as_invalid() {
    let ws = tempfile::tempdir().unwrap();
    let root = hooks_root(ws.path());
    std::fs::create_dir_all(root.join("empty")).unwrap();
    std::fs::write(root.join("loose.toml"), "").unwrap();
    std::fs::write(root.join(".DS_Store"), "").unwrap();
    let slugs: Vec<String> = list(ws.path())
        .into_iter()
        .map(|listing| match listing {
            HookListing::Invalid { slug, problems, .. } => {
                assert!(!problems.is_empty());
                slug
            }
            valid => panic!("expected invalid, got {valid:?}"),
        })
        .collect();
    assert_eq!(slugs, ["empty", "loose.toml"]);
}

#[cfg(unix)]
#[test]
fn a_link_to_nothing_reports_why_it_cannot_be_read() {
    let ws = tempfile::tempdir().unwrap();
    let root = hooks_root(ws.path());
    std::fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(root.join("gone"), root.join("dangling")).unwrap();
    let problems = only_problems(ws.path());
    assert!(
        problems
            .iter()
            .any(|p| p.contains("dangling") && p.contains("cannot be read")),
        "{problems:?}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_folder_name_that_is_not_utf8_says_so() {
    use std::os::unix::ffi::OsStrExt;
    let ws = tempfile::tempdir().unwrap();
    let dir = hooks_root(ws.path()).join(std::ffi::OsStr::from_bytes(b"caf\xe9"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(HOOK_DEFINITION_FILE),
        hook_toml("MessageReceived", ""),
    )
    .unwrap();
    std::fs::write(dir.join("check.py"), "exit 0\n").unwrap();
    let problems = only_problems(ws.path());
    assert!(problems.iter().any(|p| p.contains("UTF-8")), "{problems:?}");
}

#[test]
fn hooks_list_in_slug_order() {
    let ws = tempfile::tempdir().unwrap();
    for slug in ["zeta", "alpha", "mid"] {
        write_hook(
            ws.path(),
            slug,
            &hook_toml("MessageReceived", ""),
            &["check.py"],
        );
    }
    let slugs: Vec<String> = list(ws.path())
        .into_iter()
        .map(|listing| match listing {
            HookListing::Valid { slug, .. } | HookListing::Invalid { slug, .. } => slug,
        })
        .collect();
    assert_eq!(slugs, ["alpha", "mid", "zeta"]);
}

#[cfg(unix)]
#[test]
fn a_hooks_tree_that_links_out_of_data_is_an_error() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(DATA_DIR)).unwrap();
    std::os::unix::fs::symlink(outside.path(), hooks_root(ws.path())).unwrap();
    assert!(WorkspaceHooks.list_definitions(ws.path()).is_err());
}

#[test]
fn plugin_hooks_copy_a_shipped_script_into_the_hashed_folder() {
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    let hook = root.join("hooks/gate");
    std::fs::create_dir_all(&hook).unwrap();
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    let definition = hook_toml("MessageReceived", "").replace("check.py", "scripts/check.py");
    std::fs::write(hook.join("hook.toml"), &definition).unwrap();
    std::fs::write(root.join("scripts/check.py"), "print('allow')").unwrap();

    let mut planned = lucidos_core::core::plugins::plan_files(root);
    WorkspaceHooks
        .bundle_plugin_scripts(root, &mut planned)
        .unwrap();
    let listings = WorkspaceHooks.list_at(&root.join("hooks")).unwrap();
    let hash = listings[0].content_hash().unwrap();
    assert_eq!(
        std::fs::read_to_string(hook.join("scripts/check.py")).unwrap(),
        "print('allow')"
    );
    std::fs::write(hook.join("scripts/check.py"), "print('refuse')").unwrap();
    let changed = WorkspaceHooks.list_at(&root.join("hooks")).unwrap();
    assert_ne!(changed[0].content_hash().unwrap(), hash);
}
