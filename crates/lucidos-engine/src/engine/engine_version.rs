//! Engine-version identity + "new version available" detection.
//!
//! The dev half of the unified *Switch to new version* flow (see
//! `docs/plans/2026-07-01-new-engine-version-switch-flow.md`): after an *Apply*
//! rebuilds the engine binary on disk in the background, a running engine detects
//! that its on-disk binary differs from the version it's running and surfaces the
//! switch. Mirrors the gateway self-reload (`GATEWAY_BUILD_ID` /
//! `gateway_update_available`).
//!
//! Packaged builds never rebuild from source — `update_available` is always false
//! here (their "new version" source is the release updater, `updater.rs`) — and
//! `BuildState` stays `Idle`.

use super::LucidosEngine;
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a `source_behind_head` verdict is reused before the underlying
/// `git diff` is re-run. version-status is polled ~4s per client; a shared TTL
/// bounds the git fork to at most one per interval regardless of client count.
/// Short enough that a freshly-merged engine change surfaces within a few seconds.
const SOURCE_BEHIND_TTL: Duration = Duration::from_secs(3);

/// Max background rebuilds the self-heal driver auto-triggers for a single HEAD
/// before giving up (until HEAD moves). Bounds a genuinely broken `main` so it
/// can't spin builds forever; an Apply or a manual rebuild is always still
/// available. Debug builds are ~1 min, so this is a few minutes of retrying.
const SELF_HEAL_MAX_ATTEMPTS_PER_HEAD: u32 = 5;

/// Dev background-rebuild state. `Idle` in packaged (no source rebuild) and
/// between Applies; the Phase-2 build orchestration drives the transitions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BuildState {
    /// No rebuild running and none pending.
    #[default]
    Idle,
    /// A background `cargo build` is in progress; the old engine keeps serving.
    Building,
    /// The rebuild finished; a newer on-disk binary is ready to switch onto.
    Ready,
    /// The rebuild failed (compile error). The old engine keeps running; the
    /// error is surfaced as "Build failed — view log".
    Failed,
}

impl BuildState {
    /// Kebab-case wire tag for the version-status response.
    pub fn as_wire(&self) -> &'static str {
        match self {
            BuildState::Idle => "idle",
            BuildState::Building => "building",
            BuildState::Ready => "ready",
            BuildState::Failed => "failed",
        }
    }
}

/// Memoized on-disk binary build id, keyed by the running binary's last-seen
/// mtime. A `None` mtime means "not yet checked"; a `None` `disk_build_id` means
/// the id couldn't be read (binary mid-rewrite / spawn failure / packaged).
#[derive(Default)]
pub struct UpdateCheck {
    last_mtime: Option<std::time::SystemTime>,
    disk_build_id: Option<String>,
}

/// Throttled cache of the `source_behind_head` verdict — see
/// [`LucidosEngine::source_behind_head`]. `checked_at == None` means "never
/// computed"; `behind` is the last verdict. TTL-gated (`SOURCE_BEHIND_TTL`), so
/// the git probe runs at most once per interval across all polling clients.
#[derive(Default)]
pub struct SourceBehindCache {
    checked_at: Option<Instant>,
    behind: bool,
}

/// Per-HEAD self-heal attempt bookkeeping — see
/// [`LucidosEngine::self_heal_engine_version_if_needed`]. `head` is the HEAD the
/// attempts were counted for; when HEAD moves the counter resets.
#[derive(Default)]
pub struct SelfHealState {
    head: Option<String>,
    attempts: u32,
}

