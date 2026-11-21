use super::disposable::Scope;

#[test]
#[ignore = "subprocess fixture for disposable environment tests"]
fn disposable_environment_fixture() {
    let root = std::env::var_os("TEST_SCOPE_ROOT").unwrap();
    let scope = Scope::create(std::path::Path::new(&root)).unwrap();
    let output = scope
        .command(std::path::Path::new("/usr/bin/env"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let env = String::from_utf8(output.stdout).unwrap();
    assert!(!env.contains("SYNTHETIC_CREDENTIAL="));
    assert_eq!(
        env.lines().any(|line| line == "LUCIDOS_AGENT_SANDBOXED=1"),
        std::env::var("LUCIDOS_AGENT_SANDBOXED").as_deref() == Ok("1")
    );
    assert!(!env.lines().any(|line| line == "LUCIDOS_AGENT_SANDBOXED=0"));
}

#[test]
fn disposable_commands_preserve_only_a_supplied_canonical_sandbox_marker() {
    for marker in [None, Some("1"), Some("0")] {
        let parent = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .env_clear()
            .args([
                "--ignored",
                "--exact",
                "disposable_tests::disposable_environment_fixture",
            ])
            .env("TEST_SCOPE_ROOT", parent.path().join("run"))
            .env("SYNTHETIC_CREDENTIAL", "fixture-secret");
        if let Some(marker) = marker {
            command.env("LUCIDOS_AGENT_SANDBOXED", marker);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{marker:?}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[test]
fn disposable_children_use_the_scope_private_file_key_store() {
    let parent = tempfile::tempdir().unwrap();
    let scope = Scope::create(&parent.path().join("run")).unwrap();
    let command = scope.command(std::path::Path::new("/usr/bin/env"));
    let env: std::collections::HashMap<_, _> = command.get_envs().collect();
    assert_eq!(
        env.get(std::ffi::OsStr::new("LUCIDOS_SECRET_KEY_STORE")),
        Some(&Some(std::ffi::OsStr::new("file")))
    );
    assert_eq!(
        env.get(std::ffi::OsStr::new("LUCIDOS_PRIVATE_DIR")),
        Some(&Some(scope.private_dir().as_os_str()))
    );
}

#[test]
fn disposable_scope_requires_a_new_root() {
    let parent = tempfile::tempdir().unwrap();
    assert!(Scope::create(parent.path()).is_err());
    let root = parent.path().join("run");
    let scope = Scope::create(&root).unwrap();
    assert_eq!(scope.root(), root.canonicalize().unwrap());
    assert!(Scope::create(&root).is_err());
    scope.claim_supervisor().unwrap();
    assert!(scope.claim_supervisor().is_err());
    scope.claim_gateway().unwrap();
    assert!(scope.claim_gateway().is_err(), "a scope cannot be adopted");
}

#[test]
fn disposable_scope_rejects_symlinked_storage_and_external_workspaces() {
    let parent = tempfile::tempdir().unwrap();
    let scope = Scope::create(&parent.path().join("run")).unwrap();
    let workspace = scope.app_data().join("workspaces/smoke");
    assert!(scope.validate_workspace(&workspace).is_ok());
    std::fs::create_dir_all(&workspace).unwrap();
    assert!(scope.validate_workspace(&workspace).is_ok());
    assert!(scope.validate_workspace(parent.path()).is_err());
    std::os::unix::fs::symlink(parent.path(), scope.app_data().join("workspaces/escape")).unwrap();
    assert!(scope
        .validate_workspace(&scope.app_data().join("workspaces/escape"))
        .is_err());
    std::fs::remove_dir(scope.private_dir()).unwrap();
    std::os::unix::fs::symlink(parent.path(), scope.private_dir()).unwrap();
    assert!(scope.claim_gateway().is_err());
}

#[test]
fn disposable_scope_never_reuses_an_existing_gateway() {
    let parent = tempfile::tempdir().unwrap();
    let scope = Scope::create(&parent.path().join("run")).unwrap();
    scope.claim_supervisor().unwrap();
    std::fs::create_dir_all(scope.app_data().join("config")).unwrap();
    std::fs::write(scope.app_data().join("config/workspaces.json"), "{}").unwrap();
    assert!(scope.claim_gateway().is_err());
}
