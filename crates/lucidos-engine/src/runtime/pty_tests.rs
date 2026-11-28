use super::*;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct Session {
    shell: Shell,
    output: mpsc::Receiver<Vec<u8>>,
    seen: Vec<u8>,
    input: std::fs::File,
}

impl Session {
    fn start(program: &str, cwd: &Path) -> Session {
        let env = vec![
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
            (OsString::from("HOME"), cwd.as_os_str().to_owned()),
            (OsString::from("TERM"), OsString::from("dumb")),
            (OsString::from("PS1"), OsString::from("$ ")),
            (OsString::from("ENV"), OsString::from("/dev/null")),
        ];
        let shell = spawn_shell(ShellSpec {
            program: PathBuf::from(program),
            cwd: cwd.to_owned(),
            env,
            size: Size::DEFAULT,
        })
        .expect("spawn shell");
        let mut reader = std::fs::File::from(shell.master.try_clone().unwrap());
        let input = std::fs::File::from(shell.master.try_clone().unwrap());
        let (tx, output) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Session {
            shell,
            output,
            seen: Vec::new(),
            input,
        }
    }

    fn send(&mut self, text: &str) {
        self.input.write_all(text.as_bytes()).unwrap();
    }

    fn wait_for(&mut self, needle: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if String::from_utf8_lossy(&self.seen).contains(needle) {
                return true;
            }
            if let Ok(chunk) = self.output.recv_timeout(Duration::from_millis(100)) {
                self.seen.extend(chunk);
            }
        }
        false
    }

    fn wait_for_job(&mut self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let text = String::from_utf8_lossy(&self.seen).into_owned();
            if text
                .split("job=")
                .last()
                .and_then(|rest| rest.split('.').next())
                .is_some_and(|pid| pid.parse::<u32>().is_ok())
            {
                return true;
            }
            if let Ok(chunk) = self.output.recv_timeout(Duration::from_millis(100)) {
                self.seen.extend(chunk);
            }
        }
        false
    }

    fn pid(&self) -> u32 {
        self.shell.child.id()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        kill_session(self.pid());
        let _ = self.shell.child.wait();
    }
}

#[test]
fn the_shell_runs_in_the_requested_folder_as_a_session_leader() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::start("/bin/sh", dir.path());
    session.send("pwd; echo sid=$(ps -o sess= -p $$ 2>/dev/null || echo none)\n");
    let canonical = dir.path().canonicalize().unwrap();
    assert!(session.wait_for(&canonical.file_name().unwrap().to_string_lossy()));
    // SAFETY: getsid reads the kernel record of our own child.
    let sid = unsafe { libc::getsid(session.pid() as libc::pid_t) };
    assert_eq!(sid as u32, session.pid(), "the shell leads its own session");
}

#[test]
fn the_shell_holds_the_foreground_until_a_program_takes_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::start("/bin/sh", dir.path());
    session.send("echo re\"\"ady\n");
    assert!(session.wait_for("ready"));
    assert!(shell_in_foreground(&session.shell.master, session.pid()));
    session.send("sleep 30\n");
    let deadline = Instant::now() + Duration::from_secs(5);
    while shell_in_foreground(&session.shell.master, session.pid()) {
        assert!(Instant::now() < deadline, "sleep never took the foreground");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn an_ignored_sighup_in_the_engine_does_not_reach_the_shell() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::start("/bin/sh", dir.path());
    // The engine may inherit SIG_IGN from whatever launched it. The child
    // must reset it, so a background job still dies on SIGHUP.
    // An interactive bash expands `!` from history before the line runs.
    session.send("set +H\n");
    session.send("trap - HUP; sleep 60 & echo jo\"\"b=$!.\n");
    assert!(
        session.wait_for_job(),
        "no job pid in {:?}",
        String::from_utf8_lossy(&session.seen)
    );
    let text = String::from_utf8_lossy(&session.seen).into_owned();
    let pid: u32 = text
        .split("job=")
        .last()
        .and_then(|rest| rest.split('.').next())
        .and_then(|value| value.parse().ok())
        .expect("job pid");
    // SAFETY: SIGHUP to our own grandchild.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGHUP) };
    let deadline = Instant::now() + Duration::from_secs(5);
    while is_alive(pid) && Instant::now() < deadline {
        session.send(":\n");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!is_alive(pid), "the background job survived SIGHUP");
}

