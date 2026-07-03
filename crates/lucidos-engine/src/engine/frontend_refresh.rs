//! Frontend-only Apply → advance the served client in-process (dev).
//! `docs/plans/2026-07-02-frontend-only-apply-served-in-dev.md`.
//!
//! At boot the engine pins `dist/` into a snapshot and serves that for its
//! lifetime (`api::frontend_snapshot`, INV-A: never serve a client that could be
//! incompatible with the running engine binary). A *frontend-only* Apply
//! (`files_require_restart == false`) never respawns the engine, so without this
//! the boot snapshot never advances and the client refresh badge/toast never fire
//! — the applied TS/CSS silently doesn't take effect until an unrelated restart.
//!
//! For a frontend-only change the engine binary is unchanged, so a newer client
//! built from that diff IS compatible. This module waits for the build-watch to
//! republish `dist/`, re-snapshots it into a fresh generation, and atomically
//! swaps what `api::serve_frontend` serves — so the served `sw.js` BUILD_ID
//! advances and the EXISTING client-update surface (`syncClientUpdateFromBuild`
//! → badge + Refresh toast) fires, with no engine respawn. Mixed changes still go
//! through the Switch flow (`trigger_background_rebuild`), which re-snapshots at
//! the new engine's boot — untouched here.
//!
//! When the advance is DEFERRED instead (INV-A: an engine version change is
//! already pending, so `dist/` holds a client for the new engine that can't be
//! served on the old one), the change still ships when the user Switches — but
//! the page would otherwise get no signal that a frontend-only Apply was queued.
//! So each deferral branch emits the transient `FrontendUpdateDeferred` UI event
//! (`emit_frontend_update_deferred`); the frontend renders a keyed hint toast.

use super::LucidosEngine;
use crate::api::frontend_snapshot;
use crate::engine::engine_version::BuildState;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// How long to wait for the build-watch to republish `dist/` after a
/// frontend-only Apply before giving up. A fresh `vite build` is sub-second here;
/// this is generous headroom. On timeout we simply don't swap (safe no-op) — e.g.
/// no build-watch is running, or a deterministic rebuild produced an identical
/// BUILD_ID (nothing new to serve).
const REBUILD_WAIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Poll cadence while waiting for the rebuild.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Grace period before removing the superseded snapshot after a swap, so an
/// in-flight request that already resolved the old path can finish serving from
/// it (hardlinked files also survive removal while an fd is open, but the delay
/// closes the read-path-then-open window entirely).
const CLEANUP_GRACE: Duration = Duration::from_secs(30);

/// Cadence of the per-engine peer-sync poll (dev only). A peer workspace has no
/// event signal that the checkout-shared `dist/` moved under another workspace's
/// frontend-only Apply, so each engine periodically re-checks. Cheap: reads one
/// `sw.js` build id per tick and only re-snapshots (+ emits) when it changed AND
/// the advance is INV-A-safe. 10s keeps "switch to the peer tab and see the
/// badge" prompt without meaningful idle cost.
const SERVED_FRONTEND_SYNC_INTERVAL: Duration = Duration::from_secs(10);

/// Whether the source `dist/` has been republished with a client different from
/// the one we currently serve — i.e. the build-watch's rebuild has landed.
/// `None` current (couldn't read the served snapshot) + a readable source → treat
/// as new enough to swap; an unreadable source (mid-rebuild / no `sw.js`) → keep
/// waiting. Pure so the poll's core decision is unit-testable.
fn source_rebuilt(current_id: Option<&str>, source_id: Option<&str>) -> bool {
    match (current_id, source_id) {
        (Some(cur), Some(src)) => cur != src,
        (None, Some(_)) => true,
        _ => false,
    }
}

/// Pure INV-A decision: is it safe to advance the served client in-process,
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

/// Pure INV-A decision for a PEER engine advancing its served snapshot to a
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

/// Pure INV-A decision for the APPLYING engine's own frontend-only advance. Here
/// the git check only VETOES: a `Some(false)` means a mixed change is in flight in
/// some workspace, so the rebuilt `dist/` targets a newer engine and must not be
/// served on this one (the deferred-hint branch fires instead). When git is
/// unavailable (`None`) we keep today's disk/build_state-gate behavior (fail-open),
/// so only `Some(false)` blocks.
fn applying_git_gate(engine_source_matches_head: Option<bool>) -> bool {
    engine_source_matches_head != Some(false)
}

