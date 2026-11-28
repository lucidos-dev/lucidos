use super::*;

// ── Capturing the build's output ─────────────────────────────────────────

fn shell(script: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.args(["-c", script]);
    cmd
}

#[tokio::test]
async fn both_streams_are_captured_in_order_and_copied_to_the_log() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let log = dir.join("engine.log");
    let (status, output) = run_capturing_output(
        shell("echo first; echo second >&2; echo third; exit 3"),
        &log,
        Duration::from_secs(2),
        &AtomicU32::new(0),
    )
    .await
    .expect("the build must run");
    assert_eq!(status.code(), Some(3));
    assert_eq!(output.head_text(), "first\nsecond\nthird\n");
    assert_eq!(output.last_line().as_deref(), Some("third"));
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "first\nsecond\nthird\n"
    );
}

#[tokio::test]
async fn a_log_that_cannot_be_written_never_fails_the_build() {
    // The incident class. Handed the log as its stdout, the build died on
    // its first `echo` and said nothing. Through the pipe it cannot.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let log = dir.join("engine.log");
    std::fs::write(&log, "").unwrap();
    let mut perms = std::fs::metadata(&log).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o444);
    std::fs::set_permissions(&log, perms).unwrap();
    let (status, output) = run_capturing_output(
        shell("set -e; echo Building engine...; echo done"),
        &log,
        Duration::from_secs(2),
        &AtomicU32::new(0),
    )
    .await
    .expect("the build must run");
    assert!(status.success(), "{status}");
    assert_eq!(output.last_line().as_deref(), Some("done"));
}

#[tokio::test]
async fn a_process_left_holding_the_pipe_does_not_hang_the_build() {
    // A daemon the build started inherits the write end, so end-of-file
    // may never come. The grace bounds the wait.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let started = std::time::Instant::now();
    let (status, output) = run_capturing_output(
        shell("sleep 5 & echo done"),
        &dir.join("engine.log"),
        Duration::from_millis(200),
        &AtomicU32::new(0),
    )
    .await
    .expect("the build must run");
    assert!(status.success());
    assert_eq!(output.last_line().as_deref(), Some("done"));
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "waited {:?} for a pipe a daemon held open",
        started.elapsed()
    );
}

#[test]
fn the_output_keeps_a_bounded_head_and_tail() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let mut output = BuildOutput::new(&dir.join("engine.log"));
    output.push(b"error: the first line wins\n");
    let filler = vec![b'x'; 1024];
    for _ in 0..(BUILD_OUTPUT_HEAD_CAP / filler.len() + 8) {
        output.push(&filler);
    }
    output.push(b"\nthe last line\n");
    assert_eq!(output.head.len(), BUILD_OUTPUT_HEAD_CAP);
    assert!(output.head_text().starts_with("error: the first line wins"));
    assert!(output.tail.len() <= 2 * BUILD_OUTPUT_TAIL_CAP);
    assert_eq!(output.last_line().as_deref(), Some("the last line"));
}

/// Poll `cond` until it holds, up to ~2 s.
///
/// Releasing an `flock` is only *eventually* observable in a process that
/// spawns subprocesses. The lock belongs to the open file description, and
/// `fork` hands the child a reference to that same description. Until the
/// child reaches `exec`, where `O_CLOEXEC` drops it, the lock stays alive
/// even though the owner closed its own fd. This suite forks constantly, and
/// under load a child can be descheduled between fork and exec.
///
/// The retry is not a weakened assertion. Nothing about the release path is
/// instantaneous by contract, and the real consumer
/// (`engine_build_in_progress_elsewhere`) treats an un-acquirable lock as a
/// peer build to retry past. The *held* direction is still asserted
/// immediately; only the released direction is given time.
fn eventually(cond: impl Fn() -> bool) -> bool {
    for _ in 0..200 {
        if cond() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

/// A lock that frees DURING the wait is acquired, not reported as a peer
/// build. A build an explicit Rebuild restarted is still dropping its
/// guard when the replacement probes. A single instantaneous try turns that
/// millisecond into `SkippedLocked`, leaving no build running at all.
#[tokio::test]
async fn build_lock_wait_rides_out_a_holder_that_is_about_to_release() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let path = dir.join(".lucidos-engine-build.lock");

    let held = eventually_acquire(&path);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        drop(held);
    });
    assert!(
        acquire_engine_build_lock_waiting(&path, Duration::from_secs(5))
            .await
            .is_some(),
        "a lock released partway through the wait must be acquired, not read as a peer build"
    );
}