#[test]
fn the_shell_runs_below_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let session = Session::start("/bin/sh", dir.path());
    // SAFETY: getpriority reads a process's priority.
    let engine = unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
    let shell = unsafe { libc::getpriority(libc::PRIO_PROCESS, session.pid() as libc::id_t) };
    assert_eq!(shell, (engine + 5).min(19));
}

#[test]
fn killing_the_session_leaves_no_member_behind() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::start("/bin/sh", dir.path());
    session.send("trap '' HUP; (trap '' HUP; sleep 60) & sleep 60 & echo sta\"\"rted\n");
    assert!(session.wait_for("started"));
    let sid = session.pid();
    assert!(
        session_members(sid).len() >= 3,
        "shell and two jobs share the session"
    );
    kill_session(sid);
    let _ = session.shell.child.wait();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !session_members(sid).is_empty() {
        assert!(Instant::now() < deadline, "a session member survived");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_member_forked_during_the_sweep_is_frozen_then_killed() {
    let mut listings = vec![vec![10], vec![10, 11], vec![10, 11]].into_iter();
    let mut sent: Vec<(u32, libc::c_int)> = Vec::new();
    freeze_then_kill(
        || listings.next().unwrap_or_default(),
        |pid, signal| sent.push((pid, signal)),
    );
    assert_eq!(
        sent,
        vec![
            (10, libc::SIGSTOP),
            (11, libc::SIGSTOP),
            (10, libc::SIGKILL),
            (11, libc::SIGKILL),
        ]
    );
}

#[test]
fn a_child_forked_by_a_resumed_member_after_the_kill_is_killed_too() {
    let mut listings = vec![vec![10, 11], vec![10, 11], vec![12], vec![12]].into_iter();
    let mut sent: Vec<(u32, libc::c_int)> = Vec::new();
    freeze_then_kill(
        || listings.next().unwrap_or_default(),
        |pid, signal| sent.push((pid, signal)),
    );
    assert_eq!(
        sent,
        vec![
            (10, libc::SIGSTOP),
            (11, libc::SIGSTOP),
            (10, libc::SIGKILL),
            (11, libc::SIGKILL),
            (12, libc::SIGSTOP),
            (12, libc::SIGKILL),
        ]
    );
}

#[test]
fn a_session_that_keeps_forking_still_ends_the_sweep() {
    let mut next = 0;
    let mut sent = 0;
    freeze_then_kill(
        || {
            next += 1;
            vec![next]
        },
        |_, _| sent += 1,
    );
    assert_eq!(next as usize, MAX_FREEZE_SWEEPS);
    assert_eq!(sent, 2 * MAX_FREEZE_SWEEPS);
}

#[test]
fn the_shell_inherits_only_the_given_environment() {
    assert!(
        std::env::var_os("CARGO_PKG_NAME").is_some(),
        "cargo sets it for tests"
    );
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::start("/bin/sh", dir.path());
    session.send("echo sec\"\"ret=[${CARGO_PKG_NAME}] term=[$TERM]\n");
    assert!(session.wait_for("term=[dumb]"));
    assert!(String::from_utf8_lossy(&session.seen).contains("secret=[]"));
}

#[test]
fn a_size_is_set_before_the_shell_starts_and_can_change() {
    let (master, slave) = open(Size {
        cols: 120,
        rows: 40,
    })
    .unwrap();
    let size = rustix::termios::tcgetwinsize(&slave).unwrap();
    assert_eq!((size.ws_col, size.ws_row), (120, 40));
    resize(&master, Size { cols: 90, rows: 30 }).unwrap();
    let size = rustix::termios::tcgetwinsize(&slave).unwrap();
    assert_eq!((size.ws_col, size.ws_row), (90, 30));
}

#[test]
fn the_login_shell_is_an_executable_absolute_path() {
    let shell = login_shell();
    assert!(shell.is_absolute());
    assert!(is_executable(&shell));
}

mod guard_tests {
    #[test]
    fn no_production_source_opens_a_pty_outside_this_module() {
        let markers = [
            "/dev/ptmx",
            "rustix::pty",
            "openpty",
            "posix_openpt",
            "forkpty",
        ];
        let offenders: Vec<String> = crate::test_support::source_scan::production_sources()
            .into_iter()
            .filter(|(rel, _)| rel != "runtime/pty.rs")
            .filter(|(_, text)| markers.iter().any(|marker| text.contains(marker)))
            .map(|(rel, _)| rel)
            .collect();
        assert!(
            offenders.is_empty(),
            "every PTY spawns through runtime/pty.rs: {offenders:?}"
        );
    }
}
