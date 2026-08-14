use std::io::BufRead;
use std::process::{Command, Stdio};

use super::*;

const GB: u64 = 1024 * 1024 * 1024;

/// Env var that turns [`slot_holder_child_helper`] into a real slot holder
/// instead of a no-op. Carries the pool directory.
const ENV_TEST_HOLD: &str = "LUCIDOS_BUILD_SLOT_TEST_HOLD";

fn pool(dir: &tempfile::TempDir, capacity: usize) -> BuildSlotPool {
    BuildSlotPool::with_capacity(dir.path().to_path_buf(), capacity).expect("open pool")
}

// ── capacity resolution ─────────────────────────────────────────────────

#[test]
fn the_env_var_beats_the_file_and_the_file_beats_host_ram() {
    let from_env = resolve_capacity(Some("7"), Some("3"), Some(48 * GB));
    assert_eq!(from_env.value, 7);
    assert_eq!(from_env.source, CapacitySource::EnvVar);

    let from_file = resolve_capacity(None, Some("3"), Some(48 * GB));
    assert_eq!(from_file.value, 3);
    assert_eq!(from_file.source, CapacitySource::File);

    let from_ram = resolve_capacity(None, None, Some(48 * GB));
    assert_eq!(from_ram.value, 3);
    assert_eq!(from_ram.source, CapacitySource::HostRam);
}

#[test]
fn zero_and_junk_are_ignored_rather_than_honoured() {
    // A capacity of 0 would freeze every build on the host, including the
    // engine's own rebuild, with no way back. Each of these must fall through
    // to the next source instead of being taken at face value.
    for bad in ["0", "", "   ", "two", "-1", "3.5"] {
        let resolved = resolve_capacity(Some(bad), None, Some(48 * GB));
        assert_eq!(
            resolved.source,
            CapacitySource::HostRam,
            "{bad:?} must not be honoured as a capacity"
        );
        assert_eq!(resolved.value, 3);
    }
}

#[test]
fn a_capacity_file_with_surrounding_whitespace_still_parses() {
    // `set_capacity` writes a trailing newline, and a human editing the file
    // by hand leaves whatever their editor leaves.
    let resolved = resolve_capacity(None, Some(" 4\n"), Some(48 * GB));
    assert_eq!(resolved.value, 4);
    assert_eq!(resolved.source, CapacitySource::File);
}

#[test]
fn host_ram_derivation_has_a_floor_of_one_and_a_ceiling() {
    // A small laptop must still be able to build, so the floor is 1 rather
    // than the 0 that the raw division yields.
    assert_eq!(resolve_capacity(None, None, Some(4 * GB)).value, 1);
    assert_eq!(resolve_capacity(None, None, Some(8 * GB)).value, 1);
    assert_eq!(resolve_capacity(None, None, Some(16 * GB)).value, 1);
    assert_eq!(resolve_capacity(None, None, Some(32 * GB)).value, 2);
    assert_eq!(resolve_capacity(None, None, Some(48 * GB)).value, 3);
    assert_eq!(resolve_capacity(None, None, Some(64 * GB)).value, 4);
    // Past the ceiling, cores bind rather than memory.
    assert_eq!(resolve_capacity(None, None, Some(512 * GB)).value, 8);
}

#[test]
fn unreadable_host_ram_degrades_to_cautious_not_to_serial() {
    let resolved = resolve_capacity(None, None, None);
    assert_eq!(resolved.value, UNKNOWN_RAM_CAPACITY);
    assert_eq!(resolved.source, CapacitySource::Fallback);
    assert!(
        resolved.value > 1,
        "a guard that cannot measure must not serialise every build"
    );
}

#[test]
fn meminfo_total_parses_and_tolerates_a_missing_line() {
    let text = "MemFree:  123 kB\nMemTotal:       16316108 kB\nBuffers: 4 kB\n";
    assert_eq!(parse_meminfo_total(text), Some(16_316_108 * 1024));
    assert_eq!(parse_meminfo_total("MemFree: 1 kB\n"), None);
    assert_eq!(parse_meminfo_total("MemTotal:\n"), None);
}

#[test]
fn set_capacity_writes_a_number_the_pool_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let p = pool(&dir, 1);
    p.set_capacity(5).expect("write capacity");
    let contents = std::fs::read_to_string(dir.path().join(CAPACITY_FILE)).unwrap();
    assert_eq!(resolve_capacity(None, Some(&contents), None).value, 5);
}

#[test]
fn set_capacity_refuses_zero() {
    let dir = tempfile::tempdir().unwrap();
    assert!(pool(&dir, 1).set_capacity(0).is_err());
}

// ── the ceiling ─────────────────────────────────────────────────────────

#[test]
fn never_more_than_n_slots_are_held_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let p = pool(&dir, 2);

    let first = p.try_acquire("first").expect("first slot");
    let second = p.try_acquire("second").expect("second slot");
    assert_ne!(first.index(), second.index(), "two holders, two slots");
    assert!(
        p.try_acquire("third").is_none(),
        "a third build must not get a slot on a two-slot pool"
    );

    drop(first);
    let third = p.try_acquire("third").expect("a freed slot is reusable");
    assert!(p.try_acquire("fourth").is_none());
    drop(second);
    drop(third);
    assert!(p.try_acquire("later").is_some());
}

#[test]
fn a_bounded_wait_gives_up_instead_of_blocking_forever() {
    let dir = tempfile::tempdir().unwrap();
    let p = pool(&dir, 1);
    let _held = p.try_acquire("holder").expect("slot");
    let mut polls = 0;
    let got = p.acquire("waiter", Some(Duration::from_millis(600)), |_| polls += 1);
    assert!(got.is_none(), "the pool was full for the whole wait");
    assert!(polls > 0, "the caller must be told it is waiting");
}