impl LucidosEngine {
    /// Register the swappable served-frontend handle + its source dir. Called once
    /// by `api::create_router` when `LUCIDOS_STATIC_DIR` is set. A second call
    /// (e.g. a second router in a test) is ignored — the first registration wins.
    pub fn init_served_frontend(&self, handle: Arc<RwLock<PathBuf>>, source: PathBuf) {
        let _ = self.served_frontend.set(handle);
        let _ = self.served_frontend_source.set(source);
    }

    fn frontend_refresh_superseded(&self, generation: u64) -> bool {
        self.frontend_refresh_generation.load(Ordering::SeqCst) != generation
    }

    /// Emit the transient `FrontendUpdateDeferred` UI signal — the page-facing
    /// hint that a frontend-only Apply's in-process served-client advance was
    /// deferred because an engine version change is pending (INV-A). Fired from
    /// both deferral branches below; never persisted (a pure UI hint), so the
    /// frontend surfaces "your frontend change applies on Switch" instead of
    /// the user seeing nothing happen.
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

    /// INV-A gate for the in-process served-frontend advance — see
    /// [`frontend_advance_is_safe`]. Reads the live build state + on-disk engine
    /// build id (the latter behind its mtime cache, so it's cheap on the steady
    /// path).
    async fn engine_binary_is_current(&self) -> bool {
        let disk = self.engine_disk_build_id().await;
        frontend_advance_is_safe(&self.build_state(), disk.as_deref(), crate::ENGINE_BUILD_ID)
    }