/// Wire shape of `GET /api/v1/engine/version-status`.
#[derive(Serialize)]
pub struct VersionStatus {
    /// The running engine's baked build id.
    pub build_id: String,
    /// True when a newer engine version is ready to switch onto (dev: on-disk
    /// binary build-id differs; always false in packaged — see module docs).
    pub update_available: bool,
    /// The on-disk binary's build id (dev), or `None` when packaged / unreadable.
    /// The frontend keys the "Switch to new version" badge+toast dismissal on this
    /// so a dismiss sticks for THIS on-disk build but a genuinely newer build
    /// re-surfaces the switch (INV-C, plan 2026-07-02).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_build_id: Option<String>,
    /// True under the packaged desktop/service runtime. The frontend uses it to
    /// route to the release updater instead of the dev build/switch flow.
    pub packaged: bool,
    /// Dev background-rebuild state: `idle` | `building` | `ready` | `failed`.
    pub build_state: &'static str,
    /// True (dev only) when the engine SOURCE is behind HEAD by a
    /// restart-requiring change — i.e. a NEW engine version exists in source even
    /// if no fresh binary is on disk yet (rebuild failed / not run). Distinct from
    /// `update_available` (a fresh binary IS on disk). The frontend uses it to
    /// surface a pending new version + drive self-heal so the Switch can't be a
    /// dead-end. Always false packaged / when git is unavailable.
    pub source_behind_head: bool,
}

impl LucidosEngine {
    /// Stash the device actor at switch-request time so the graceful-shutdown
    /// boundary emit (which runs in the signal handler, with no HTTP context) can
    /// attribute the switch to "You" and recovery can auto-resume in-flight
    /// coding-agent threads. Called by the `/api/v1/restart` switch handler.
    pub fn set_restart_actor(&self, actor: Option<crate::engine::thread_events::MessageOrigin>) {
        *self.restart_actor.lock().unwrap() = actor;
    }

    /// Take (and clear) the stashed switch actor. Cleared so a later non-switch
    /// stop (stop.sh, external SIGUSR1) doesn't reuse a stale device actor — it
    /// then falls back to System attribution + manual Continue.
    pub fn take_restart_actor(&self) -> Option<crate::engine::thread_events::MessageOrigin> {
        self.restart_actor.lock().unwrap().take()
    }

    /// Enqueue a thread for auto-resume after a user-initiated switch (recovery).
    pub(crate) fn enqueue_switch_resume(&self, thread_id: uuid::Uuid) {
        self.pending_switch_resumes.lock().unwrap().push(thread_id);
    }

    /// Emit `ContinuationRequested` for every thread recovery queued for
    /// auto-resume after a user-initiated switch, so the spawn dispatcher boots a
    /// `--resume`. Called by `main.rs` AFTER the dispatcher subscribes — recovery
    /// runs before it, so the emit can't happen inside recovery. Engine-attributed
    /// (`actor: None`): the resume is a recovery consequence, not a device click,
    /// and the "Resumed" boundary keeps the Lucidos-mark chip.
    pub async fn resume_pending_switches(&self) {
        let ids = std::mem::take(&mut *self.pending_switch_resumes.lock().unwrap());
        for thread_id in ids {
            crate::engine::thread_events::emit_continuation_requested_or_log(
                &self.event_bus,
                thread_id,
                crate::engine::agent_recovery::AUTO_RESUME_AFTER_SWITCH_REASON,
                None,
                "[Recovery] ContinuationRequested (auto-resume after switch)",
            )
            .await;
        }
    }

    /// Current background-rebuild state.
    pub fn build_state(&self) -> BuildState {
        self.build_state.read().unwrap().clone()
    }

    /// Set the background-rebuild state (Phase 2 build orchestration).
    pub fn set_build_state(&self, state: BuildState) {
        *self.build_state.write().unwrap() = state;
    }

