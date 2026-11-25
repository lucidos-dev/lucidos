#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use lucidos_build_slot::{
    BuildSlotPool, SlotClass, ENV_CLASS, ENV_HELD, ENV_MAX_BUILDS, ENV_POOL_DIR,
};

const CLI: &str = env!("CARGO_BIN_EXE_lucidos");

struct Lease {
    shell_or_broker: Child,
    pool: BuildSlotPool,
    _dir: tempfile::TempDir,
}

impl Lease {
    fn start(through_shell: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let pool = BuildSlotPool::with_capacity(dir.path().to_path_buf(), 1).unwrap();
        let mut cmd = if through_shell {
            let mut cmd = Command::new("sh");
            cmd.args([
                "-c",
                "exec 3<&0; \"$1\" build-slot --lease <&3 3<&- & wait",
                "owner",
                CLI,
            ]);
            cmd
        } else {
            let mut cmd = Command::new(CLI);
            cmd.args(["build-slot", "--lease"]);
            cmd
        };
        let child = cmd
            .current_dir(dir.path())
            .env(ENV_POOL_DIR, dir.path())
            .env(ENV_MAX_BUILDS, "1")
            .env(ENV_CLASS, "priority")
            .env("CARGO_BUILD_JOBS", "4")
            .env_remove("LUCIDOS_BUILD_SLOT_LEASE_OWNER")
            .env_remove("LUCIDOS_BUILD_SLOT_LEASE_RECORDS")
            .env_remove(ENV_HELD)
            .env_remove("LUCIDOS_BUILD_SLOT_NICE")
            .env_remove("LUCIDOS_WORKSPACE")
            .env_remove("LUCIDOS_API_BASE_URL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut lease = Self {
            shell_or_broker: child,
            pool,
            _dir: dir,
        };
        let mut grant = String::new();
        BufReader::new(lease.shell_or_broker.stdout.take().unwrap())
            .read_line(&mut grant)
            .unwrap();
        assert_eq!(grant, "granted 0 - 0\n");
        assert!(lease
            .pool
            .try_acquire_as("ordinary", SlotClass::Ordinary)
            .is_none());
        lease
    }

    fn assert_released(&self) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.pool.try_acquire("after lease").is_none() {
            assert!(Instant::now() < deadline, "lease did not release");
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn kill_owner(&mut self) {
        self.shell_or_broker.kill().unwrap();
        self.shell_or_broker.wait().unwrap();
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.shell_or_broker.kill();
        let _ = self.shell_or_broker.wait();
    }
}

#[test]
fn a_lease_reports_its_grant_then_holds_the_slot() {
    let mut lease = Lease::start(false);
    writeln!(lease.shell_or_broker.stdin.as_mut().unwrap()).unwrap();
    assert!(lease.shell_or_broker.wait().unwrap().success());
    lease.assert_released();
}

#[test]
fn an_ordinary_build_cannot_take_a_leased_slot_between_steps() {
    let mut lease = Lease::start(false);
    for _ in 0..2 {
        assert!(Command::new(CLI)
            .args([
                "build-slot",
                "--",
                "sh",
                "-c",
                r#"test -z "$LUCIDOS_BUILD_SLOT_LEASE_NICE""#
            ])
            .current_dir(lease._dir.path())
            .env_remove("LUCIDOS_API_BASE_URL")
            .env_remove("LUCIDOS_WORKSPACE")
            .env(ENV_HELD, "0")
            .env("LUCIDOS_BUILD_SLOT_LEASE_NICE", "10")
            .status()
            .unwrap()
            .success());
        assert!(lease.pool.try_acquire("ordinary").is_none());
    }
    drop(lease.shell_or_broker.stdin.take());
    assert!(lease.shell_or_broker.wait().unwrap().success());
    lease.assert_released();
}

#[test]
fn a_lease_frees_its_slot_when_its_owner_dies() {
    let mut lease = Lease::start(true);
    // Keep stdin open in the test so EOF cannot be the cause of release.
    let _input = lease.shell_or_broker.stdin.take().unwrap();
    lease.kill_owner();
    lease.assert_released();
}

#[test]
fn a_lease_frees_its_slot_when_it_is_killed() {
    let mut lease = Lease::start(false);
    lease.kill_owner();
    lease.assert_released();
}

#[test]
fn an_orphaned_wrapper_cannot_acquire_a_lease() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(CLI)
        .args(["build-slot", "--lease", "--max-wait", "0"])
        .env(ENV_POOL_DIR, dir.path())
        .env("LUCIDOS_BUILD_SLOT_LEASE_OWNER", "0")
        .env_remove(ENV_HELD)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn a_stalled_nested_announcement_preserves_the_completed_command_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let mut child = Command::new(CLI)
        .args(["build-slot", "--", "sh", "-c", "touch completed; exit 23"])
        .current_dir(dir.path())
        .env("LUCIDOS_API_BASE_URL", url)
        .env(ENV_HELD, "0")
        .env(
            "LUCIDOS_BUILD_SLOT_REQUEST_ID",
            "78d4a98d-1710-4fef-a661-f5fc8f2ac3f2",
        )
        .env_remove("LUCIDOS_BUILD_SLOT_GRANTED_CLASS")
        .spawn()
        .unwrap();

