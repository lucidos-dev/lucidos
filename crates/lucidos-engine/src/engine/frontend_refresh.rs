use super::LucidosEngine;
use crate::engine::engine_version::{BuildState, FrontendBuildWatchStopped};
use crate::engine::frontend_snapshot;
use chrono::{DateTime, Utc};
use lucidos_owner_lock::{build_watch, ConsumerLock};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const REBUILD_WAIT_TIMEOUT: Duration = Duration::from_secs(60);

const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// INVARIANT: Grace period before removing the superseded snapshot after a swap, so an
/// in-flight request that already resolved the old path can finish serving from
/// it (hardlinked files also survive removal while an fd is open, but the delay
/// closes the read-path-then-open window entirely).
const CLEANUP_GRACE: Duration = Duration::from_secs(30);

fn build_failure_reason(status_json: &str) -> Option<String> {
    let doc: serde_json::Value = serde_json::from_str(status_json).ok()?;
    if doc.get("ok")?.as_bool()? {
        return None;
    }
    let reason = doc.get("error")?.as_str()?.trim();
    (!reason.is_empty()).then(|| reason.to_string())
}

fn build_succeeded_since(status_json: &str, since: DateTime<Utc>) -> bool {
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(status_json) else {
        return false;
    };
    doc.get("ok").and_then(serde_json::Value::as_bool) == Some(true)
        && doc
            .get("at")
            .and_then(serde_json::Value::as_str)
            .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
            .is_some_and(|at| at >= since)
}

fn build_watch_dir(served_dir: &Path) -> Option<PathBuf> {
    Some(build_watch::dir(served_dir.parent()?))
}

fn read_build_status(served_dir: &Path) -> Option<String> {
    std::fs::read_to_string(build_watch_dir(served_dir)?.join("status.json")).ok()
}

fn read_build_failure(served_dir: &Path) -> Option<String> {
    build_failure_reason(&read_build_status(served_dir)?)
}