    /// The on-disk engine binary's build id (dev), or `None` when packaged or the
    /// id can't be read (binary mid-rewrite / spawn failure). Cheap on the steady
    /// path: only forks `current_exe --build-id` when the binary's mtime has moved
    /// since the last check; otherwise reuses the cached id. Mirrors
    /// `gateway_update_available`'s memoization.
    pub async fn engine_disk_build_id(&self) -> Option<String> {
        if crate::runtime::is_packaged() {
            return None;
        }
        let exe = std::env::current_exe().ok()?;
        let mtime = std::fs::metadata(&exe).and_then(|m| m.modified()).ok();
        // Fast path: mtime unchanged since the last check → reuse the cached id.
        {
            let cache = self.update_check.lock().unwrap();
            if cache.last_mtime == mtime {
                return cache.disk_build_id.clone();
            }
        }
        // mtime moved (or first check): ask the on-disk binary for its build id.
        let disk_id = match tokio::process::Command::new(&exe)
            .arg("--build-id")
            .output()
            .await
        {
            Ok(out) if out.status.success() => {
                let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
                (!id.is_empty()).then_some(id)
            }
            // Unreadable (mid-rewrite / failure): leave the cache untouched so the
            // next poll retries once the mtime settles.
            _ => return self.update_check.lock().unwrap().disk_build_id.clone(),
        };
        let mut cache = self.update_check.lock().unwrap();
        cache.last_mtime = mtime;
        cache.disk_build_id = disk_id.clone();
        disk_id
    }

    /// Whether the engine SOURCE is behind HEAD by a restart-requiring change —
    /// a NEW engine version exists in source even if no fresh binary is on disk
    /// yet. Reuses [`Self::engine_source_matches_head`] (the SAME git classifier
    /// the frontend-only-Apply veto uses), so this signal and that veto agree by
    /// construction. TTL-cached ([`SOURCE_BEHIND_TTL`]) so the `git diff` runs at
    /// most once per interval across all polling clients. Dev-only: false packaged.
    pub async fn source_behind_head(&self) -> bool {
        if crate::runtime::is_packaged() {
            return false;
        }
        {
            let cache = self.source_behind_cache.lock().unwrap();
            if cache.checked_at.is_some_and(|at| at.elapsed() < SOURCE_BEHIND_TTL) {
                return cache.behind;
            }
        }
        // `Some(false)` = a restart-requiring change is pending between the running
        // engine's commit and HEAD (a new engine version). `Some(true)`/`None`
        // (frontend-only / git unavailable) → not behind.
        let behind = self.engine_source_matches_head().await == Some(false);
        let mut cache = self.source_behind_cache.lock().unwrap();
        cache.checked_at = Some(Instant::now());
        cache.behind = behind;
        behind
    }

    /// Full version status for `GET /api/v1/engine/version-status`. A newer engine
    /// is available when the on-disk build id is readable and differs from the
    /// running one (always false in packaged, where `disk_build_id` is `None`).
    pub async fn version_status(&self) -> VersionStatus {
        let disk_build_id = self.engine_disk_build_id().await;
        let source_behind_head = self.source_behind_head().await;
        VersionStatus {
            build_id: crate::ENGINE_BUILD_ID.to_string(),
            update_available: disk_build_id
                .as_deref()
                .is_some_and(|id| id != crate::ENGINE_BUILD_ID),
            disk_build_id,
            packaged: crate::runtime::is_packaged(),
            build_state: self.build_state().as_wire(),
            source_behind_head,
        }
    }