    let (mut peer, posted) = accept_post(&listener, &mut child);
    assert!(dir.path().join("completed").exists());
    let event = read_post(&mut peer);
    // Keep the socket open without a response until the CLI reaches its deadline.
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(23));
            break;
        }
        if posted.elapsed() >= Duration::from_secs(8) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the CLI did not exit after its five-second announcement timeout");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(event["event_type"], "BuildSlotStepCompleted");
    assert_eq!(event["payload"]["label"], "sh");
    assert_eq!(event["payload"]["exit_code"], 23);
    assert_eq!(
        event["payload"]["request_id"],
        "78d4a98d-1710-4fef-a661-f5fc8f2ac3f2"
    );
    assert!(event["payload"]["duration_ms"].is_u64());
    assert!(event["payload"]["class"].is_null());
}

#[test]
fn a_nested_step_that_cannot_launch_still_posts_its_step_record() {
    let dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut child = Command::new(CLI)
        .args(["build-slot", "--", "./no-such-program"])
        .current_dir(dir.path())
        .env(
            "LUCIDOS_API_BASE_URL",
            format!("http://{}", listener.local_addr().unwrap()),
        )
        .env(ENV_HELD, "0")
        .env("LUCIDOS_BUILD_SLOT_GRANTED_CLASS", "background")
        .env_remove("LUCIDOS_BUILD_SLOT_REQUEST_ID")
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let (mut peer, _) = accept_post(&listener, &mut child);
    let event = read_post(&mut peer);
    peer.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
        .unwrap();
    assert!(!child.wait().unwrap().success());
    assert_eq!(event["event_type"], "BuildSlotStepCompleted");
    let payload = &event["payload"];
    assert_eq!(payload["label"], "no-such-program");
    assert!(payload["exit_code"].is_null());
    assert_eq!(payload["outcome"], "unknown");
    assert_eq!(payload["class"], "background");
}

#[test]
fn a_release_carries_the_full_hold_and_the_wrapped_exit_status() {
    let dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut child = Command::new(CLI)
        .args(["build-slot", "--", "sh", "-c", "exit 23"])
        .current_dir(dir.path())
        .env(
            "LUCIDOS_API_BASE_URL",
            format!("http://{}", listener.local_addr().unwrap()),
        )
        .env(ENV_POOL_DIR, dir.path())
        .env(ENV_MAX_BUILDS, "1")
        .env_remove(ENV_HELD)
        .spawn()
        .unwrap();
    let (mut peer, _) = accept_post(&listener, &mut child);
    let event = read_post(&mut peer);
    let pool = BuildSlotPool::with_capacity(dir.path().to_path_buf(), 1).unwrap();
    assert!(pool.try_acquire("after release").is_some());
    peer.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
        .unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(23));
    assert_eq!(event["event_type"], "BuildSlotReleased");
    let payload = &event["payload"];
    assert_eq!(payload["slot"], "0");
    assert_eq!(payload["label"], "sh");
    assert_eq!(payload["outcome"], "failed");
    assert_eq!(payload["exit_code"], 23);
    assert!(uuid::Uuid::parse_str(payload["request_id"].as_str().unwrap()).is_ok());
    assert!(payload["wait_ms"].is_u64());
    assert!(payload["hold_ms"].is_u64());
}