/// The other direction, so the wait cannot paper over a genuine peer build.
/// A lock held for the whole window still yields `None`, which becomes
/// `SkippedLocked` rather than a second concurrent cargo.
#[tokio::test]
async fn build_lock_wait_gives_up_on_a_holder_that_never_releases() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let path = dir.join(".lucidos-engine-build.lock");

    let _held = eventually_acquire(&path);
    assert!(
        acquire_engine_build_lock_waiting(&path, Duration::from_millis(150))
            .await
            .is_none(),
        "a lock held for the whole wait must still report the peer build"
    );
}

/// A restart must take the GRANDCHILD down with the script.
/// `kill_on_drop` reaps only the direct child, so without the group guard a
/// restarted build leaves its `cargo` compiling against the shared
/// `target/`. Modelled with a shell that backgrounds a `sleep` and waits.
#[tokio::test]
async fn dropping_the_build_group_guard_kills_the_grandchild() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let pid_file = dir.join("grandchild.pid");

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(format!(
            "sleep 60 & echo $! > {}; wait",
            pid_file.to_string_lossy()
        ))
        .kill_on_drop(true);
    lucidos_engine::runtime::spawn_env::isolate_in_process_group(&mut cmd);
    let mut child = cmd.spawn().expect("spawn the group leader");

    let grandchild = read_pid_when_written(&pid_file).expect("grandchild pid");
    assert!(pid_is_alive(grandchild), "the grandchild must start alive");

    // Exactly what a cancelled build future does: guard first (declared
    // last), then the child.
    let published = AtomicU32::new(0);
    drop(BuildProcessGroupGuard::arm(child.id(), &published));
    child.start_kill().ok();
    child.wait().await.ok();
    assert_eq!(
        published.load(Ordering::SeqCst),
        0,
        "a dead build names no group"
    );

    assert!(
        eventually(|| !pid_is_alive(grandchild)),
        "the group kill must reach the grandchild; kill_on_drop alone leaves it running"
    );
}

/// The queued probe reads the group while the build runs. It must never
/// read it once the leader is reaped and its pid can be recycled.
#[tokio::test]
async fn the_build_group_is_published_while_it_runs_and_cleared_once_reaped() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let log = dir.join("engine.log");
    let published = AtomicU32::new(0);
    let mut cmd = shell("sleep 0.5");
    lucidos_engine::runtime::spawn_env::isolate_in_process_group(&mut cmd);
    let (result, seen) = tokio::join!(
        run_capturing_output(cmd, &log, Duration::from_secs(2), &published),
        async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            published.load(Ordering::SeqCst)
        },
    );
    assert!(result.expect("the build must run").0.success());
    assert_ne!(seen, 0, "a running build publishes its group");
    assert_eq!(
        published.load(Ordering::SeqCst),
        0,
        "a reaped build does not"
    );
}

#[test]
fn the_engine_rebuild_waits_as_a_priority_build() {
    let cmd = engine_build_command(
        std::path::Path::new("/repo/scripts/web-dev.sh"),
        std::path::Path::new("/ws"),
        std::path::Path::new("/repo/target/.launch/release/plain"),
        true,
    );
    let priority = cmd
        .as_std()
        .get_envs()
        .find(|(k, _)| *k == BUILD_SLOT_CLASS_ENV)
        .and_then(|(_, v)| v);
    assert_eq!(priority, Some(std::ffi::OsStr::new("priority")));
    let args: Vec<_> = cmd.as_std().get_args().collect();
    assert_eq!(
        args,
        [
            "-w",
            "/ws",
            "--engine-build",
            "--publish-to",
            "/repo/target/.launch/release/plain",
            "--fresh"
        ]
    );
}