    /// One periodic self-heal tick (dev only): if the engine SOURCE is behind
    /// HEAD by a restart-requiring change but no fresh binary is on disk,
    /// (re)trigger a background rebuild so the shared binary advances and the
    /// Switch surfaces — WITHOUT a manual `web-dev.sh -b`. Closes the wedge where a
    /// mixed Apply's rebuild failed (or was never run) and left every co-located
    /// workspace deferring frontend-only Applies to a Switch that never appeared.
    ///
    /// Coordinated + bounded:
    /// - Skips when a co-located engine is already building (shared-lock probe),
    ///   so the N workspaces don't stampede the shared `target/`.
    /// - Skips when a fresh binary IS on disk (`disk != running`) — the Switch is
    ///   already surfaced via `update_available`; switching, not rebuilding, is next.
    /// - Bounded to [`SELF_HEAL_MAX_ATTEMPTS_PER_HEAD`] per HEAD so a genuinely
    ///   broken `main` can't spin builds forever; the count resets when HEAD moves.
    ///
    /// No-op packaged / when git is unavailable. Driven by the dev periodic loop
    /// (`frontend_refresh::spawn_served_frontend_sync`).
    pub(crate) async fn self_heal_engine_version_if_needed(self: &Arc<Self>) {
        if crate::runtime::is_packaged() {
            return;
        }
        // Is a new engine version pending in source?
        if !self.source_behind_head().await {
            *self.self_heal_state.lock().unwrap() = SelfHealState::default();
            return;
        }
        // A rebuild (self-heal's or the Apply path's) is already in flight → wait.
        if self.build_state() == BuildState::Building {
            return;
        }
        // A fresh binary is already on disk → the Switch is surfaced via
        // `update_available`; don't rebuild (switching is the next step). `None`
        // (binary mid-rewrite / unreadable) → retry next tick.
        match self.engine_disk_build_id().await {
            Some(disk) if disk != crate::ENGINE_BUILD_ID => return,
            None => return,
            _ => {} // disk == running → the binary is stale; a rebuild is needed.
        }
        let head = current_head_sha().await;
        {
            let mut sh = self.self_heal_state.lock().unwrap();
            if sh.head != head {
                sh.head = head;
                sh.attempts = 0;
            }
            if sh.attempts >= SELF_HEAL_MAX_ATTEMPTS_PER_HEAD {
                return; // gave up for this HEAD until a new commit lands.
            }
            // Don't stampede a peer that's already building (racy probe; the
            // in-build lock in `run_engine_build` is the real guard). Quick
            // synchronous flock probe — no await while holding the state lock.
            if engine_build_in_progress_elsewhere() {
                return;
            }
            sh.attempts += 1;
        }
        crate::log!(
            "[Rebuild] self-heal: engine source is behind HEAD with a stale binary — triggering a background rebuild"
        );
        self.trigger_background_rebuild();
    }

    /// Kick off a background engine rebuild (dev only), the non-disruptive half of
    /// the *Switch to new version* flow. The running engine keeps serving; when the
    /// rebuild finishes, its on-disk binary's build-id differs from the running
    /// `ENGINE_BUILD_ID` (surfaced via `version_status`) → the frontend surfaces
    /// "New version available". A second call **coalesces**: the in-flight build is aborted (its
    /// child is `kill_on_drop`, so the cargo process dies) and a fresh build starts,
    /// so the build always reflects the latest merged source. No-op in packaged —
    /// it never rebuilds from source (its "new version" is the release updater).
    pub fn trigger_background_rebuild(self: &Arc<Self>) {
        if crate::runtime::is_packaged() {
            return;
        }
        let generation = self.build_generation.fetch_add(1, Ordering::SeqCst) + 1;
        // Coalesce: abort any running build task; its child is kill_on_drop.
        if let Some(old) = self.build_task.lock().unwrap().take() {
            old.abort();
        }
        self.set_build_state(BuildState::Building);
        let engine = self.clone();
        let workspace = self.workspace_path().to_path_buf();
        let handle = tokio::spawn(async move {
            // Push `building` over SSE so a connected client shows the spinner
            // immediately — the 4s version-status poll alone misses this transient
            // window (iOS suspends the timer on a backgrounded PWA), so the spinner
            // badge never showed. See engine_version::emit_build_state_changed.
            engine.emit_build_state_changed(&BuildState::Building).await;
            let outcome = run_engine_build(&workspace).await;
            // Only the latest generation updates state — a superseded build's
            // completion is ignored (a newer build is already `Building`).
            if engine.build_generation.load(Ordering::SeqCst) == generation {
                let state = match outcome {
                    EngineBuildOutcome::Succeeded => BuildState::Ready,
                    EngineBuildOutcome::Failed => BuildState::Failed,
                    // A co-located engine holds the shared build lock — this is
                    // NOT our compile failure, so must not surface the
                    // build-failed toast. Fall back to Idle; the peer's build
                    // advances the shared binary (→ `update_available` surfaces
                    // the Switch), and the self-heal driver retries next tick if
                    // the peer's build doesn't land.
                    EngineBuildOutcome::SkippedLocked => BuildState::Idle,
                };
                engine.set_build_state(state.clone());
                engine.emit_build_state_changed(&state).await;
            }
        });
        *self.build_task.lock().unwrap() = Some(handle);
    }

