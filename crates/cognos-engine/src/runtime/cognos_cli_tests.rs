use super::*;

#[test]
fn install_cognos_cli_skill_writes_file() {
    let tmp = tempfile::tempdir().unwrap();
    let cli = tempfile::tempdir().unwrap();
    install_cognos_cli_skill(tmp.path(), Some(cli.path())).unwrap();
    let skill = tmp.path().join(".claude/skills/cognos-cli/SKILL.md");
    let content = std::fs::read_to_string(&skill).unwrap();
    assert!(content.contains("cognos data write"));
    assert!(content.starts_with("---"));
}

#[test]
fn install_cognos_cli_skill_skips_when_no_cli_dir() {
    let tmp = tempfile::tempdir().unwrap();
    install_cognos_cli_skill(tmp.path(), None).unwrap();
    let skill = tmp.path().join(".claude/skills/cognos-cli/SKILL.md");
    assert!(
        !skill.exists(),
        "skill must not be installed when binary is missing — \
         otherwise CC sees a skill for a tool it can't run"
    );
}

#[test]
fn find_cognos_cli_dir_returns_none_when_no_binary() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(find_cognos_cli_dir(tmp.path()).is_none());
}

#[test]
fn find_cognos_cli_dir_finds_binary_at_start() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(COGNOS_BIN_NAME), b"").unwrap();
    let found = find_cognos_cli_dir(tmp.path()).expect("should find at start");
    assert_eq!(found, tmp.path());
}

#[test]
fn find_cognos_cli_dir_walks_up_from_deps_dir() {
    // Mirrors `cargo test` layout: test bin in target/debug/deps/, cognos in target/debug/.
    let target = tempfile::tempdir().unwrap();
    let deps = target.path().join("deps");
    std::fs::create_dir(&deps).unwrap();
    std::fs::write(target.path().join(COGNOS_BIN_NAME), b"").unwrap();
    let found = find_cognos_cli_dir(&deps).expect("should walk up to find binary");
    assert_eq!(found, target.path());
}

#[test]
fn workspace_bin_dir_is_under_dotcognos() {
    let ws = std::path::Path::new("/tmp/ws");
    assert_eq!(workspace_bin_dir(ws), ws.join(".cognos").join("bin"));
}

#[cfg(unix)]
#[test]
fn ensure_workspace_bin_symlink_creates_symlink_to_cli() {
    let ws = tempfile::tempdir().unwrap();
    let cli = tempfile::tempdir().unwrap();
    std::fs::write(cli.path().join(COGNOS_BIN_NAME), b"").unwrap();

    let bin_dir = ensure_workspace_bin_symlink(ws.path(), Some(cli.path()))
        .expect("should install symlink when cli_dir is given");

    assert_eq!(bin_dir, ws.path().join(".cognos").join("bin"));
    let link = bin_dir.join(COGNOS_BIN_NAME);
    let target = std::fs::read_link(&link).expect("should be a symlink");
    assert_eq!(target, cli.path().join(COGNOS_BIN_NAME));
}

#[test]
fn ensure_workspace_bin_symlink_returns_none_when_no_cli_dir() {
    let ws = tempfile::tempdir().unwrap();
    assert!(ensure_workspace_bin_symlink(ws.path(), None).is_none());
    assert!(
        !ws.path().join(".cognos/bin").exists(),
        "bin dir must not be created when there's no binary to point to"
    );
}

#[test]
fn ensure_workspace_bin_symlink_returns_none_when_cli_binary_missing() {
    // cli_dir given but no `cognos` binary inside it — engine binary
    // discovery returned a junk path, don't pretend it works.
    let ws = tempfile::tempdir().unwrap();
    let cli = tempfile::tempdir().unwrap();
    assert!(ensure_workspace_bin_symlink(ws.path(), Some(cli.path())).is_none());
}

#[cfg(unix)]
#[test]
fn ensure_workspace_bin_symlink_is_idempotent() {
    let ws = tempfile::tempdir().unwrap();
    let cli = tempfile::tempdir().unwrap();
    std::fs::write(cli.path().join(COGNOS_BIN_NAME), b"").unwrap();

    let first = ensure_workspace_bin_symlink(ws.path(), Some(cli.path())).unwrap();
    let second = ensure_workspace_bin_symlink(ws.path(), Some(cli.path())).unwrap();
    assert_eq!(first, second);

    let link = first.join(COGNOS_BIN_NAME);
    let target = std::fs::read_link(&link).unwrap();
    assert_eq!(target, cli.path().join(COGNOS_BIN_NAME));
}

#[cfg(unix)]
#[test]
fn ensure_workspace_bin_symlink_replaces_stale_target() {
    let ws = tempfile::tempdir().unwrap();
    let old_cli = tempfile::tempdir().unwrap();
    let new_cli = tempfile::tempdir().unwrap();
    std::fs::write(old_cli.path().join(COGNOS_BIN_NAME), b"").unwrap();
    std::fs::write(new_cli.path().join(COGNOS_BIN_NAME), b"").unwrap();

    ensure_workspace_bin_symlink(ws.path(), Some(old_cli.path())).unwrap();
    ensure_workspace_bin_symlink(ws.path(), Some(new_cli.path())).unwrap();

    let link = ws.path().join(".cognos/bin").join(COGNOS_BIN_NAME);
    let target = std::fs::read_link(&link).unwrap();
    assert_eq!(
        target,
        new_cli.path().join(COGNOS_BIN_NAME),
        "stale symlink (pointing at old engine location) must be replaced"
    );
}

#[test]
fn workspace_script_env_vars_always_sets_cognos_workspace() {
    let ws = tempfile::tempdir().unwrap();
    let vars = workspace_script_env_vars(ws.path(), None);
    let map: std::collections::HashMap<_, _> = vars.into_iter().collect();
    assert_eq!(
        map.get("COGNOS_WORKSPACE").map(String::as_str),
        Some(ws.path().display().to_string().as_str()),
        "COGNOS_WORKSPACE must be set even without the CLI — scripts may want \
         to know the workspace path for direct fs access"
    );
}

#[test]
fn workspace_script_env_vars_skips_path_when_cli_unavailable() {
    let ws = tempfile::tempdir().unwrap();
    let vars = workspace_script_env_vars(ws.path(), None);
    let map: std::collections::HashMap<_, _> = vars.into_iter().collect();
    assert!(
        !map.contains_key("PATH"),
        "must not inject PATH when there's no CLI to point at — \
         otherwise we'd shadow the inherited PATH for no benefit"
    );
}

#[cfg(unix)]
#[test]
fn workspace_script_env_vars_prepends_bin_dir_to_path() {
    let ws = tempfile::tempdir().unwrap();
    let cli = tempfile::tempdir().unwrap();
    std::fs::write(cli.path().join(COGNOS_BIN_NAME), b"").unwrap();

    let vars = workspace_script_env_vars(ws.path(), Some(cli.path()));
    let map: std::collections::HashMap<_, _> = vars.into_iter().collect();

    let path = map
        .get("PATH")
        .expect("PATH must be set when CLI available");
    let bin_dir = ws.path().join(".cognos/bin");
    let bin_str = bin_dir.display().to_string();
    let first = path.split(':').next().unwrap_or("");
    assert_eq!(
        first, bin_str,
        "workspace .cognos/bin must come FIRST in PATH so `cognos` resolves to our symlink"
    );
}
