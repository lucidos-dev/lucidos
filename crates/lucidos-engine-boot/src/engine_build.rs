use lucidos_engine::engine::engine_version::{
    classify_build_failure, published_engine_exe, published_launch_dir, unrecognized_build_failure,
    BuildFailure, BuildFailureKind, EngineBuildOutcome, EngineBuildRunner, ENGINE_BUILD_LOG_PATH,
};
use lucidos_engine::runtime::spawn_env::BUILD_SLOT_CLASS_ENV;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

pub struct CheckoutEngineBuildRunner;

#[async_trait::async_trait]
impl EngineBuildRunner for CheckoutEngineBuildRunner {
    async fn run(
        &self,
        workspace: &std::path::Path,
        previous_failure: Option<&str>,
        published_group: &AtomicU32,
        fresh: bool,
    ) -> EngineBuildOutcome {
        run_engine_build(workspace, previous_failure, published_group, fresh).await
    }

    fn engine_build_in_progress_elsewhere(&self) -> bool {
        engine_build_in_progress_elsewhere()
    }

    fn shared_engine_build_lock_held(&self) -> bool {
        shared_engine_build_lock_held()
    }
}

/// Try to acquire the checkout-shared engine-build lock, an advisory `flock` in
/// the checkout's `.launch/` (see `engine_build_lock_path` and ADR 0063).
/// Returns the held guard, or `None` when a co-located engine holds it.
///
/// Co-located workspaces share one checkout and one `target/`, so only one
/// `web-dev.sh --engine-build` may run at a time. Concurrent cargo builds on
/// the same target OOM or corrupt it (CLAUDE.md). Keep the returned guard alive
/// for the whole build; dropping it releases the lock.
fn try_acquire_engine_build_lock() -> Option<std::fs::File> {
    try_lock_file(&engine_build_lock_path()?)
}

/// How long [`run_engine_build`] waits for the checkout-shared build lock before
/// concluding a co-located engine owns it.
///
/// A single instantaneous try is not a verdict, it is a sample. Two things make
/// a FREE lock read as held for a few milliseconds. A superseded build may
/// still be dropping its guard. A subprocess can inherit the open file
/// description until it reaches `exec`. Both resolve in milliseconds.
/// A genuine peer build runs for a minute or more.
const BUILD_LOCK_WAIT: Duration = Duration::from_secs(3);

/// Poll interval while waiting for [`BUILD_LOCK_WAIT`]. `flock` has no async
/// notification. A transient hold resolves in milliseconds.
const BUILD_LOCK_POLL: Duration = Duration::from_millis(50);

/// [`try_lock_file`] with a bounded retry, for the one caller that must not
/// mistake a millisecond of contention for a peer build. Returns the held guard,
/// or `None` when `wait` elapsed with the lock still held. Path- and
/// duration-parameterized so the timing is unit-testable without a checkout.
async fn acquire_engine_build_lock_waiting(
    lock_path: &std::path::Path,
    wait: Duration,
) -> Option<std::fs::File> {
    let deadline = Instant::now() + wait;
    loop {
        if let Some(guard) = try_lock_file(lock_path) {
            return Some(guard);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(BUILD_LOCK_POLL).await;
    }
}

/// SIGKILLs the build's process group if the build future is DROPPED, i.e. when
/// an explicit Rebuild restarts it.
///
/// `kill_on_drop(true)` reaches only the direct child, which is `web-dev.sh`.
/// The `cargo` it spawned is a grandchild and survives. Without this guard, a
/// restarted build leaves its `cargo` compiling against the shared `target/`,
/// slowing the build the user is waiting on.
///
/// Disarmed the moment the child is reaped. After `wait` the pid, and with it
/// the group id, can be recycled, and signalling a recycled group would hit
/// unrelated processes (see `spawn_env::signal_child_process_group`).
///
/// It also PUBLISHES the group while armed, for the queued probe. Clearing it
/// at the same moment keeps the probe from reading a recycled group too.
///
/// SIGKILL is untrappable, so a kill landing inside the launch-binary publish
/// leaves its `*.tmp.<pid>` behind. That is disk only, never a corrupt binary:
/// the publish copies and signs a temp and reaches the launch path solely
/// through `mv -f`. `prune_dead_launch_temps` (`scripts/lib/workspace.sh`)
/// collects the residue on the next publish.
struct BuildProcessGroupGuard<'a> {
    pid: Option<u32>,
    published: &'a AtomicU32,
}

impl<'a> BuildProcessGroupGuard<'a> {
    fn arm(pid: Option<u32>, published: &'a AtomicU32) -> Self {
        published.store(pid.unwrap_or(0), Ordering::SeqCst);
        BuildProcessGroupGuard { pid, published }
    }

