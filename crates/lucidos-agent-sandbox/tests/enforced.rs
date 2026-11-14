//! Real children under the real sandbox: the seatbelt on macOS, Landlock on
//! Linux. Linux runs it in a container, as a non-root user, from a static
//! binary that any image runs. Build it with
//! `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld cargo test
//! --target aarch64-unknown-linux-musl --no-run`.

#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::path::{Path, PathBuf};

use lucidos_agent_sandbox::{command, probe, Availability, Policy};

struct Tree {
    _scratch: tempfile::TempDir,
    top: PathBuf,
}

impl Tree {
    fn new() -> Self {
        let scratch = tempfile::tempdir().unwrap();
        let top = std::fs::canonicalize(scratch.path()).unwrap();
        for dir in ["private", "gw/workspaces/ws/.lucidos/worktrees", "open"] {
            std::fs::create_dir_all(top.join(dir)).unwrap();
        }
        std::fs::write(top.join("private/secret"), "s3cret").unwrap();
        std::fs::write(top.join("gw/registry.json"), "{}").unwrap();
        std::fs::write(top.join("gw/workspaces/ws/.lucidos/grants"), "none").unwrap();
        Tree {
            _scratch: scratch,
            top,
        }
    }

    fn policy(&self) -> Policy {
        let ws = self.top.join("gw/workspaces/ws");
        Policy {
            hidden: vec![self.top.join("private"), self.top.join("gw")],
            open_within_hidden: vec![self.top.join("gw/workspaces")],
            read_only: vec![ws.join(".lucidos")],
            writable_within_read_only: vec![ws.join(".lucidos/worktrees")],
        }
    }

    /// Whether `script` succeeds as a sandboxed `sh -c`.
    fn sandboxed(&self, script: &str) -> bool {
        let out = command("/bin/sh", &self.policy())
            .unwrap()
            .arg("-c")
            .arg(script)
            .current_dir(&self.top)
            .output()
            .unwrap();
        out.status.success()
    }

    fn at(&self, rel: &str) -> String {
        self.top.join(rel).display().to_string()
    }
}

fn available() -> bool {
    match probe() {
        Availability::Available => true,
        Availability::Unavailable(why) => {
            eprintln!("skipped: {why}");
            false
        }
    }
}

#[test]
fn a_hidden_file_cannot_be_read_written_or_replaced() {
    if !available() {
        return;
    }
    let t = Tree::new();
    let secret = t.at("private/secret");
    assert!(!t.sandboxed(&format!("cat '{secret}'")));
    assert!(!t.sandboxed(&format!("echo x > '{secret}'")));
    assert!(!t.sandboxed(&format!("rm '{secret}'")));
    assert!(!t.sandboxed(&format!("echo x > '{}'", t.at("private/new"))));
    assert!(!t.sandboxed(&format!("cat '{}'", t.at("gw/registry.json"))));
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "s3cret");
}

/// Renaming a folder above a fenced root would carry the root out from under
/// a rule that binds its path.
#[test]
fn a_folder_above_a_fenced_root_cannot_be_moved() {
    if !available() {
        return;
    }
    let t = Tree::new();
    let moved = format!("{}-moved", t.at(""));
    assert!(!t.sandboxed(&format!(
        "mv '{}' '{moved}' && cat '{moved}/private/secret'",
        t.at("")
    )));
    let ws = t.at("gw/workspaces/ws");
    assert!(!t.sandboxed(&format!(
        "mv '{ws}' '{ws}2' && echo x > '{ws}2/.lucidos/grants'"
    )));
    assert_eq!(
        std::fs::read_to_string(t.top.join("gw/workspaces/ws/.lucidos/grants")).unwrap(),
        "none"
    );
    assert!(t.sandboxed(&format!("echo x > '{}'", t.at("open/beside"))));
}

#[test]
fn a_workspace_inside_a_hidden_data_dir_stays_usable() {
    if !available() {
        return;
    }
    let t = Tree::new();
    let worktree = t.at("gw/workspaces/ws/.lucidos/worktrees/f");
    assert!(t.sandboxed(&format!("echo x > '{worktree}' && cat '{worktree}'")));
    assert!(t.sandboxed(&format!(
        "cat '{}'",
        t.at("gw/workspaces/ws/.lucidos/grants")
    )));
    assert!(!t.sandboxed(&format!(
        "echo all > '{}'",
        t.at("gw/workspaces/ws/.lucidos/grants")
    )));
    assert_eq!(
        std::fs::read_to_string(t.top.join("gw/workspaces/ws/.lucidos/grants")).unwrap(),
        "none"
    );
}

