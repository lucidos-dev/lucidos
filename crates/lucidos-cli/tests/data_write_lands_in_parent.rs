//! Demonstrates the bug-fix in the original spec:
//!
//!   "cd into a fake worktree dir, run `lucidos data write artifacts/test.txt
//!    --from /tmp/x`, confirm the file lands at <parent>/data/artifacts/test.txt"
//!
//! The CLI must walk up from the worktree-shaped subdirectory and resolve back
//! to the parent workspace's `data/` dir — never the worktree itself.

use std::fs;
use std::process::Command;

const LUCIDOS: &str = env!("CARGO_BIN_EXE_lucidos");

fn write_ports(workspace: &std::path::Path, port: u16) {
    let lucidos = workspace.join(".lucidos");
    fs::create_dir_all(&lucidos).unwrap();
    fs::write(
        lucidos.join("ports"),
        format!("API_PORT={}\nVITE_PORT={}\n", port, port),
    )
    .unwrap();
}

#[test]
fn write_from_worktree_lands_in_parent_workspace_data_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    write_ports(&workspace, 1);

    // Mirror the engine's worktree layout so the walk-up logic gets exercised
    // exactly the way it would at runtime.
    let worktree = workspace.join(".lucidos/worktrees/abc123");
    fs::create_dir_all(&worktree).unwrap();

    let src = tmp.path().join("input.txt");
    fs::write(&src, b"hello world").unwrap();

    let out = Command::new(LUCIDOS)
        .args(["data", "write", "artifacts/ua/test.txt", "--from"])
        .arg(&src)
        .current_dir(&worktree)
        .env_remove("LUCIDOS_WORKSPACE")
        .output()
        .expect("lucidos binary should run");
    assert!(
        out.status.success(),
        "lucidos data write failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let landed = workspace.join("data/artifacts/ua/test.txt");
    assert!(
        landed.exists(),
        "expected file at {}, but it does not exist",
        landed.display()
    );
    assert_eq!(fs::read(&landed).unwrap(), b"hello world");

    // And specifically — must NOT have landed in the worktree.
    let in_worktree = worktree.join("artifacts/ua/test.txt");
    assert!(
        !in_worktree.exists(),
        "file should NOT have been written into the worktree at {}",
        in_worktree.display()
    );
}

#[test]
fn write_falls_back_to_env_when_pwd_outside_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    write_ports(&workspace, 1);

    // PWD is unrelated to the workspace. Resolution must fall through to env.
    let unrelated = tmp.path().join("elsewhere");
    fs::create_dir_all(&unrelated).unwrap();

    let src = tmp.path().join("input.txt");
    fs::write(&src, b"via env").unwrap();

    let out = Command::new(LUCIDOS)
        .args(["data", "write", "artifacts/x.txt", "--from"])
        .arg(&src)
        .current_dir(&unrelated)
        .env("LUCIDOS_WORKSPACE", &workspace)
        .output()
        .expect("lucidos binary should run");
    assert!(
        out.status.success(),
        "lucidos data write failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(
        fs::read(workspace.join("data/artifacts/x.txt")).unwrap(),
        b"via env"
    );
}

#[test]
fn data_path_prints_resolved_path_with_normalization() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    write_ports(&workspace, 1);

    let worktree = workspace.join(".lucidos/worktrees/foo");
    fs::create_dir_all(&worktree).unwrap();

    // Loose name → prefixed with artifacts/
    let out = Command::new(LUCIDOS)
        .args(["data", "path", "report.html"])
        .current_dir(&worktree)
        .env_remove("LUCIDOS_WORKSPACE")
        .output()
        .expect("lucidos binary should run");
    assert!(out.status.success());
    let printed = String::from_utf8(out.stdout).unwrap();
    let printed = printed.trim();
    // macOS symlinks /var → /private/var, so the child's current_dir() resolves
    // through the symlink. Canonicalize the workspace root and assert the
    // printed path ends with the right suffix relative to it.
    let ws_canon = fs::canonicalize(&workspace).unwrap();
    let expected = ws_canon.join("data/artifacts/report.html");
    assert_eq!(std::path::PathBuf::from(printed), expected);
}