    /// Emit the transient `EngineBuildStateChanged` UI poke so a connected client
    /// learns of a background-rebuild transition over the live SSE stream instead
    /// of waiting on the throttled 4s version-status poll (which iOS suspends on a
    /// backgrounded PWA — the reason the "building" spinner badge never showed).
    /// The frontend handler re-runs the authoritative `checkEngineVersion` GET, so
    /// this is a pure nudge; `state` is informational. Dev-only: the sole caller
    /// (`trigger_background_rebuild`) already no-ops packaged.
    async fn emit_build_state_changed(&self, state: &BuildState) {
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::System(
                    crate::engine::event_bus::SystemEvent::EngineBuildStateChanged {
                        state: state.as_wire().to_string(),
                        sent_at_ms: crate::engine::now_epoch_millis(),
                    },
                ),
                "[Rebuild] EngineBuildStateChanged",
            )
            .await;
    }
}

/// Outcome of a background engine build. `SkippedLocked` is distinct from
/// `Failed`: a co-located engine holds the checkout-shared build lock, so THIS
/// engine deliberately didn't build — not a compile failure (must not surface the
/// "build failed" toast).
enum EngineBuildOutcome {
    Succeeded,
    Failed,
    SkippedLocked,
}

/// Try to acquire the checkout-shared engine-build lock — an advisory `flock`
/// (auto-released on drop / process death) under the shared `target/`. Returns
/// the held file guard, or `None` when another co-located engine already holds it.
/// Co-located workspaces share ONE checkout + ONE `target/`, so only one
/// `web-dev.sh --engine-build` may run at a time — concurrent cargo builds on the
/// same target OOM/corrupt (CLAUDE.md). Keep the returned guard alive for the
/// whole build; dropping it releases the lock.
fn try_acquire_engine_build_lock() -> Option<std::fs::File> {
    try_lock_file(&engine_build_lock_path()?)
}

/// Path of the checkout-shared engine-build lock, or `None` when the checkout
/// can't be resolved (`repo_root` unavailable). Split out so `run_engine_build`
/// can distinguish "no checkout to coordinate on" (proceed uncoordinated) from
/// "a peer holds the lock" (skip) — the two must not be conflated.
fn engine_build_lock_path() -> Option<std::path::PathBuf> {
    Some(
        crate::paths::repo_root()
            .ok()?
            .join("target")
            .join(".lucidos-engine-build.lock"),
    )
}

/// Open (creating parents) and non-blocking advisory-`flock` `lock_path`, returning
/// the held guard or `None` when another open file description already holds it.
/// Pure w.r.t. the path so the single-holder invariant is unit-testable without a
/// repo checkout. `flock` is per-open-file-description on Unix, so a second open of
/// the same path — even in this process, and across processes — conflicts.
fn try_lock_file(lock_path: &std::path::Path) -> Option<std::fs::File> {
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(lock_path)
        .ok()?;
    fs2::FileExt::try_lock_exclusive(&file).ok()?;
    Some(file)
}