    fn disarm(&mut self) {
        self.pid = None;
        self.published.store(0, Ordering::SeqCst);
    }
}

impl Drop for BuildProcessGroupGuard<'_> {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            self.published.store(0, Ordering::SeqCst);
            lucidos_engine::runtime::spawn_env::kill_child_process_group_now(pid);
        }
    }
}

/// Path of the checkout-shared engine-build lock, or `None` when `repo_root` is
/// unresolvable. Split out so `run_engine_build` can tell "no checkout to
/// coordinate on", which proceeds uncoordinated, from "a peer holds the lock",
/// which skips. The two must not be conflated.
///
/// Sits beside the published launch binaries in `.launch/`, deliberately NOT
/// under `target/`. `flock` binds to an INODE, not to a path, so a `cargo clean`
/// that deletes the lock file mid-build releases nothing: the next builder
/// creates a fresh file, gets a fresh inode, and takes an uncontended lock. Both
/// then run cargo against the shared `target/` at once, which is the collision
/// this lock exists to prevent. `.launch/` is outside every cargo subcommand's
/// reach, so the inode is stable for the life of the checkout.
fn engine_build_lock_path() -> Option<std::path::PathBuf> {
    Some(
        lucidos_engine::paths::repo_root()
            .ok()?
            .join(".launch")
            .join(".lucidos-engine-build.lock"),
    )
}

/// Open (creating parents) and non-blocking advisory-`flock` `lock_path`,
/// returning the held guard or `None` when another open file description holds
/// it. `flock` is per-open-file-description on Unix, so a second open of the
/// same path conflicts, in this process and across processes alike.
fn try_lock_file(lock_path: &std::path::Path) -> Option<std::fs::File> {
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        // The file is a pure `flock` handle: nothing is ever written into it,
        // and a peer may already hold this same path open.
        .truncate(false)
        .open(lock_path)
        .ok()?;
    fs2::FileExt::try_lock_exclusive(&file).ok()?;
    Some(file)
}

/// Cheap "is a co-located engine building right now?" probe: try the shared
/// build lock and immediately release it. Racy by nature, since the real guard
/// is the lock held across [`run_engine_build`]. Treats an un-acquirable lock as
/// busy, so the self-heal driver skips this tick and retries.
fn engine_build_in_progress_elsewhere() -> bool {
    match try_acquire_engine_build_lock() {
        Some(file) => {
            let _ = fs2::FileExt::unlock(&file);
            false
        }
        None => true,
    }
}

/// Whether the checkout-shared engine-build lock is **definitely** held, so a
/// build IS running, this engine's own or a co-located peer's. Drives the
/// `shared_build_in_progress` version-status field.
///
/// Deliberately the INVERSE fail-mode of [`engine_build_in_progress_elsewhere`],
/// which treats an unrunnable probe as busy so the self-heal driver errs toward
/// NOT stampeding a peer. This one **fails open to `false`**. The wire field
/// suppresses the manual "Rebuild" escape hatch, so an indeterminate probe
/// reading as busy would hide that hatch while nothing advances the binary.
/// Returns `true` ONLY when the non-blocking `flock` fails with `WouldBlock`.
fn shared_engine_build_lock_held() -> bool {
    match engine_build_lock_path() {
        Some(path) => lock_held_at(&path),
        None => false, // no checkout to coordinate on, so no shared build
    }
}

/// Path-parameterized core of [`shared_engine_build_lock_held`], split out so
/// the held, free and indeterminate cases are unit-testable without a checkout.
/// Returns `true` ONLY when a non-blocking `flock` fails with `WouldBlock`. A
/// free lock, an unopenable file, or any other lock error returns `false`.
fn lock_held_at(path: &std::path::Path) -> bool {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        // Probe-only open: never truncate the lock file a peer may be holding.
        .truncate(false)
        .open(path)
    else {
        return false; // unopenable probe file is indeterminate, so fail open
    };
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            // Acquired, so held by no one. Release immediately.
            let _ = fs2::FileExt::unlock(&file);
            false
        }
        // WouldBlock means another open file description holds it.
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => true,
        // Indeterminate, so fail open and keep Rebuild available.
        Err(_) => false,
    }
}