/// Poll for the pid the shell writes once it has backgrounded its child.
fn read_pid_when_written(path: &std::path::Path) -> Option<i32> {
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(pid) = text.trim().parse::<i32>() {
                return Some(pid);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

/// `ps -p` rather than `kill(pid, 0)`, to keep the test free of `unsafe`.
/// The grandchild is reparented to init when its shell dies, so init reaps
/// it and it leaves the table rather than lingering as a zombie.
fn pid_is_alive(pid: i32) -> bool {
    std::process::Command::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The load-bearing single-builder invariant: while one holder has the
/// build lock, no other acquire succeeds, and the lock releases on drop.
/// `flock` is per-open-file-description on Unix, so this same-process check
/// mirrors the cross-process case that serializes rebuilds.
#[test]
fn build_lock_admits_a_single_holder_and_releases_on_drop() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let path = dir.join(".lucidos-engine-build.lock");

    let held = eventually_acquire(&path);
    assert!(
        try_lock_file(&path).is_none(),
        "a second acquire must fail while the lock is held (single builder)"
    );
    drop(held);
    assert!(
        eventually(|| try_lock_file(&path).is_some()),
        "the lock must become acquirable again after release"
    );
}

/// Take the lock, tolerating a transient hold by a concurrently-forked
/// child that hasn't reached `exec` yet. See [`eventually`].
fn eventually_acquire(path: &std::path::Path) -> std::fs::File {
    for _ in 0..200 {
        if let Some(file) = try_lock_file(path) {
            return file;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("could not acquire {} within 2 s", path.display());
}

/// The `shared_build_in_progress` fail-OPEN contract. The held-detection
/// probe reports `true` ONLY while the lock is genuinely held. A free or
/// indeterminate probe can therefore never hide the manual "Rebuild" escape
/// hatch behind a phantom spinner.
#[test]
fn lock_held_at_reports_held_only_while_genuinely_locked() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let path = dir.join(".lucidos-engine-build.lock");

    // Free lock reads as not held, so the escape hatch stays available.
    assert!(
        eventually(|| !lock_held_at(&path)),
        "an unlocked path must read as NOT held"
    );
    // Held by another open file description, so genuinely busy. This
    // direction is immediate: nothing can make a held lock look free.
    let held = eventually_acquire(&path);
    assert!(
        lock_held_at(&path),
        "a held lock must read as held (flock WouldBlock)"
    );
    // Released, so free again. See `eventually`: a forked child can hold
    // the inherited description for a beat after we close ours.
    drop(held);
    assert!(
        eventually(|| !lock_held_at(&path)),
        "a released lock must read as NOT held again"
    );
}

mod guard_tests {
    use super::*;

    /// The lock must not live under `target/`. `flock` binds to an inode, so a
    /// `cargo clean` deleting the file mid-build releases nothing: the next
    /// builder creates a fresh inode and takes an uncontended lock, and two
    /// cargo builds then run against the shared `target/` at once. That is the
    /// collision the lock exists to prevent, and a clean build is exactly when
    /// it would happen.
    #[test]
    fn build_lock_lives_outside_cargos_target_dir() {
        let Some(path) = engine_build_lock_path() else {
            // No repo root (packaged runtime): there is no checkout to
            // coordinate on, which `run_engine_build` handles separately.
            return;
        };
        // Checked RELATIVE to the checkout. A repo living under an unrelated
        // directory named `target` is not a cargo target dir, and an
        // absolute-path scan would fail the test for it.
        let root = lucidos_engine::paths::repo_root().expect("lock path implies a repo root");
        let rel = path
            .strip_prefix(&root)
            .expect("the lock lives inside the checkout");
        assert!(
            !rel.components().any(|c| c.as_os_str() == "target"),
            "build lock must not sit under target/, cargo clean would orphan its inode: {}",
            rel.display()
        );
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some(".lucidos-engine-build.lock"),
            "lock filename changed: {}",
            path.display()
        );
    }
}