fn read_build_succeeded_since(served_dir: &Path, since: DateTime<Utc>) -> bool {
    read_build_status(served_dir).is_some_and(|status| build_succeeded_since(&status, since))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuildWatchState {
    Running,
    Stopped,
    Unknown,
}

/// INVARIANT: The build-watch's state for the checkout that owns `served_dir`, read from
/// the watch lock beside the status file [`read_build_failure`] uses. The
/// watcher takes that lock blocking, so the probe can never make it lose.
fn read_build_watch_state(served_dir: &Path) -> BuildWatchState {
    let Some(app) = served_dir.parent() else {
        return BuildWatchState::Unknown;
    };
    if !build_watch::dir(app).is_dir() {
        return BuildWatchState::Unknown;
    }
    match lucidos_owner_lock::is_held(&build_watch::watch_lock_path(app)) {
        Ok(true) => BuildWatchState::Running,
        Ok(false) => BuildWatchState::Stopped,
        Err(_) => BuildWatchState::Unknown,
    }
}

const BUILD_WATCH_STOPPED_GRACE: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct BuildWatchTracker {
    stopped_since: Option<(Instant, i64)>,
}

impl BuildWatchTracker {
    fn observe(
        &mut self,
        state: BuildWatchState,
        now: Instant,
        now_ms: i64,
        cause: Option<String>,
    ) -> Option<FrontendBuildWatchStopped> {
        if state != BuildWatchState::Stopped {
            self.stopped_since = None;
            return None;
        }
        let (since, since_ms) = *self.stopped_since.get_or_insert((now, now_ms));
        (now.duration_since(since) >= BUILD_WATCH_STOPPED_GRACE)
            .then_some(FrontendBuildWatchStopped { since_ms, cause })
    }
}

fn read_build_watch_exit_cause(source: &Path) -> Option<String> {
    let record = std::fs::read_to_string(build_watch_dir(source)?.join("exit.json")).ok()?;
    let record: serde_json::Value = serde_json::from_str(&record).ok()?;
    let cause = record.get("cause")?.as_str()?.trim();
    (!cause.is_empty()).then(|| cause.to_owned())
}

fn build_watch_consumer_lock(source: &Path) -> Option<PathBuf> {
    let app = source.parent()?;
    app.join(build_watch::SCRIPT)
        .is_file()
        .then(|| build_watch::consumers_lock_path(app))
}

static BUILD_WATCH_CONSUMER: std::sync::OnceLock<ConsumerLock> = std::sync::OnceLock::new();

/// INVARIANT: Register this engine as a consumer of the build-watch that republishes
/// `source`. The teardown takes the consumer lock exclusively without waiting,
/// so it cannot stop the watch while any engine serves this checkout (ADR
/// 0438). `main` calls it before waiting for a predecessor. Taken on a thread,
/// because a teardown in progress must not stall boot. No-op outside a dev
/// checkout.
pub fn register_as_build_watch_consumer(source: &Path) {
    let Some(path) = build_watch_consumer_lock(source) else {
        return;
    };
    std::thread::spawn(move || match ConsumerLock::acquire(&path) {
        Ok(lock) => {
            let _ = BUILD_WATCH_CONSUMER.set(lock);
        }
        Err(e) => crate::log_legacy!(
            "[Frontend] could not register with the build-watch at {}: {e}",
            path.display()
        ),
    });
}

fn source_rebuilt(current_id: Option<&str>, source_id: Option<&str>) -> bool {
    match (current_id, source_id) {
        (Some(cur), Some(src)) => cur != src,
        (None, Some(_)) => true,
        _ => false,
    }
}

fn live_refresh_elapsed(started: Option<(u64, Instant)>, current: u64) -> Option<Duration> {
    started
        .filter(|(generation, _)| *generation == current)
        .map(|(_, at)| at.elapsed())
}

/// INVARIANT: Pure INV-A decision: is it safe to advance the served client in-process,
/// given the engine's rebuild state + on-disk vs running binary id?
///
/// Safe ONLY when no engine version change is pending: `build_state == Idle`
/// (no rebuild this engine triggered is in flight/ready) AND the on-disk binary
/// still matches the running one. If EITHER signals a pending engine change, the
/// live `dist/` was rebuilt from source that includes that engine change (new
/// endpoint / event / migration), so serving it on the still-running OLD engine
/// would reintroduce the incompatible pairing the snapshot exists to prevent —
/// the Switch must advance client + engine together instead. `None` disk id
/// mirrors `version_status`'s "no update" semantics (packaged / unreadable), and
/// `build_state` — set to `Building` synchronously the instant a mixed Apply
/// triggers the rebuild — covers the window before the new binary is even written.
fn frontend_advance_is_safe(
    build_state: &BuildState,
    disk_build_id: Option<&str>,
    running_build_id: &str,
) -> bool {
    if *build_state != BuildState::Idle {
        return false;
    }
    match disk_build_id {
        Some(disk) => disk == running_build_id,
        None => true,
    }
}

/// INVARIANT: Pure INV-A decision for a PEER engine advancing its served snapshot to a
/// client built from HEAD. The disk/build_state gate ([`frontend_advance_is_safe`])
/// is NOT enough cross-workspace: during ANOTHER workspace's mixed-change engine
/// rebuild the on-disk binary is still old (disk == running) for tens of seconds
/// while the build-watch has already republished `dist/` with a new-engine client,
/// so the disk gate would wrongly permit the advance. The load-bearing signal is
/// whether engine-relevant source changed between the running engine's commit and
/// HEAD (see [`LucidosEngine::engine_source_matches_head`]): `Some(true)` = no
/// engine change (frontend-only → safe), `Some(false)` = engine change in flight
/// (mixed → the Switch must advance client+engine together), `None` = git
/// unavailable → don't drag a peer forward on a guess (fail-safe; the manual
/// restart still works). So a peer advances ONLY on `Some(true)`.
fn peer_git_gate(engine_source_matches_head: Option<bool>) -> bool {
    engine_source_matches_head == Some(true)
}

fn applying_git_gate(engine_source_matches_head: Option<bool>) -> bool {
    engine_source_matches_head != Some(false)
}

impl LucidosEngine {
    pub fn init_served_frontend(&self, handle: Arc<RwLock<PathBuf>>, source: PathBuf) {
        self.served_frontend.get_or_init(|| {
            *self.activated_frontend_commit.lock().unwrap() =
                frontend_snapshot::read_source_commit(&handle.read().unwrap());
            let _ = self.served_frontend_source.set(source);
            handle
        });
    }

    pub(crate) fn served_frontend_source_commit(&self) -> Option<String> {
        let served_dir = self.served_frontend.get()?.read().unwrap().clone();
        frontend_snapshot::read_source_commit(&served_dir)
    }

    fn frontend_refresh_superseded(&self, generation: u64) -> bool {
        self.frontend_refresh_generation.load(Ordering::SeqCst) != generation
    }

    pub(crate) fn frontend_refresh_elapsed(&self) -> Option<Duration> {
        live_refresh_elapsed(
            *self.frontend_refresh_started.lock().unwrap(),
            self.frontend_refresh_generation.load(Ordering::SeqCst),
        )
    }

    async fn finish_frontend_refresh(&self, generation: u64) {
        {
            let mut slot = self.frontend_refresh_started.lock().unwrap();
            if slot.is_some_and(|(started, _)| started == generation) {
                *slot = None;
            }
        }
        self.emit_frontend_refresh_state_changed().await;
    }

    async fn emit_frontend_refresh_state_changed(&self) {
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::System(
                    crate::engine::event_bus::SystemEvent::FrontendRefreshStateChanged {
                        sent_at_ms: crate::engine::now_epoch_millis(),
                    },
                ),
                "[Frontend] FrontendRefreshStateChanged",
            )
            .await;
    }

    async fn emit_frontend_update_deferred(&self) {
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::System(
                    crate::engine::event_bus::SystemEvent::FrontendUpdateDeferred {
                        sent_at_ms: crate::engine::now_epoch_millis(),
                    },
                ),
                "[Frontend] FrontendUpdateDeferred",
            )
            .await;
    }

    /// INVARIANT: Emit the transient `FrontendUpdateStranded` UI signal — a frontend-only
    /// Apply that rebuilt but can never reach the served client, because the
    /// `dist/` this engine serves is not the one being republished. Deliberately
    /// NOT `FrontendUpdateDeferred`: that one promises "arrives on Switch", which
    /// here would be false.
    async fn emit_frontend_update_stranded(
        &self,
        served_dir: &Path,
        in_worktree: bool,
        build_error: Option<String>,
        build_watch_stopped: bool,
    ) {
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::System(
                    crate::engine::event_bus::SystemEvent::FrontendUpdateStranded {
                        served_dir: served_dir.display().to_string(),
                        served_in_worktree: in_worktree,
                        build_error,
                        build_watch_stopped,
                        sent_at_ms: crate::engine::now_epoch_millis(),
                    },
                ),
                "[Frontend] FrontendUpdateStranded",
            )
            .await;
    }

    async fn engine_binary_is_current(&self) -> bool {
        let disk = self.engine_disk_build_id().await;
        frontend_advance_is_safe(
            &self.build_state(),
            disk.as_deref(),
            crate::build_identity().id,
        )
    }

    /// INVARIANT: Runtime INV-A signal: is the running engine binary already current with
    /// HEAD, or is a restart-requiring (binary-affecting) engine change pending?
    /// `Some(true)` = NO restart-requiring file changed between the running
    /// engine's commit and HEAD (a client built from HEAD is compatible with this
    /// running binary — the change is frontend-only / test / docs); `Some(false)` =
    /// a restart-requiring change IS pending (a mixed change is in flight/applied —
    /// the rebuilt `dist/` targets a NEWER engine, so don't serve it on this one);
    /// `None` = couldn't determine (git unavailable, or an unstamped / shipped
    /// `ENGINE_BUILD_ID`).
    ///
    /// Reuses [`files_require_restart`](crate::engine::git_ops::files_require_restart)
    /// — the SAME classifier the Apply path uses to decide `restart_required` —
    /// over the files changed since the running engine's commit, so this gate
    /// agrees EXACTLY with whether a rebuild + Switch was (or would be) scheduled.
    /// A coarser pathspec diff over `crates/lucidos-engine` would wrongly flag
    /// restart-IGNORED engine files (a test `.rs`, a `.md`) and strand a genuinely
    /// frontend-only advance the Apply path never schedules a Switch for. This is
    /// the cross-workspace-correct complement to `engine_binary_is_current` (the
    /// disk-id gate): during another workspace's mixed rebuild the on-disk binary
    /// is still old (disk == running) for tens of seconds while `dist/` already
    /// holds a new-engine client, so the disk gate alone would wrongly permit an
    /// advance.
    pub(crate) async fn engine_source_matches_head(&self) -> Option<bool> {
        crate::engine::engine_version::own_source_matches_head().await
    }

    async fn applying_advance_is_safe(&self) -> bool {
        self.engine_binary_is_current().await
            && applying_git_gate(self.engine_source_matches_head().await)
    }

    async fn peer_advance_is_safe(&self) -> bool {
        self.engine_binary_is_current().await
            && peer_git_gate(self.engine_source_matches_head().await)
    }

    pub fn refresh_served_frontend_after_rebuild(self: &Arc<Self>) {
        if crate::runtime::is_packaged() {
            return;
        }
        let (Some(handle), Some(source)) = (
            self.served_frontend.get().cloned(),
            self.served_frontend_source.get().cloned(),
        ) else {
            return;
        };
        // INVARIANT: Taken here, not inside the task: the merge has just landed, and a fast
        // build must not finish before the task is first polled.
        let applied_at = Utc::now();
        let generation = self
            .frontend_refresh_generation
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        *self.frontend_refresh_started.lock().unwrap() = Some((generation, Instant::now()));
        if let Some(old) = self.frontend_refresh_task.lock().unwrap().take() {
            old.abort();
        }
        let engine = self.clone();
        let workspace = self.workspace_path().to_path_buf();
        let task = tokio::spawn(async move {
            engine.emit_frontend_refresh_state_changed().await;
            engine
                .clone()
                .run_served_frontend_refresh(handle, source, workspace, generation, applied_at)
                .await;
            engine.finish_frontend_refresh(generation).await;
        });
        *self.frontend_refresh_task.lock().unwrap() = Some(task);
    }

    async fn run_served_frontend_refresh(
        self: Arc<Self>,
        handle: Arc<RwLock<PathBuf>>,
        source: PathBuf,
        workspace: PathBuf,
        generation: u64,
        applied_at: DateTime<Utc>,
    ) {
        if !self.applying_advance_is_safe().await {
            crate::log_legacy!(
                "[Frontend] frontend-only Apply: an engine version change is pending — not advancing the served client (the Switch will)"
            );
            self.emit_frontend_update_deferred().await;
            return;
        }

        let current_dir = handle.read().unwrap().clone();
        let current_id = frontend_snapshot::read_build_id(&current_dir);

        let deadline = Instant::now() + REBUILD_WAIT_TIMEOUT;
        loop {
            if self.frontend_refresh_superseded(generation) {
                return;
            }
            let source_id = frontend_snapshot::read_build_id(&source);
            if source_rebuilt(current_id.as_deref(), source_id.as_deref()) {
                break;
            }
            if Instant::now() >= deadline {
                let in_worktree = crate::paths::path_is_in_cc_worktree(&source);
                let build_error = read_build_failure(&source);
                let watch_stopped = read_build_watch_state(&source) == BuildWatchState::Stopped;
                // INVARIANT: Decided at the deadline rather than on the first green build,
                // so a build queued behind it has had time to move the id.
                if !in_worktree && !watch_stopped && read_build_succeeded_since(&source, applied_at)
                {
                    crate::log_legacy!(
                        "[Frontend] frontend-only Apply: {} rebuilt with the same BUILD_ID, so \
                         the served client is already current",
                        source.display()
                    );
                    return;
                }
                if let Some(reason) = build_error.as_deref() {
                    crate::log_legacy!(
                        "[Frontend] the build-watch reports a FAILING build, which is why \
                         nothing republished: {reason}"
                    );
                }
                if watch_stopped {
                    crate::log_legacy!(
                        "[Frontend] the build-watch is NOT RUNNING, so nothing will republish \
                         {} until the stack is relaunched",
                        source.display()
                    );
                }
                if in_worktree {
                    crate::log_legacy!(
                        "[Frontend] frontend-only Apply STRANDED: {} is inside a coding-agent \
                         worktree, so the build-watch republishes a different directory and the \
                         served client can NEVER advance — relaunch the stack from the real \
                         checkout",
                        source.display()
                    );
                } else if !watch_stopped {
                    crate::log_legacy!(
                        "[Frontend] frontend-only Apply: {} did not republish within {}s — not \
                         advancing the served client yet. Recoverable: the periodic sync picks \
                         it up if the rebuild lands.",
                        source.display(),
                        REBUILD_WAIT_TIMEOUT.as_secs()
                    );
                }
                self.emit_frontend_update_stranded(
                    &source,
                    in_worktree,
                    build_error,
                    watch_stopped,
                )
                .await;
                return;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        if self.frontend_refresh_superseded(generation) {
            return;
        }
        // INVARIANT: Definitive INV-A re-check: a mixed Apply may have landed DURING the poll
        // above (its rebuild flips build_state to Building and republishes dist/
        // with an engine-requiring client, or a peer workspace's mixed Apply moved
        // engine source past HEAD). If so, the dist we're about to snapshot is for
        // the new engine — don't serve it on the running old engine.
        if !self.applying_advance_is_safe().await {
            crate::log_legacy!(
                "[Frontend] frontend-only Apply: an engine version change landed during the rebuild wait — not advancing the served client (the Switch will)"
            );
            self.emit_frontend_update_deferred().await;
            return;
        }

        self.advance_served_snapshot(&handle, &source, &workspace, generation)
            .await;
    }

    async fn advance_served_snapshot(
        &self,
        handle: &Arc<RwLock<PathBuf>>,
        source: &Path,
        workspace: &Path,
        generation: u64,
    ) {
        match frontend_snapshot::pin_generation(source, workspace, generation) {
            Ok(new_dir) => {
                if self.frontend_refresh_superseded(generation) {
                    frontend_snapshot::remove_snapshot_dir(&new_dir);
                    return;
                }
                let source_commit = frontend_snapshot::read_source_commit(&new_dir);
                let (prev, commit_write) = {
                    let mut guard = handle.write().unwrap();
                    let prev = std::mem::replace(&mut *guard, new_dir.clone());
                    let previous_source_commit = std::mem::replace(
                        &mut *self.activated_frontend_commit.lock().unwrap(),
                        source_commit.clone(),
                    );
                    let commit_write = source_commit
                        .filter(|commit| previous_source_commit.as_ref() != Some(commit))
                        .map(|source_commit| {
                            self.spawn_served_commit_write(previous_source_commit, source_commit)
                        });
                    (prev, commit_write)
                };
                crate::log_legacy!(
                    "[Frontend] serving re-snapshotted dist at {}",
                    new_dir.display()
                );
                let parent = frontend_snapshot::snapshot_parent(workspace);
                if prev != new_dir && prev.starts_with(&parent) {
                    tokio::spawn(async move {
                        tokio::time::sleep(CLEANUP_GRACE).await;
                        frontend_snapshot::remove_snapshot_dir(&prev);
                    });
                }
                self.emit_served_frontend_advanced().await;
                if let Some(write) = commit_write {
                    let _ = write.await;
                }
            }
            Err(e) => {
                crate::log_legacy!(
                    "[Frontend] re-snapshot failed ({e}); served snapshot unchanged"
                );
            }
        }
    }

    fn spawn_served_commit_write(
        &self,
        previous_source_commit: Option<String>,
        source_commit: String,
    ) -> tokio::task::JoinHandle<()> {
        use crate::engine::event_bus::{BusEvent, SystemEvent};
        let (written, next_waits_on) = tokio::sync::oneshot::channel();
        let earlier = self
            .served_commit_write_done
            .lock()
            .unwrap()
            .replace(next_waits_on);
        let bus = self.event_bus.clone();
        tokio::spawn(async move {
            if let Some(earlier) = earlier {
                let _ = earlier.await;
            }
            bus.emit_or_log(
                BusEvent::System(SystemEvent::ServedFrontendCommitChanged {
                    previous_source_commit,
                    source_commit,
                }),
                "[Frontend] ServedFrontendCommitChanged",
            )
            .await;
            let _ = written.send(());
        })
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub async fn advance_served_snapshot_for_test(&self, source: &Path) {
        let generation = self
            .frontend_refresh_generation
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        let handle = self
            .served_frontend
            .get()
            .expect("register the served frontend first");
        self.advance_served_snapshot(handle, source, self.workspace_path(), generation)
            .await;
    }

    /// INVARIANT: Emit the transient `ServedFrontendAdvanced` UI signal: this engine just
    /// swapped its served snapshot, so the client re-runs
    /// `syncClientUpdateFromBuild` and surfaces the Refresh badge and toast.
    async fn emit_served_frontend_advanced(&self) {
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::System(
                    crate::engine::event_bus::SystemEvent::ServedFrontendAdvanced {
                        sent_at_ms: crate::engine::now_epoch_millis(),
                    },
                ),
                "[Frontend] ServedFrontendAdvanced",
            )
            .await;
    }

    /// INVARIANT: One periodic peer-sync check (dev only): if the checkout-shared `dist/` has
    /// advanced past what this engine serves AND advancing is INV-A-safe
    /// ([`Self::peer_advance_is_safe`]), re-snapshot in-process and broadcast
    /// `ServedFrontendAdvanced`. This is what lets a PEER workspace pick up ANOTHER
    /// workspace's frontend-only Apply without a manual restart — the applying
    /// engine advances only its own snapshot. No advance / no emit when a mixed
    /// change is pending (the Switch flow handles that peer instead).
    /// Warn ONCE per process when the `dist/` this engine serves lives inside a
    /// coding-agent worktree — a configuration that can never track `main`, so
    /// every future frontend-only Apply will strand.
    ///
    /// This is deliberately a config check, not a build-id comparison. The
    /// obvious "is what I serve older than the source?" test is already
    /// [`Self::sync_served_frontend_if_safe`]'s own trigger and self-heals, so it can
    /// never fire for this failure; and because `repo_root()` resolves from
    /// `current_exe()`, a worktree-pinned engine's repo root IS the worktree, so
    /// comparing against the checkout's `dist/` finds no divergence either. The
    /// one thing reliably knowable from inside the pinned process is the shape of
    /// the path it was handed.
    ///
    /// Fires from the periodic tick rather than boot so it lands after the event
    /// bus has subscribers; latched so a ~10s cadence doesn't log forever. Log
    /// only — the user-facing event belongs to an actual stranded Apply
    /// ([`emit_frontend_update_stranded`]), which carries the same flag.
    fn warn_once_if_frontend_worktree_pinned(&self, source: &Path) {
        if !crate::paths::path_is_in_cc_worktree(source) {
            return;
        }
        if self
            .frontend_worktree_pin_warned
            .swap(true, Ordering::SeqCst)
        {
            return;
        }
        crate::log_legacy!(
            "[Frontend] WARNING: serving dist/ from a coding-agent worktree ({}). \
             A worktree is a throwaway checkout pinned to one commit, and the build-watch \
             republishes the real checkout's dist/, so frontend changes will NOT appear here. \
             Relaunch the stack from the real checkout (./scripts/web-dev.sh -w <workspace> -b). \
             See docs/plans/2026-07-26-worktree-pinned-stack-guard.md",
            source.display()
        );
    }

    pub async fn sync_served_frontend_if_safe(self: &Arc<Self>) {
        if !self.newer_version_visible() {
            return;
        }
        let (Some(handle), Some(source)) = (
            self.served_frontend.get().cloned(),
            self.served_frontend_source.get().cloned(),
        ) else {
            return;
        };
        self.warn_once_if_frontend_worktree_pinned(&source);
        let served_dir = handle.read().unwrap().clone();
        let served_id = frontend_snapshot::read_build_id(&served_dir);
        let source_id = frontend_snapshot::read_build_id(&source);
        if !source_rebuilt(served_id.as_deref(), source_id.as_deref()) {
            return;
        }
        if !self.peer_advance_is_safe().await {
            return;
        }
        // INVARIANT: Advance under a fresh generation so a concurrent applying refresh
        // coalesces (only the latest generation swaps).
        let superseded_applying_refresh = self.frontend_refresh_elapsed().is_some();
        let generation = self
            .frontend_refresh_generation
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        if let Some(old) = self.frontend_refresh_task.lock().unwrap().take() {
            old.abort();
        }
        // INVARIANT: The aborted task never reaches `finish_frontend_refresh`, so this
        // takeover owes the page the end-of-refresh poke.
        if superseded_applying_refresh {
            self.emit_frontend_refresh_state_changed().await;
        }
        let workspace = self.workspace_path().to_path_buf();
        self.advance_served_snapshot(&handle, &source, &workspace, generation)
            .await;
    }

    pub async fn track_frontend_build_watch(&self, tracker: &mut BuildWatchTracker) {
        if !self.newer_version_visible() {
            return;
        }
        let Some(source) = self.served_frontend_source.get() else {
            return;
        };
        let state = read_build_watch_state(source);
        let cause = (state == BuildWatchState::Stopped)
            .then(|| read_build_watch_exit_cause(source))
            .flatten();
        let sent_at_ms = crate::engine::now_epoch_millis();
        let next = tracker.observe(state, Instant::now(), sent_at_ms, cause);
        {
            let mut reported = self.frontend_build_watch_stopped.lock().unwrap();
            if *reported == next {
                return;
            }
            *reported = next;
        }

        use crate::engine::event_bus::{BusEvent, SystemEvent};
        self.event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::FrontendBuildWatchStateChanged { sent_at_ms }),
                "[Frontend] FrontendBuildWatchStateChanged",
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        applying_git_gate, build_failure_reason, build_succeeded_since, build_watch_consumer_lock,
        frontend_advance_is_safe, live_refresh_elapsed, peer_git_gate, read_build_failure,
        read_build_succeeded_since, read_build_watch_state, source_rebuilt, BuildState,
        BuildWatchState,
    };
    use lucidos_owner_lock::{build_watch, OwnerLock};
    use std::time::{Duration, Instant};

    #[test]
    fn every_served_snapshot_swap_announces_itself() {
        let src = include_str!("frontend_refresh.rs");
        let emit = concat!("self.emit_served_frontend", "_advanced()");
        let start = src
            .find("async fn advance_served_snapshot(")
            .expect("advance_served_snapshot exists");
        let swap = &src[start..start + src[start..].find("\n    }\n").expect("method end")];
        assert!(
            swap.contains(emit),
            "the swap itself must emit ServedFrontendAdvanced, so no caller can forget"
        );
        assert_eq!(
            src.matches(emit).count(),
            1,
            "ServedFrontendAdvanced is emitted in one place, the swap"
        );
    }

    #[test]
    fn a_refresh_is_live_only_while_its_generation_is_current() {
        let started = Instant::now() - Duration::from_secs(5);
        let elapsed = live_refresh_elapsed(Some((3, started)), 3).expect("current generation");
        assert!(elapsed >= Duration::from_secs(5));
        assert_eq!(live_refresh_elapsed(Some((3, started)), 4), None);
        assert_eq!(live_refresh_elapsed(None, 3), None);
    }

    #[test]
    fn peer_git_gate_advances_only_on_confirmed_source_parity() {
        assert!(peer_git_gate(Some(true)));
        assert!(!peer_git_gate(Some(false)));
        assert!(!peer_git_gate(None));
    }

    #[test]
    fn applying_git_gate_only_vetoes_a_confirmed_engine_change() {
        assert!(applying_git_gate(Some(true)));
        assert!(!applying_git_gate(Some(false)));
        assert!(applying_git_gate(None));
    }

    #[test]
    fn source_rebuilt_fires_only_when_the_client_actually_changed() {
        assert!(!source_rebuilt(Some("aaaa"), Some("aaaa")));
        assert!(source_rebuilt(Some("aaaa"), Some("bbbb")));
        assert!(source_rebuilt(None, Some("bbbb")));
        assert!(!source_rebuilt(Some("aaaa"), None));
        assert!(!source_rebuilt(None, None));
    }

    #[test]
    fn frontend_advance_is_safe_only_when_no_engine_change_is_pending() {
        assert!(frontend_advance_is_safe(
            &BuildState::Idle,
            Some("engine1"),
            "engine1"
        ));
        assert!(frontend_advance_is_safe(&BuildState::Idle, None, "engine1"));

        assert!(!frontend_advance_is_safe(
            &BuildState::building_now(),
            Some("engine1"),
            "engine1"
        ));
        assert!(!frontend_advance_is_safe(
            &BuildState::ready_from(Some("head1".into())),
            Some("engine1"),
            "engine1"
        ));
        assert!(!frontend_advance_is_safe(
            &BuildState::failed_with(crate::engine::engine_version::BuildFailure::plain(
                "error: boom".into()
            )),
            Some("engine1"),
            "engine1"
        ));

        assert!(!frontend_advance_is_safe(
            &BuildState::Idle,
            Some("engine2"),
            "engine1"
        ));
    }

    #[test]
    fn a_failing_build_is_read_out_of_the_watch_status() {
        let status = r#"{
          "ok": false,
          "at": "2026-08-21T11:13:53.249Z",
          "error": "[vite]: Rollup failed to resolve import \"jsqr\" from \"main.tsx\".",
          "skippedInstall": null
        }"#;
        assert_eq!(
            build_failure_reason(status).as_deref(),
            Some("[vite]: Rollup failed to resolve import \"jsqr\" from \"main.tsx\".")
        );
    }

    #[test]
    fn a_healthy_or_unreadable_status_adds_nothing() {
        for status in [
            r#"{"ok": true, "at": "2026-08-21T11:00:00Z", "error": null}"#,
            r#"{"ok": true, "at": "2026-08-21T11:00:00Z", "error": "an old failure"}"#,
            r#"{"ok": false, "error": null}"#,
            r#"{"ok": false, "error": ""}"#,
            r#"{"ok": false, "error": "   "}"#,
            r#"{"at": "2026-08-21T11:00:00Z"}"#,
            "not json at all",
            "",
        ] {
            assert_eq!(build_failure_reason(status), None, "spoke for {status:?}");
        }
    }

    #[test]
    fn the_status_is_looked_for_beside_the_served_dist() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path();
        std::fs::create_dir_all(app.join(".build-watch")).unwrap();
        std::fs::write(
            app.join(".build-watch/status.json"),
            r#"{"ok": false, "error": "boom"}"#,
        )
        .unwrap();
        assert_eq!(
            read_build_failure(&app.join("dist")).as_deref(),
            Some("boom")
        );

        let bare = tempfile::tempdir().unwrap();
        assert_eq!(read_build_failure(&bare.path().join("dist")), None);
    }

    fn at(rfc3339: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(rfc3339)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn a_green_build_after_the_apply_counts_as_landed() {
        let applied = at("2026-09-26T05:15:34Z");
        let status = r#"{"ok": true, "at": "2026-09-26T05:15:38.732Z", "error": null}"#;
        assert!(build_succeeded_since(status, applied));
    }

    #[test]
    fn only_a_green_build_after_the_apply_counts() {
        let applied = at("2026-09-26T05:15:34Z");
        for status in [
            r#"{"ok": true, "at": "2026-09-26T05:02:00Z", "error": null}"#,
            r#"{"ok": false, "at": "2026-09-26T05:15:38Z", "error": "boom"}"#,
            r#"{"ok": true, "at": "not a time"}"#,
            r#"{"ok": true}"#,
            "not json at all",
            "",
        ] {
            assert!(
                !build_succeeded_since(status, applied),
                "counted {status:?}"
            );
        }
    }

    #[test]
    fn the_green_build_is_read_beside_the_served_dist() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path();
        std::fs::create_dir_all(app.join(".build-watch")).unwrap();
        std::fs::write(
            app.join(".build-watch/status.json"),
            r#"{"ok": true, "at": "2026-09-26T05:15:38Z", "error": null}"#,
        )
        .unwrap();
        let applied = at("2026-09-26T05:15:34Z");
        assert!(read_build_succeeded_since(&app.join("dist"), applied));

        let bare = tempfile::tempdir().unwrap();
        assert!(!read_build_succeeded_since(
            &bare.path().join("dist"),
            applied
        ));
    }

    #[test]
    fn stopped_watch_requires_a_continuous_grace_period() {
        use super::BuildWatchTracker;
        let mut tracker = BuildWatchTracker::default();
        let start = Instant::now();
        let mut sample = |state, seconds| {
            tracker.observe(
                state,
                start + Duration::from_secs(seconds),
                1_000 + seconds as i64 * 1_000,
                Some("SIGTERM".to_owned()),
            )
        };
        assert!(sample(BuildWatchState::Stopped, 0).is_none());
        assert!(sample(BuildWatchState::Stopped, 59).is_none());
        assert!(sample(BuildWatchState::Unknown, 60).is_none());
        assert!(sample(BuildWatchState::Stopped, 70).is_none());
        let report = sample(BuildWatchState::Stopped, 130).unwrap();
        assert_eq!(report.since_ms, 71_000);
        assert_eq!(report.cause.as_deref(), Some("SIGTERM"));
        assert_eq!(sample(BuildWatchState::Stopped, 140), Some(report));
        assert!(sample(BuildWatchState::Running, 150).is_none());
        assert!(sample(BuildWatchState::Stopped, 160).is_none());
        assert_eq!(
            sample(BuildWatchState::Stopped, 220).unwrap().since_ms,
            161_000
        );
        assert!(sample(BuildWatchState::Unknown, 230).is_none());
    }

    #[test]
    fn stopped_watch_cause_comes_from_its_exit_record() {
        use super::read_build_watch_exit_cause;
        let app = tempfile::tempdir().unwrap();
        let source = app.path().join("dist");
        let state_dir = build_watch::dir(app.path());
        std::fs::create_dir_all(&state_dir).unwrap();
        assert_eq!(read_build_watch_exit_cause(&source), None);

        let record = state_dir.join("exit.json");
        std::fs::write(
            &record,
            r#"{"at":"2026-01-01T00:00:00Z","cause":"SIGTERM"}"#,
        )
        .unwrap();
        assert_eq!(
            read_build_watch_exit_cause(&source).as_deref(),
            Some("SIGTERM")
        );
        std::fs::write(record, "incomplete").unwrap();
        assert_eq!(read_build_watch_exit_cause(&source), None);
    }

    #[test]
    fn the_watch_state_is_read_from_the_watch_lock_beside_the_served_dist() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path();
        std::fs::create_dir_all(build_watch::dir(app)).unwrap();

        std::fs::write(build_watch::dir(app).join("status.json"), r#"{"ok": true}"#).unwrap();
        std::fs::write(build_watch::pid_path(app), std::process::id().to_string()).unwrap();
        assert_eq!(
            read_build_watch_state(&app.join("dist")),
            BuildWatchState::Stopped
        );

        let _watcher = OwnerLock::acquire(&build_watch::watch_lock_path(app)).unwrap();
        assert_eq!(
            read_build_watch_state(&app.join("dist")),
            BuildWatchState::Running
        );

        let bare = tempfile::tempdir().unwrap();
        assert_eq!(
            read_build_watch_state(&bare.path().join("dist")),
            BuildWatchState::Unknown
        );
    }

    #[test]
    fn only_a_dev_checkouts_dist_registers_as_a_build_watch_consumer() {
        let checkout = tempfile::tempdir().unwrap();
        std::fs::write(checkout.path().join(build_watch::SCRIPT), "").unwrap();
        assert_eq!(
            build_watch_consumer_lock(&checkout.path().join("dist")),
            Some(build_watch::consumers_lock_path(checkout.path()))
        );
        let bundle = tempfile::tempdir().unwrap();
        assert_eq!(
            build_watch_consumer_lock(&bundle.path().join("frontend")),
            None
        );
    }
}