/// Run `web-dev.sh --engine-build -w <ws>` to rebuild the engine binary on disk,
/// copying its output into the workspace build log. The build runs in its own
/// process group so a restart kills `cargo` too, not just the script
/// (see [`BuildProcessGroupGuard`]). Holds the checkout-shared build lock for
/// the whole build, and returns `SkippedLocked` when a peer already holds it.
async fn run_engine_build(
    workspace: &std::path::Path,
    previous_failure: Option<&str>,
    published_group: &AtomicU32,
    fresh: bool,
) -> EngineBuildOutcome {
    // Elect a single builder across co-located engines. With no resolvable
    // checkout there is no shared `target/` to coordinate on, so proceed
    // uncoordinated rather than never building. Only a genuinely held lock
    // means a peer is building.
    let _build_lock = match engine_build_lock_path() {
        Some(path) => match acquire_engine_build_lock_waiting(&path, BUILD_LOCK_WAIT).await {
            Some(guard) => Some(guard),
            None => {
                lucidos_engine::log_legacy!(
                    "[Rebuild] the checkout-shared engine build lock stayed held for {}s, \
                     skipping this build (a co-located workspace is building; its build \
                     advances the shared binary for us too)",
                    BUILD_LOCK_WAIT.as_secs()
                );
                return EngineBuildOutcome::SkippedLocked;
            }
        },
        None => None,
    };
    let script = match lucidos_engine::paths::script("web-dev.sh") {
        Ok(s) => s,
        Err(e) => {
            lucidos_engine::log_legacy!("[Rebuild] cannot locate web-dev.sh: {e}");
            return EngineBuildOutcome::Failed(BuildFailure::plain(format!(
                "cannot locate web-dev.sh: {e}"
            )));
        }
    };
    let publish_to = match published_engine_exe().as_deref().and_then(published_launch_dir) {
        Some(path) => path,
        _ => return EngineBuildOutcome::Failed(BuildFailure::delivery(
            BuildFailureKind::UnsupportedLaunch,
            "Launch the engine from the checkout's published .launch/<profile>/<variant>/ path to enable background builds.",
        )),
    };
    let cmd = engine_build_command(&script, workspace, &publish_to, fresh);
    let log_path = workspace.join(ENGINE_BUILD_LOG_PATH);
    match run_capturing_output(cmd, &log_path, BUILD_OUTPUT_DRAIN_GRACE, published_group).await {
        Ok((status, _)) if status.success() => EngineBuildOutcome::Succeeded,
        Ok((status, output)) => {
            lucidos_engine::log_legacy!("[Rebuild] engine build exited {status}");
            let reason = classify_build_failure(&output.head_text(), previous_failure)
                .unwrap_or_else(|| {
                    unrecognized_build_failure(&status.to_string(), output.last_line().as_deref())
                });
            lucidos_engine::log_legacy!("[Rebuild] cause: {}", reason.summary);
            EngineBuildOutcome::Failed(reason)
        }
        Err(e) => {
            lucidos_engine::log_legacy!("[Rebuild] {e}");
            EngineBuildOutcome::Failed(BuildFailure::plain(e))
        }
    }
}

/// The `web-dev.sh --engine-build` command, before its output is wired.
///
/// It waits for a build slot as a priority waiter, because the user is
/// watching this build (ADR 0304). Nothing else in the tree asks for that.
fn engine_build_command(
    script: &std::path::Path,
    workspace: &std::path::Path,
    publish_to: &std::path::Path,
    fresh: bool,
) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(script);
    cmd.arg("-w")
        .arg(workspace)
        .arg("--engine-build")
        .arg("--publish-to")
        .arg(publish_to)
        .env(BUILD_SLOT_CLASS_ENV, "priority")
        .kill_on_drop(true);
    if fresh {
        cmd.arg("--fresh");
    }
    // Own process group, so a restart can reach the `cargo` grandchild
    // and not just this script. See `BuildProcessGroupGuard`.
    lucidos_engine::runtime::spawn_env::isolate_in_process_group(&mut cmd);
    cmd
}

