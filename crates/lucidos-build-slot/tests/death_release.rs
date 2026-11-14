//! Death release, tested against a real killed process.
//!
//! This file is its own test binary on purpose. The test forks, and a fork
//! copies every open descriptor, `flock`ed slot files included. In the unit
//! test binary, sibling tests hold such locks on other threads, and the child
//! kept them held after the parent released them.

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use lucidos_build_slot::BuildSlotPool;

/// Env var that turns [`slot_holder_child_helper`] into a real slot holder
/// instead of a no-op. Carries the pool directory.
const ENV_TEST_HOLD: &str = "LUCIDOS_BUILD_SLOT_TEST_HOLD";

/// Helper, not a real test: re-invoked as a child process by
/// [`child_process_holds_a_slot_until_killed`]. A plain run does nothing.
#[test]
fn slot_holder_child_helper() {
    let Ok(dir) = std::env::var(ENV_TEST_HOLD) else {
        return;
    };
    let p = BuildSlotPool::with_capacity(PathBuf::from(dir), 1).expect("open pool");
    let _guard = p.try_acquire("child holder").expect("child takes the slot");
    println!("HELD");
    // Long enough that the parent's kill is what ends this, never the sleep.
    std::thread::sleep(Duration::from_secs(120));
}

#[test]
fn child_process_holds_a_slot_until_killed() {
    // Death-release is the property the whole design rests on, so it is tested
    // against a real killed process rather than only against `Drop`.
    let dir = tempfile::tempdir().unwrap();
    let p = BuildSlotPool::with_capacity(dir.path().to_path_buf(), 1).expect("open pool");

    let mut child = Command::new(std::env::current_exe().expect("test binary path"))
        .args(["--exact", "slot_holder_child_helper", "--nocapture"])
        .env(ENV_TEST_HOLD, dir.path())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn holder");

    let mut line = String::new();
    let mut out = std::io::BufReader::new(child.stdout.take().expect("piped stdout"));
    while !line.contains("HELD") {
        line.clear();
        if out.read_line(&mut line).expect("read child stdout") == 0 {
            let _ = child.kill();
            panic!("holder exited before taking the slot");
        }
    }

    assert!(
        p.try_acquire("parent").is_none(),
        "the child holds the only slot"
    );

    // `Child::kill` is SIGKILL on Unix, so no destructor of the child's runs.
    child.kill().expect("kill holder");
    child.wait().expect("reap holder");

    let mut acquired = None;
    for _ in 0..50 {
        acquired = p.try_acquire("parent");
        if acquired.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        acquired.is_some(),
        "the kernel must release the slot of a SIGKILLed holder, with no reclaim step"
    );
}