    /// Runtime INV-A signal: is the running engine binary already current with
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
        // Running engine commit = the `sha` prefix of ENGINE_BUILD_ID (strip any
        // `-<diffhash>` dirty suffix). Empty (unstamped) or a `src-…` shipped id
        // (no git) → can't compare via git → unknown.
        let id = crate::ENGINE_BUILD_ID;
        if id.is_empty() || id.starts_with("src") {
            return None;
        }
        let commit = id.split('-').next().unwrap_or("");
        if commit.is_empty() {
            return None;
        }
        let root = crate::paths::repo_root().ok()?;
        let out = tokio::process::Command::new("git")
            .args(["diff", "--name-only", commit, "HEAD"])
            .current_dir(&root)
            .output()
            .await
            .ok()?;
        if !out.status.success() {
            return None; // e.g. `commit` unknown in this checkout → unknown
        }
        let files: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        // Safe iff NO restart-requiring (binary-affecting) file changed since the
        // running engine's commit — the exact inverse of the Apply path's mixed-
        // change decision, so we never strand a change it treats as frontend-only.
        Some(!crate::engine::git_ops::files_require_restart(&files))
    }

    /// Composed INV-A gate for the APPLYING engine's own frontend-only advance:
    /// the disk/build_state gate AND the git veto (see [`applying_git_gate`]).
    async fn applying_advance_is_safe(&self) -> bool {
        self.engine_binary_is_current().await
            && applying_git_gate(self.engine_source_matches_head().await)
    }

    /// Composed INV-A gate for the periodic PEER advance: the disk/build_state gate
    /// AND git-confirmed engine-source parity with HEAD (see [`peer_git_gate`]).
    async fn peer_advance_is_safe(&self) -> bool {
        self.engine_binary_is_current().await
            && peer_git_gate(self.engine_source_matches_head().await)
    }

    /// After a *frontend-only* Apply (engine binary unchanged), wait for the
    /// build-watch to republish `dist/`, then re-snapshot it into a fresh
    /// generation and atomically swap what the engine serves — so the served
    /// `sw.js` BUILD_ID advances and the client refresh badge/toast surface
    /// WITHOUT an engine respawn. The caller must only invoke this for a
    /// frontend-only, Lucidos-source change, which keeps INV-A.
    ///
    /// No-op in packaged (never rebuilds) or when the frontend isn't served
    /// (no `LUCIDOS_STATIC_DIR`). Coalesces: a later call supersedes an in-flight
    /// one (only the latest generation swaps). Mirrors `trigger_background_rebuild`.
    pub fn refresh_served_frontend_after_rebuild(self: &Arc<Self>) {
        if crate::runtime::is_packaged() {
            return;
        }
        let (Some(handle), Some(source)) = (
            self.served_frontend.get().cloned(),
            self.served_frontend_source.get().cloned(),
        ) else {
            return; // frontend not served (headless) — nothing to advance
        };
        let generation = self.frontend_refresh_generation.fetch_add(1, Ordering::SeqCst) + 1;
        // Coalesce: abort any in-flight refresh; the new generation supersedes it.
        if let Some(old) = self.frontend_refresh_task.lock().unwrap().take() {
            old.abort();
        }
        let engine = self.clone();
        let workspace = self.workspace_path().to_path_buf();
        let task = tokio::spawn(async move {
            engine
                .run_served_frontend_refresh(handle, source, workspace, generation)
                .await;
        });
        *self.frontend_refresh_task.lock().unwrap() = Some(task);
    }

    async fn run_served_frontend_refresh(
        self: Arc<Self>,
        handle: Arc<RwLock<PathBuf>>,
        source: PathBuf,
        workspace: PathBuf,
        generation: u64,
    ) {
        // INV-A early-out: if an engine version change is already pending (a prior
        // mixed Apply's rebuild is in flight/ready, a newer binary is on disk, or
        // git shows engine source diverged from HEAD — the cross-workspace case
        // where ANOTHER workspace's mixed Apply moved `dist/` while this engine's
        // binary is still old), the live `dist/` is built for the NEW engine —
        // advancing it onto the running old engine would serve an incompatible
        // client. Skip; the Switch advances client + engine together. (Re-checked
        // before the swap, since a mixed Apply can land during the poll window.)
        if !self.applying_advance_is_safe().await {
            crate::log!(
                "[Frontend] frontend-only Apply: an engine version change is pending — not advancing the served client (the Switch will)"
            );
            // Tell the page the change is queued for the Switch (not ignored).
            self.emit_frontend_update_deferred().await;
            return;
        }

        // The build id the running client loaded against (what we serve now).
        let current_dir = handle.read().unwrap().clone();
        let current_id = frontend_snapshot::read_build_id(&current_dir);

        // Wait for the build-watch to republish `dist/` with a different BUILD_ID.
        // Bounded — on timeout we simply don't swap (safe no-op).
        let deadline = Instant::now() + REBUILD_WAIT_TIMEOUT;
        loop {
            if self.frontend_refresh_superseded(generation) {
                return; // a newer refresh took over
            }
            let source_id = frontend_snapshot::read_build_id(&source);
            if source_rebuilt(current_id.as_deref(), source_id.as_deref()) {
                break;
            }
            if Instant::now() >= deadline {
                crate::log!(
                    "[Frontend] frontend-only Apply: dist/ did not republish within {}s — served snapshot unchanged",
                    REBUILD_WAIT_TIMEOUT.as_secs()
                );
                return;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        if self.frontend_refresh_superseded(generation) {
            return;
        }
        // Definitive INV-A re-check: a mixed Apply may have landed DURING the poll
        // above (its rebuild flips build_state to Building and republishes dist/
        // with an engine-requiring client, or a peer workspace's mixed Apply moved
        // engine source past HEAD). If so, the dist we're about to snapshot is for
        // the new engine — don't serve it on the running old engine.
        if !self.applying_advance_is_safe().await {
            crate::log!(
                "[Frontend] frontend-only Apply: an engine version change landed during the rebuild wait — not advancing the served client (the Switch will)"
            );
            // Tell the page the change is queued for the Switch (not ignored).
            self.emit_frontend_update_deferred().await;
            return;
        }

        // Pin a fresh snapshot of the rebuilt `dist/` and swap the served dir.
        let _ = self
            .advance_served_snapshot(&handle, &source, &workspace, generation)
            .await;
    }

    /// Pin a fresh snapshot of `source` and atomically swap the served handle to
    /// it, grace-cleaning the previous snapshot. Returns `true` if the swap
    /// happened. Shared by the applying-engine refresh
    /// ([`run_served_frontend_refresh`]) and the periodic peer sync
    /// ([`sync_served_frontend_if_safe`]); both honor the generation guard (a newer
    /// refresh supersedes this one) and the cleanup guard (only ever removes a dir
    /// UNDER the snapshot parent, never the live `dist/`). The caller is
    /// responsible for the INV-A gate before calling.
    async fn advance_served_snapshot(
        &self,
        handle: &Arc<RwLock<PathBuf>>,
        source: &Path,
        workspace: &Path,
        generation: u64,
    ) -> bool {
        match frontend_snapshot::pin_generation(source, workspace, generation) {
            Ok(new_dir) => {
                if self.frontend_refresh_superseded(generation) {
                    // A newer refresh won while we were copying; drop our snapshot.
                    frontend_snapshot::remove_snapshot_dir(&new_dir);
                    return false;
                }
                let prev = {
                    let mut guard = handle.write().unwrap();
                    std::mem::replace(&mut *guard, new_dir.clone())
                };
                crate::log!(
                    "[Frontend] serving re-snapshotted dist at {}",
                    new_dir.display()
                );
                // Grace-delayed cleanup of the previous snapshot. Guard: only remove
                // a dir UNDER the snapshot parent — never the live `dist/` (which the
                // boot fail-safe may serve directly).
                let parent = frontend_snapshot::snapshot_parent(workspace);
                if prev != new_dir && prev.starts_with(&parent) {
                    tokio::spawn(async move {
                        tokio::time::sleep(CLEANUP_GRACE).await;
                        frontend_snapshot::remove_snapshot_dir(&prev);
                    });
                }
                true
            }
            Err(e) => {
                // Fail-safe: leave the current served dir in place (never a 404).
                crate::log!("[Frontend] re-snapshot failed ({e}); served snapshot unchanged");
                false
            }
        }
    }

    /// Emit the transient `ServedFrontendAdvanced` UI signal — tells the connected
    /// client this engine just advanced its served snapshot to the shared `dist/`
    /// (a peer workspace's frontend-only Apply), so it re-runs
    /// `syncClientUpdateFromBuild` and surfaces the Refresh badge/toast. Mirrors
    /// [`Self::emit_frontend_update_deferred`].
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

    /// One periodic peer-sync check (dev only): if the checkout-shared `dist/` has
    /// advanced past what this engine serves AND advancing is INV-A-safe
    /// ([`Self::peer_advance_is_safe`]), re-snapshot in-process and broadcast
    /// `ServedFrontendAdvanced`. This is what lets a PEER workspace pick up ANOTHER
    /// workspace's frontend-only Apply without a manual restart — the applying
    /// engine advances only its own snapshot. No advance / no emit when a mixed
    /// change is pending (the Switch flow handles that peer instead).
    async fn sync_served_frontend_if_safe(self: &Arc<Self>) {
        if crate::runtime::is_packaged() {
            return;
        }
        let (Some(handle), Some(source)) = (
            self.served_frontend.get().cloned(),
            self.served_frontend_source.get().cloned(),
        ) else {
            return; // frontend not served (headless) — nothing to advance
        };
        // Cheap first: has the source `dist/` diverged from what we serve? Avoids
        // the git fork on the steady (unchanged) path — the common case per tick.
        let served_dir = handle.read().unwrap().clone();
        let served_id = frontend_snapshot::read_build_id(&served_dir);
        let source_id = frontend_snapshot::read_build_id(&source);
        if !source_rebuilt(served_id.as_deref(), source_id.as_deref()) {
            return;
        }
        // Diverged — is it safe to advance THIS engine to the newer client? The git
        // gate distinguishes a peer's frontend-only advance (safe) from a mixed
        // change still mid-rebuild (defer; the Switch advances this peer).
        if !self.peer_advance_is_safe().await {
            return; // mixed change pending → no in-process advance, no hint here
        }
        // Advance under a fresh generation so a concurrent applying refresh
        // coalesces (only the latest generation swaps).
        let generation = self.frontend_refresh_generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(old) = self.frontend_refresh_task.lock().unwrap().take() {
            old.abort();
        }
        let workspace = self.workspace_path().to_path_buf();
        if self
            .advance_served_snapshot(&handle, &source, &workspace, generation)
            .await
        {
            self.emit_served_frontend_advanced().await;
        }
    }

    /// Dev-only: spawn the periodic dev-maintenance loop (every
    /// [`SERVED_FRONTEND_SYNC_INTERVAL`]). Two responsibilities per tick:
    /// 1. **Served-frontend peer sync** (`sync_served_frontend_if_safe`) — a peer
    ///    workspace picks up another workspace's frontend-only Apply without a
    ///    manual restart. See `docs/plans/2026-07-03-cross-workspace-frontend-only-refresh.md`.
    /// 2. **Engine-version self-heal** (`self_heal_engine_version_if_needed`) — if
    ///    the engine source is behind HEAD with a stale on-disk binary (a mixed
    ///    Apply's rebuild failed / never ran), (re)trigger a coordinated background
    ///    rebuild so the Switch surfaces without a manual `-b`. See
    ///    `docs/plans/2026-07-03-engine-version-switch-selfheal.md`.
    ///
    /// Packaged returns immediately (no source rebuild); a headless engine (no
    /// served handle yet) ticks harmlessly — each step early-returns per tick, so
    /// this is robust to being spawned before the router registers the handle.
    pub fn spawn_served_frontend_sync(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let engine = self.clone();
        tokio::spawn(async move {
            if crate::runtime::is_packaged() {
                return;
            }
            let mut ticker = tokio::time::interval(SERVED_FRONTEND_SYNC_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                engine.sync_served_frontend_if_safe().await;
                // Same dev-only cadence drives engine-version self-heal: if the
                // engine SOURCE is behind HEAD with a stale on-disk binary
                // (a mixed Apply's rebuild failed / never ran), (re)trigger a
                // coordinated background rebuild so the Switch can surface without
                // a manual `-b`. No-op packaged / when not behind.
                engine.self_heal_engine_version_if_needed().await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        applying_git_gate, frontend_advance_is_safe, peer_git_gate, source_rebuilt, BuildState,
    };

    #[test]
    fn peer_git_gate_advances_only_on_confirmed_source_parity() {
        // Frontend-only: engine source matches HEAD → advance the peer.
        assert!(peer_git_gate(Some(true)));
        // Mixed change in flight (engine source diverged) → defer; the Switch
        // advances client + engine together on this peer.
        assert!(!peer_git_gate(Some(false)));
        // git unavailable → don't drag a peer forward on a guess (fail-safe).
        assert!(!peer_git_gate(None));
    }

    #[test]
    fn applying_git_gate_only_vetoes_a_confirmed_engine_change() {
        // Engine source matches HEAD → allow (defer to the disk gate for the rest).
        assert!(applying_git_gate(Some(true)));
        // A mixed change is in flight somewhere → veto (the deferred-hint fires).
        assert!(!applying_git_gate(Some(false)));
        // git unavailable → keep today's disk/build_state-gate behavior (fail-open).
        assert!(applying_git_gate(None));
    }

    #[test]
    fn source_rebuilt_fires_only_when_the_client_actually_changed() {
        // Same id → not yet republished (or deterministic identical rebuild).
        assert!(!source_rebuilt(Some("aaaa"), Some("aaaa")));
        // Different id → the rebuild landed.
        assert!(source_rebuilt(Some("aaaa"), Some("bbbb")));
        // Served snapshot unreadable but source is stamped → swap to the source.
        assert!(source_rebuilt(None, Some("bbbb")));
        // Source unreadable (mid-rebuild / no sw.js) → keep waiting.
        assert!(!source_rebuilt(Some("aaaa"), None));
        assert!(!source_rebuilt(None, None));
    }

    #[test]
    fn frontend_advance_is_safe_only_when_no_engine_change_is_pending() {
        // Clean engine: idle, on-disk binary matches the running one → safe.
        assert!(frontend_advance_is_safe(&BuildState::Idle, Some("engine1"), "engine1"));
        // Idle + no readable disk id (packaged / unreadable) → treat as current.
        assert!(frontend_advance_is_safe(&BuildState::Idle, None, "engine1"));

        // A mixed Apply's rebuild is in flight/ready → NOT safe (dist is for the
        // new engine; the Switch must advance client + engine together).
        assert!(!frontend_advance_is_safe(&BuildState::Building, Some("engine1"), "engine1"));
        assert!(!frontend_advance_is_safe(&BuildState::Ready, Some("engine1"), "engine1"));
        assert!(!frontend_advance_is_safe(&BuildState::Failed, Some("engine1"), "engine1"));

        // A newer engine binary is already on disk (e.g. an external/peer rebuild)
        // even though this engine is Idle → NOT safe.
        assert!(!frontend_advance_is_safe(&BuildState::Idle, Some("engine2"), "engine1"));
    }
}