/// How long to wait for a build's output to end after the build exits. A
/// process the build left running can hold the pipe open indefinitely, so the
/// build's result does not wait on end-of-file.
const BUILD_OUTPUT_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Run `cmd` with stdout and stderr on ONE pipe that this function drains. It
/// copies every chunk into `log_path` and keeps the text for
/// [`classify_build_failure`].
///
/// A pipe rather than the log file itself, because the file failed for real.
/// Handed the log as its stdout, the build exited 1 in under a second and wrote
/// zero bytes: its first `echo` failed under `set -e`, and so did the error
/// message. The pipe stays writable while this function reads it. So a log that
/// cannot be written costs the copy, never the build, and says so with its
/// real error.
///
/// One pipe for both streams keeps them in the order they were printed.
async fn run_capturing_output(
    mut cmd: tokio::process::Command,
    log_path: &std::path::Path,
    drain_grace: Duration,
    published_group: &AtomicU32,
) -> Result<(std::process::ExitStatus, BuildOutput), String> {
    use tokio::io::AsyncReadExt;
    let capture_failed = |e: std::io::Error| format!("could not capture the build output: {e}");
    let (sender, mut receiver) = tokio::net::unix::pipe::pipe().map_err(capture_failed)?;
    let stdout = sender.into_blocking_fd().map_err(capture_failed)?;
    let stderr = stdout.try_clone().map_err(capture_failed)?;
    cmd.stdout(stdout).stderr(stderr);
    // `spawn` + `wait` rather than `status`, so the pid is in hand for the
    // group-kill guard before the wait can be cancelled.
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start the build: {e}"))?;
    // Our copies of the write end must go, or end-of-file never arrives.
    drop(cmd);
    let mut group_guard = BuildProcessGroupGuard::arm(child.id(), published_group);
    let mut output = BuildOutput::new(log_path);
    let mut buf = vec![0u8; 8192];
    let mut open = true;
    let status = loop {
        tokio::select! {
            status = child.wait() => break status,
            read = receiver.read(&mut buf), if open => match read {
                Ok(0) => open = false,
                Err(e) => {
                    lucidos_engine::log_legacy!("[Rebuild] stopped reading the build output: {e}");
                    open = false;
                }
                Ok(n) => output.push(&buf[..n]),
            },
        }
    };
    // Reaped: the pid may now be recycled, so the group must not be signalled.
    group_guard.disarm();
    let status = status.map_err(|e| format!("the build could not be waited on: {e}"))?;
    if open {
        let drained = tokio::time::timeout(drain_grace, async {
            while let Ok(n @ 1..) = receiver.read(&mut buf).await {
                output.push(&buf[..n]);
            }
        })
        .await;
        if drained.is_err() {
            lucidos_engine::log_legacy!(
                "[Rebuild] a process the build started still holds its output open; \
                 its output keeps going to the log in the background"
            );
            // Keep the read end alive. Closing it would turn that process's
            // next write into a SIGPIPE, which the log file never did.
            let mut log = output.log.take();
            tokio::spawn(async move {
                use std::io::Write;
                let mut buf = vec![0u8; 8192];
                while let Ok(n @ 1..) = receiver.read(&mut buf).await {
                    if let Some(Err(e)) = log.as_mut().map(|f| f.write_all(&buf[..n])) {
                        lucidos_engine::log_legacy!(
                            "[Rebuild] stopped copying late build output: {e}"
                        );
                        log = None;
                    }
                }
            });
        }
    }
    Ok((status, output))
}

/// How much of a build's output to keep for [`classify_build_failure`]. Cargo
/// stops at the error, so it sits near the start. The cap therefore costs
/// nothing and bounds a pathological build.
const BUILD_OUTPUT_HEAD_CAP: usize = 512 * 1024;

/// How much of the END of a build's output to keep, for the last line an
/// unrecognized failure reports.
const BUILD_OUTPUT_TAIL_CAP: usize = 4 * 1024;

/// What a build printed, as it arrives: the head the classifier reads, a short
/// tail for the last line, and the copy into the build log.
struct BuildOutput {
    head: Vec<u8>,
    tail: Vec<u8>,
    log: Option<std::fs::File>,
    log_path: std::path::PathBuf,
}

impl BuildOutput {
    /// Opens `log_path` for appending. A log that cannot be opened is logged
    /// and skipped: the copy is a convenience, the build is not.
    fn new(log_path: &std::path::Path) -> Self {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .inspect_err(|e| {
                lucidos_engine::log_legacy!(
                    "[Rebuild] cannot open {} to copy the build output: {e}",
                    log_path.display()
                );
            })
            .ok();
        BuildOutput {
            head: Vec::new(),
            tail: Vec::new(),
            log,
            log_path: log_path.to_path_buf(),
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        use std::io::Write;
        if let Some(log) = &mut self.log {
            if let Err(e) = log.write_all(chunk) {
                lucidos_engine::log_legacy!(
                    "[Rebuild] stopped copying the build output to {}: {e}",
                    self.log_path.display()
                );
                self.log = None;
            }
        }
        let room = BUILD_OUTPUT_HEAD_CAP.saturating_sub(self.head.len());
        self.head.extend_from_slice(&chunk[..chunk.len().min(room)]);
        self.tail.extend_from_slice(chunk);
        if self.tail.len() > 2 * BUILD_OUTPUT_TAIL_CAP {
            self.tail.drain(..self.tail.len() - BUILD_OUTPUT_TAIL_CAP);
        }
    }

    /// The kept head as text. Lossy, deliberately: a partial UTF-8 sequence
    /// can land at the cap, and a decode error must not cost the error line
    /// before it.
    fn head_text(&self) -> String {
        String::from_utf8_lossy(&self.head).into_owned()
    }

    /// The last non-blank line the build printed, or `None` when it printed
    /// nothing but whitespace.
    fn last_line(&self) -> Option<String> {
        String::from_utf8_lossy(&self.tail)
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .map(str::to_string)
    }
}

#[cfg(test)]
#[path = "engine_build_tests.rs"]
mod tests;
