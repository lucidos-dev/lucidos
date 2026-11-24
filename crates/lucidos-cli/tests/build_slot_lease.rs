#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
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
