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
            ..Policy::default()
        }
    }

    fn sandboxed(&self, script: &str) -> bool {
        self.sandboxed_under(&self.policy(), script)
    }

    fn sandboxed_under(&self, policy: &Policy, script: &str) -> bool {
        let out = command("/bin/sh", policy)
            .unwrap()
            .arg("-c")
            .arg(script)
            .current_dir(&self.top)
            .output()
            .unwrap();
        if !out.status.success() {
            eprintln!("{script}: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        out.status.success()
    }

    fn trigger_policy(&self) -> Policy {
        std::fs::create_dir_all(self.top.join("scope")).unwrap();
        Policy {
            write_scope: Some(vec![self.top.join("scope"), "/dev".into()]),
            ..self.policy()
        }
    }

    fn at(&self, rel: &str) -> String {
        self.top.join(rel).display().to_string()
    }
}

fn require_sandbox() {
    if let Availability::Unavailable(why) = probe() {
        panic!("the real sandbox is unavailable: {why}");
    }
}

#[test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
fn a_hidden_file_cannot_be_read_written_or_replaced() {
    require_sandbox();
    let t = Tree::new();
    let secret = t.at("private/secret");
    assert!(!t.sandboxed(&format!("cat '{secret}'")));
    assert!(!t.sandboxed(&format!("echo x > '{secret}'")));
    assert!(!t.sandboxed(&format!("rm '{secret}'")));
    assert!(!t.sandboxed(&format!("echo x > '{}'", t.at("private/new"))));
    assert!(!t.sandboxed(&format!("cat '{}'", t.at("gw/registry.json"))));
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "s3cret");
}

#[test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
fn a_folder_above_a_fenced_root_cannot_be_moved() {
    require_sandbox();
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
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
fn a_workspace_inside_a_hidden_data_dir_stays_usable() {
    require_sandbox();
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
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
fn everything_else_stays_open() {
    require_sandbox();
    let t = Tree::new();
    let open = t.at("open");
    assert!(t.sandboxed(&format!(
        "mkdir '{open}/a' && echo x > '{open}/a/f' && mv '{open}/a/f' '{open}/g' && cat '{open}/g'"
    )));
    assert!(t.sandboxed("ls / >/dev/null && echo ok >/dev/null"));
}

#[test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
fn a_missing_program_still_fails_as_a_spawn() {
    require_sandbox();
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
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
fn realpath_inside_reopened_workspaces() {
    require_sandbox();
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
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
fn reopened_ancestors_keep_hidden_contents_denied() {
    require_sandbox();
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
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
fn stock_venv_starts_inside_reopened_workspace() {
    require_sandbox();
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

#[test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
fn one_hidden_socket_connects_without_opening_its_folder() {
    use std::os::unix::net::UnixListener;

    require_sandbox();
    let t = Tree::new();
    let socket = t.top.join("private/allowed");
    let sibling = t.top.join("private/sibling");
    let _allowed = UnixListener::bind(&socket).unwrap();
    let _sibling = UnixListener::bind(&sibling).unwrap();
    let mut policy = t.policy();
    policy.connectable_sockets.push(socket.clone());
    let connect = |path: &Path| {
        command("python3", &policy)
            .unwrap()
            .args([
                "-c",
                "import socket,sys; s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])",
            ])
            .arg(path)
            .output()
            .unwrap()
    };
    let out = connect(&socket);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    #[cfg(target_os = "macos")]
    assert!(!connect(&sibling).status.success());
    for (program, path) in [
        ("cat", t.top.join("private/secret")),
        ("touch", t.top.join("private/new")),
        #[cfg(target_os = "macos")]
        ("ls", t.top.join("private")),
    ] {
        let out = command(program, &policy)
            .unwrap()
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "{program} {} succeeded",
            path.display()
        );
    }
}

#[test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
fn a_write_scope_refuses_outside_changes_including_metadata() {
    require_sandbox();
    let t = Tree::new();
    let policy = t.trigger_policy();
    let existing = t.at("open/existing");
    std::fs::write(&existing, "keep").unwrap();
    let metadata = std::fs::metadata(&existing).unwrap();
    let mut refused = vec![
        format!("echo x > '{}'", t.at("open/new")),
        format!("mv '{existing}' '{}'", t.at("open/renamed")),
        format!("mkdir '{}'", t.at("open/dir")),
        format!("rm '{existing}'"),
    ];
    if lucidos_agent_sandbox::guards_truncation() {
        refused.push(format!(": > '{existing}'"));
    }
    if lucidos_agent_sandbox::guards_metadata() {
        refused.extend([
            format!("chmod 600 '{existing}'"),
            format!("chown \"$(id -u)\" '{existing}'"),
            format!("touch -t 200001010000 '{existing}'"),
            set_xattr(&existing),
        ]);
    }
    for script in &refused {
        assert!(!t.sandboxed_under(&policy, script), "{script} succeeded");
    }
    assert!(!t.top.join("open/new").exists());
    assert_eq!(std::fs::read_to_string(&existing).unwrap(), "keep");
    let after = std::fs::metadata(&existing).unwrap();
    assert_eq!(
        (after.permissions(), after.modified().unwrap()),
        (metadata.permissions(), metadata.modified().unwrap())
    );

    let scope = t.at("scope");
    for script in [
        format!("mkdir '{scope}/d' && echo x > '{scope}/d/f' && mv '{scope}/d/f' '{scope}/g'"),
        format!("chmod 600 '{scope}/g' && touch '{scope}/g' && : > '{scope}/g'"),
        "printf '' > /dev/null && printf '' > /dev/stderr && printf '' > /dev/fd/2".to_string(),
        "if [ -d /dev/shm ]; then echo x > /dev/shm/lucidos-scope-$$ && rm /dev/shm/lucidos-scope-$$; fi"
            .to_string(),
        "mkdir rel && echo x > rel/f".to_string(),
    ] {
        let out = command("/bin/sh", &policy)
            .unwrap()
            .arg("-c")
            .arg(&script)
            .current_dir(t.top.join("scope"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{script}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
fn only_a_write_scope_enters_a_mount_namespace_and_the_report_matches() {
    require_sandbox();
    let t = Tree::new();
    let own = std::fs::read_link("/proc/self/ns/mnt").unwrap();
    let child_ns = |policy: &Policy| {
        let out = command("readlink", policy)
            .unwrap()
            .arg("/proc/self/ns/mnt")
            .output()
            .unwrap();
        assert!(out.status.success());
        PathBuf::from(String::from_utf8_lossy(&out.stdout).trim())
    };
    assert_eq!(child_ns(&t.policy()), own);
    assert_eq!(
        child_ns(&t.trigger_policy()) != own,
        lucidos_agent_sandbox::guards_metadata()
    );
    eprintln!(
        "metadata guarded: {}",
        lucidos_agent_sandbox::guards_metadata()
    );
}

fn set_xattr(path: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("xattr -w com.example.lucidos x '{path}'")
    } else {
        format!(
            "command -v python3 >/dev/null || exit 0; python3 -c 'import os,sys; os.setxattr(sys.argv[1], \"user.lucidos\", b\"x\")' '{path}'"
        )
    }
}