#[test]
fn everything_else_stays_open() {
    if !available() {
        return;
    }
    let t = Tree::new();
    let open = t.at("open");
    assert!(t.sandboxed(&format!(
        "mkdir '{open}/a' && echo x > '{open}/a/f' && mv '{open}/a/f' '{open}/g' && cat '{open}/g'"
    )));
    assert!(t.sandboxed("ls / >/dev/null && echo ok >/dev/null"));
}

#[test]
fn a_missing_program_still_fails_as_a_spawn() {
    if !available() {
        return;
    }
    let t = Tree::new();
    let status = command(Path::new("/nonexistent/lucidos-program"), &t.policy())
        .unwrap()
        .status();
    match status {
        Ok(status) => assert!(!status.success()),
        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
    }
}

fn nested_layout(private: bool) -> (Tree, Policy, PathBuf) {
    let t = Tree::new();
    let gateway = t.top.join(if private { "private/gateway" } else { "gw" });
    let ws = gateway.join("workspaces/ws");
    std::fs::create_dir_all(ws.join(".lucidos/runtime/python")).unwrap();
    std::fs::write(gateway.join("registry.json"), "{}").unwrap();
    std::fs::write(t.top.join("local-token"), "secret").unwrap();
    let policy = Policy {
        hidden: vec![
            t.top.join("private"),
            gateway.clone(),
            t.top.join("local-token"),
        ],
        open_within_hidden: vec![gateway.join("workspaces")],
        ..Policy::default()
    };
    (t, policy, ws)
}

#[test]
fn realpath_inside_reopened_workspaces() {
    if !available() {
        return;
    }
    for private in [false, true] {
        let (_t, policy, ws) = nested_layout(private);
        let out = command("realpath", &policy)
            .unwrap()
            .arg(ws.join(".lucidos/runtime/python"))
            .current_dir(&ws)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn reopened_ancestors_keep_hidden_contents_denied() {
    if !available() {
        return;
    }
    for private in [false, true] {
        let (t, policy, ws) = nested_layout(private);
        let gateway = ws.parent().unwrap().parent().unwrap();
        for (program, path) in [
            #[cfg(target_os = "macos")]
            ("ls", gateway.to_path_buf()),
            #[cfg(target_os = "macos")]
            ("ls", t.top.join("private")),
            ("cat", gateway.join("registry.json")),
            #[cfg(target_os = "macos")]
            ("stat", gateway.join("registry.json")),
            ("cat", t.top.join("private/secret")),
            #[cfg(target_os = "macos")]
            ("stat", t.top.join("private/secret")),
            #[cfg(target_os = "macos")]
            ("stat", t.top.join("local-token")),
            ("touch", gateway.join("new")),
        ] {
            let out = command(program, &policy)
                .unwrap()
                .arg(&path)
                .current_dir(&ws)
                .output()
                .unwrap();
            assert!(
                !out.status.success(),
                "{program} {} succeeded",
                path.display()
            );
        }
    }
}

#[test]
fn stock_venv_starts_inside_reopened_workspace() {
    if !available() {
        return;
    }
    let info = match std::process::Command::new("python3")
        .args([
            "-c",
            "import sysconfig; print(sysconfig.get_config_var('PYTHONFRAMEWORK'))",
        ])
        .output()
    {
        Ok(out) => {
            assert!(out.status.success());
            out
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipped: python3 unavailable");
            return;
        }
        Err(e) => panic!("python3: {e}"),
    };
    eprintln!(
        "python3 framework: {}",
        String::from_utf8_lossy(&info.stdout).trim()
    );
    for private in [false, true] {
        let (_t, policy, ws) = nested_layout(private);
        let venv = ws.join(".lucidos/runtime/python/venv");
        assert!(std::process::Command::new("python3")
            .args(["-m", "venv", "--without-pip"])
            .arg(&venv)
            .status()
            .unwrap()
            .success());
        let out = command(venv.join("bin/python"), &policy)
            .unwrap()
            .args(["-c", "import sys; print(sys.prefix)"])
            .current_dir(&ws)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            venv.to_str().unwrap()
        );
    }
}