#[test]
fn a_release_names_the_class_its_acquisition_resolved() {
    for (requested, resolved) in [
        (None, "ordinary"),
        (Some("priority"), "priority"),
        (Some("background"), "background"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut cmd = Command::new(CLI);
        cmd.args(["build-slot", "--", "sh", "-c", "exit 0"])
            .current_dir(dir.path())
            .env(
                "LUCIDOS_API_BASE_URL",
                format!("http://{}", listener.local_addr().unwrap()),
            )
            .env(ENV_POOL_DIR, dir.path())
            .env(ENV_MAX_BUILDS, "1")
            .env_remove(ENV_HELD)
            .env_remove(ENV_CLASS)
            .env_remove("LUCIDOS_BUILD_SLOT_PRIORITY");
        if let Some(requested) = requested {
            cmd.env(ENV_CLASS, requested);
        }
        let mut child = cmd.spawn().unwrap();
        let (mut peer, _) = accept_post(&listener, &mut child);
        let event = read_post(&mut peer);
        peer.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
            .unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(event["event_type"], "BuildSlotReleased");
        assert_eq!(event["payload"]["class"], resolved);
    }
}

#[test]
fn a_contended_background_build_names_its_class_on_all_three_events() {
    let dir = tempfile::tempdir().unwrap();
    let pool = BuildSlotPool::with_capacity(dir.path().to_path_buf(), 1).unwrap();
    let holder = pool.try_acquire("holder").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut child = Command::new(CLI)
        .args(["build-slot", "--", "sh", "-c", "exit 0"])
        .current_dir(dir.path())
        .env(
            "LUCIDOS_API_BASE_URL",
            format!("http://{}", listener.local_addr().unwrap()),
        )
        .env(ENV_POOL_DIR, dir.path())
        .env(ENV_MAX_BUILDS, "1")
        .env(ENV_CLASS, "background")
        .env_remove(ENV_HELD)
        .spawn()
        .unwrap();
    let mut holder = Some(holder);
    let events = [(); 3].map(|()| {
        let (mut peer, _) = accept_post(&listener, &mut child);
        let event = read_post(&mut peer);
        peer.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
            .unwrap();
        drop(holder.take());
        event
    });
    assert!(child.wait().unwrap().success());
    let types: Vec<_> = events
        .iter()
        .map(|event| event["event_type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        ["BuildSlotWaiting", "BuildSlotAcquired", "BuildSlotReleased"]
    );
    for event in &events {
        assert_eq!(event["payload"]["class"], "background");
    }
}

#[test]
fn a_shell_lease_links_its_nested_step_and_reports_the_body_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let scripts = dir.path().join("scripts");
    std::fs::create_dir(&scripts).unwrap();
    // Use the real broker without the host memory gate or global CLI lookup.
    let shim = scripts.join("with-build-slot.sh");
    std::fs::write(
        &shim,
        "#!/bin/sh\nexec \"$TEST_BUILD_SLOT_CLI\" build-slot \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut child = Command::new("bash")
        .args([
            "-ec",
            r#"source "$1"; build_slot_leased launch "$TEST_BUILD_SLOT_CLI" build-slot -- sh -c 'exit 23'"#,
            "test",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/lib/build_slot_lease.sh"),
        ])
        .current_dir(dir.path())
        .env("PROJECT_DIR", dir.path())
        .env("TEST_BUILD_SLOT_CLI", CLI)
        .env("LUCIDOS_API_BASE_URL", format!("http://{}", listener.local_addr().unwrap()))
        .env(ENV_POOL_DIR, dir.path())
        .env(ENV_MAX_BUILDS, "1")
        .env(ENV_CLASS, "background")
        .env_remove(ENV_HELD)
        .env_remove("LUCIDOS_BUILD_SLOT_LEASE_RECORDS")
        .env_remove("LUCIDOS_BUILD_SLOT_REQUEST_ID")
        .env_remove("LUCIDOS_BUILD_SLOT_GRANTED_CLASS")
        .spawn()
        .unwrap();

    let [step, release] = [(); 2].map(|()| {
        let (mut peer, _) = accept_post(&listener, &mut child);
        let event = read_post(&mut peer);
        peer.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
            .unwrap();
        event
    });
    assert_eq!(child.wait().unwrap().code(), Some(23));
    assert_eq!(step["event_type"], "BuildSlotStepCompleted");
    assert_eq!(release["event_type"], "BuildSlotReleased");
    let request_id = step["payload"]["request_id"].as_str().unwrap();
    assert!(uuid::Uuid::parse_str(request_id).is_ok());
    assert_eq!(release["payload"]["request_id"], request_id);
    for event in [step, release] {
        assert_eq!(event["payload"]["exit_code"], 23);
        assert_eq!(event["payload"]["outcome"], "failed");
        assert_eq!(event["payload"]["class"], "background");
    }
}

fn accept_post(listener: &TcpListener, child: &mut Child) -> (TcpStream, Instant) {
    listener.set_nonblocking(true).unwrap();
    let start = Instant::now();
    loop {
        match listener.accept() {
            Ok((peer, _)) => {
                peer.set_nonblocking(false).unwrap();
                peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                return (peer, Instant::now());
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "missing completion post"
                );
                assert!(start.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(err) => panic!("{err}"),
        }
    }
}

fn read_post(peer: &mut TcpStream) -> serde_json::Value {
    let mut reader = BufReader::new(peer);
    let mut content_length = None;
    loop {
        let mut line = String::new();
        assert_ne!(reader.read_line(&mut line).unwrap(), 0);
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.strip_prefix("content-length:") {
            content_length = Some(value.trim().parse::<usize>().unwrap());
        }
    }
    let mut body = vec![0; content_length.unwrap()];
    reader.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}