/// Cheap "is a co-located engine building right now?" probe: try the shared build
/// lock and immediately release it. Racy by nature (the real guard is the lock
/// held across [`run_engine_build`]), it just lets the self-heal driver avoid
/// churning a build attempt while a peer is already building. Treats an
/// un-acquirable lock (peer holds it, or git/lockfile unavailable) as "busy" —
/// the driver then skips this tick and retries, which is safe.
fn engine_build_in_progress_elsewhere() -> bool {
    match try_acquire_engine_build_lock() {
        Some(file) => {
            let _ = fs2::FileExt::unlock(&file);
            false
        }
        None => true,
    }
}

/// The checkout's current HEAD sha (`git rev-parse HEAD`), or `None` when git is
/// unavailable. Keys the self-heal attempt counter so a broken `main` gives up
/// per-HEAD and retries once new work lands.
async fn current_head_sha() -> Option<String> {
    let root = crate::paths::repo_root().ok()?;
    let out = tokio::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&root)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Run `web-dev.sh --engine-build -w <ws>` to rebuild the engine binary on disk,
/// appending output to the workspace engine log. `kill_on_drop` so a coalescing
/// abort kills the cargo process instead of orphaning it. Holds the
/// checkout-shared build lock for the whole build so co-located engines can't run
/// concurrent cargo builds on the same `target/`; returns `SkippedLocked` when a
/// peer already holds it.
async fn run_engine_build(workspace: &std::path::Path) -> EngineBuildOutcome {
    // Elect a single builder across co-located engines (see the lock helper).
    // Held (via `_build_lock`) until this function returns. When the checkout
    // can't be resolved there's no shared `target/` to coordinate on — proceed
    // uncoordinated (fail-open) rather than never building; only a genuinely
    // held lock means a peer is building (→ SkippedLocked).
    let _build_lock = match engine_build_lock_path() {
        Some(path) => match try_lock_file(&path) {
            Some(guard) => Some(guard),
            None => {
                crate::log!(
                    "[Rebuild] engine build already in progress on this checkout — skipping (a co-located workspace is building)"
                );
                return EngineBuildOutcome::SkippedLocked;
            }
        },
        None => None,
    };
    let script = match crate::paths::script("web-dev.sh") {
        Ok(s) => s,
        Err(e) => {
            crate::log!("[Rebuild] cannot locate web-dev.sh: {e}");
            return EngineBuildOutcome::Failed;
        }
    };
    let ws = workspace.to_string_lossy().to_string();
    let log_path = workspace.join(".lucidos/engine.log");
    let mut cmd = tokio::process::Command::new(&script);
    cmd.args(["-w", &ws, "--engine-build"]).kill_on_drop(true);
    if let Ok(f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        match f.try_clone() {
            Ok(f2) => {
                cmd.stdout(f).stderr(f2);
            }
            Err(_) => {
                cmd.stdout(f);
            }
        }
    }
    match cmd.status().await {
        Ok(status) if status.success() => EngineBuildOutcome::Succeeded,
        Ok(status) => {
            crate::log!("[Rebuild] engine build exited {status}");
            EngineBuildOutcome::Failed
        }
        Err(e) => {
            crate::log!("[Rebuild] failed to spawn engine build: {e}");
            EngineBuildOutcome::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::try_lock_file;

    /// The load-bearing single-builder invariant: while one holder has the build
    /// lock, no other acquire succeeds; the lock releases on drop. `flock` is
    /// per-open-file-description on Unix, so this same-process check faithfully
    /// mirrors the cross-engine (cross-process) case that serializes rebuilds.
    #[test]
    fn build_lock_admits_a_single_holder_and_releases_on_drop() {
        let dir =
            std::env::temp_dir().join(format!("lucidos-buildlock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".lucidos-engine-build.lock");

        let held = try_lock_file(&path).expect("first acquire succeeds");
        assert!(
            try_lock_file(&path).is_none(),
            "a second acquire must fail while the lock is held (single builder)"
        );
        drop(held);
        let reacquired = try_lock_file(&path).expect("acquire succeeds again after release");
        drop(reacquired);

        std::fs::remove_dir_all(&dir).ok();
    }
}