#[test]
fn a_waiter_acquires_once_the_holder_releases() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let held = BuildSlotPool::with_capacity(path.clone(), 1)
        .unwrap()
        .try_acquire("holder")
        .expect("slot");

    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        drop(held);
    });

    let p = BuildSlotPool::with_capacity(path, 1).unwrap();
    let got = p.acquire("waiter", Some(Duration::from_secs(10)), |_| {});
    releaser.join().unwrap();
    assert!(got.is_some(), "the waiter must take the freed slot");
}

// ── nothing stale can wedge the pool ────────────────────────────────────

#[test]
fn holder_metadata_from_a_dead_process_does_not_reserve_the_slot() {
    // The kernel's flock is the only thing that says a slot is taken. Recorded
    // metadata is forensics, never state we consult to decide freeness, which
    // is exactly what the `mkdir` lock got wrong.
    let dir = tempfile::tempdir().unwrap();
    let p = pool(&dir, 1);
    std::fs::write(
        dir.path().join("slot-0.lock"),
        "PID=999999\nLABEL=a build that died\nSTARTED_EPOCH=1\n",
    )
    .unwrap();
    assert!(
        p.try_acquire("next").is_some(),
        "a slot whose recorded holder is long gone must be free"
    );
}

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
    let p = pool(&dir, 1);

    let mut child = Command::new(std::env::current_exe().expect("test binary path"))
        .args(["--exact", "tests::slot_holder_child_helper", "--nocapture"])
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

// ── waiting flag ────────────────────────────────────────────────────────

#[test]
fn nobody_waiting_on_an_idle_pool_and_somebody_waiting_while_a_flag_is_held() {
    let dir = tempfile::tempdir().unwrap();
    let p = pool(&dir, 1);
    assert!(!p.anyone_waiting(), "an idle pool has no waiters");

    let flag = p.mark_waiting().expect("mark waiting");
    assert!(p.anyone_waiting(), "a held flag means somebody is queued");

    drop(flag);
    assert!(!p.anyone_waiting(), "the flag is released with its holder");
}

#[test]
fn two_waiters_can_hold_the_flag_at_once() {
    // It is a shared lock, so a second waiter must not be excluded by the
    // first: both are queued and both need the announcement.
    let dir = tempfile::tempdir().unwrap();
    let p = pool(&dir, 1);
    let first = p.mark_waiting().expect("first waiter");
    let second = p.mark_waiting().expect("second waiter");
    assert!(p.anyone_waiting());
    drop(first);
    assert!(p.anyone_waiting(), "the second waiter is still queued");
    drop(second);
    assert!(!p.anyone_waiting());
}

// ── status ──────────────────────────────────────────────────────────────

#[test]
fn status_reports_holders_without_taking_a_slot() {
    let dir = tempfile::tempdir().unwrap();
    let p = pool(&dir, 2);
    let held = p.try_acquire("make lint").expect("slot");

    let states = p.status();
    assert_eq!(states.len(), 2, "one entry per slot");
    let holder = states[held.index()]
        .holder
        .as_ref()
        .expect("the held slot names its holder");
    assert_eq!(holder.label, "make lint");
    assert_eq!(holder.pid, Some(std::process::id()));
    assert!(holder.held_secs().is_some());
    assert_eq!(
        states.iter().filter(|s| s.holder.is_some()).count(),
        1,
        "only the held slot reports a holder"
    );

    // Probing must not have consumed the free slot.
    assert!(
        p.try_acquire("second").is_some(),
        "status must leave the remaining slot acquirable"
    );
}

#[test]
fn status_ignores_metadata_left_in_a_free_slot() {
    let dir = tempfile::tempdir().unwrap();
    let p = pool(&dir, 1);
    std::fs::write(
        dir.path().join("slot-0.lock"),
        "PID=999999\nLABEL=long gone\n",
    )
    .unwrap();
    assert!(
        p.status()[0].holder.is_none(),
        "an unlocked slot is free however its file reads"
    );
}

#[test]
fn holder_metadata_parses_and_tolerates_gaps() {
    let full = SlotHolder::parse("PID=42\nLABEL=make test\nCWD=/tmp/x\nSTARTED_EPOCH=1700\n");
    assert_eq!(full.pid, Some(42));
    assert_eq!(full.label, "make test");
    assert_eq!(full.cwd, "/tmp/x");
    assert_eq!(full.started_epoch, Some(1700));

    // An unknown key is ignored and a missing one leaves its field empty. A
    // slot written by an older build still reports what it did record.
    let partial = SlotHolder::parse("PID=7\nFUTURE_KEY=whatever\nnot a pair\n");
    assert_eq!(partial.pid, Some(7));
    assert_eq!(partial.label, "");
    assert_eq!(partial.started_epoch, None);
    assert_eq!(partial.held_secs(), None);
}

// ── re-entrancy and pool location ───────────────────────────────────────

#[test]
fn an_inherited_slot_is_read_from_the_environment() {
    // The value is whatever the wrapper exported; only presence decides that a
    // nested acquisition passes through.
    assert_eq!(ENV_HELD, "LUCIDOS_BUILD_SLOT_HELD");
    assert!(
        std::env::var(ENV_HELD).is_err() || inherited_slot().is_some(),
        "a set variable must be reported, an unset one must not"
    );
}

#[test]
fn the_pool_dir_override_wins_over_home() {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: process-wide env mutation, and this is the only test that
    // touches this variable.
    unsafe {
        std::env::set_var(ENV_POOL_DIR, dir.path());
    }
    let resolved = default_pool_dir().expect("resolve");
    unsafe {
        std::env::remove_var(ENV_POOL_DIR);
    }
    assert_eq!(resolved, dir.path());
}
