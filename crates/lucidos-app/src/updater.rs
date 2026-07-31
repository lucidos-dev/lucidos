//! Auto-update wiring (`tauri-plugin-updater`).
//!
//! A packaged build surfaces updates INSIDE the workspace UI — not a native
//! launch dialog, not the picker. Most users have a single workspace and auto-open
//! straight into it, so the web app (running in the packaged Tauri client) polls
//! [`check_app_update`] on startup, on an interval, and whenever the window comes
//! back to the foreground, and shows an in-app "Update & restart" toast. The three
//! nets and their throttling live in
//! `crates/lucidos-app/src/store/actions/app-update.ts`. The toast's action calls
//! [`install_app_update_and_restart`], which installs the new signed bundle and
//! restarts the WHOLE stack onto the new version — the launchd background service
//! (gateway + per-workspace engines + embedded Postgres) AND the GUI client —
//! rather than only relaunching the window.
//!
//! **The install narrates itself.** Downloading ~100 MB, verifying it, swapping the
//! `.app` bundle and restarting the stack takes long enough that a silent `await`
//! reads as a frozen app — which is exactly what it did until the progress
//! callbacks below were wired up. Every step emits an [`AppUpdateProgress`] frame
//! on the [`PROGRESS_EVENT`] Tauri event, so the page can say what is happening and
//! how far along it is (`crates/lucidos-app/src/store/actions/app-update.ts`).
//! The phases are ordered: `checking` → `downloading` → `verifying` → `installing`
//! → `restarting-services` → `relaunching`, with `cancelled` / `failed` as the two
//! terminal off-ramps.
//!
//! **Only the download is cancellable.** Until the bytes are verified they exist
//! only in memory, so abandoning them changes nothing on disk. Once the bundle
//! swap starts there is nothing to go back to, so [`cancel_app_update`] is refused
//! from that point on — the [`AppUpdateRun`] state machine is what makes that a
//! structural guarantee rather than a timing accident.
//!
//! Distribution model: the `.dmg` is for first install; the updater ships the
//! `.app.tar.gz` + its `.sig` and `latest.json` (all on the same GitHub Release).
//! Update artifacts are signed with the Tauri updater key (`plugins.updater.pubkey`
//! in config; `TAURI_SIGNING_PRIVATE_KEY` at build time) — separate from Apple
//! notarization, which gates the first-install `.dmg`.
//!
//! No-op in development (the updater endpoint isn't reachable and there's no
//! launchd service to restart).

use serde::Serialize;
use std::sync::Mutex;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

/// Tauri event carrying [`AppUpdateProgress`] frames to the page. Emitted with
/// `AppHandle::emit`, so every webview of the client sees it — the same mechanism
/// `native-notification-tapped` uses, and already covered by `core:default` on both
/// the local and the gateway origin (see `desktop::gateway_capability`).
const PROGRESS_EVENT: &str = "app-update-progress";

/// Minimum byte advance between two `downloading` frames when the server declared
/// no `Content-Length`. A known-size download steps by whole percentage points
/// instead; this is the fallback for the case where there is no percentage to step
/// on. Either way the frame count stays ~100 for a ~100 MB payload rather than one
/// per network chunk.
const PROGRESS_BYTE_STEP: u64 = 1024 * 1024;

/// Where an update run currently is. Ordered as the run proceeds; `Cancelled` and
/// `Failed` are terminal off-ramps.
///
/// Serialized internally-tagged so the page reads it as a discriminated union on
/// `phase`, with kebab-case wire values (CLAUDE.md: public API parameter values are
/// kebab-case). The TypeScript mirror is `AppUpdateProgress` in `utils/tauri.ts`.
#[derive(Clone, Serialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
enum AppUpdatePhase {
    /// Asking the update endpoint whether there is anything newer.
    Checking,
    /// Streaming the new bundle. `total` is `None` when the server sent no
    /// `Content-Length` — the page must then show bytes without a percentage
    /// rather than invent one.
    Downloading { downloaded: u64, total: Option<u64> },
    /// Bytes are in; checking the updater signature over them.
    Verifying,
    /// Unpacking and swapping the `.app` bundle. No way back from here.
    Installing,
    /// Kicking the launchd background service onto the new binaries.
    RestartingServices,
    /// Re-execing the client. The page is about to be torn down.
    Relaunching,
    /// The user abandoned the download; nothing on disk changed.
    Cancelled,
    /// Terminal failure, carrying the reason (the page shows it — this runs on a
    /// click, so it is owed a real error, not a `console.warn`).
    Failed { message: String },
}

/// One progress frame: the phase plus the version it applies to (`None` until the
/// check resolves, since we don't know what we're installing before then).
#[derive(Clone, Serialize)]
struct AppUpdateProgress {
    version: Option<String>,
    #[serde(flatten)]
    phase: AppUpdatePhase,
}

/// Announce a phase to the page. Best-effort by design: the last frames race the
/// client teardown, and a progress frame that fails to reach a webview must never
/// be the thing that fails an update.
fn emit(app: &AppHandle, version: Option<&str>, phase: AppUpdatePhase) {
    let _ = app.emit(
        PROGRESS_EVENT,
        AppUpdateProgress {
            version: version.map(str::to_string),
            phase,
        },
    );
}

/// Announce a terminal failure AND return it as the command's error string. Both
/// halves matter: the event gives the page a terminal phase even if the promise
/// rejection races the teardown, and the returned string is what the caller's
/// `catch` reports.
fn fail(app: &AppHandle, version: Option<&str>, message: String) -> String {
    emit(
        app,
        version,
        AppUpdatePhase::Failed {
            message: message.clone(),
        },
    );
    message
}

/// A failed run, carrying the version it failed on when we had got far enough to
/// know one. Kept separate from the plain error string so the single [`fail`]
/// callsite can attribute the failure.
struct UpdateFailure {
    version: Option<String>,
    message: String,
}

impl UpdateFailure {
    fn new(version: Option<String>, message: String) -> Self {
        Self { version, message }
    }
}

/// One `downloading` frame worth announcing.
struct DownloadFrame {
    downloaded: u64,
    total: Option<u64>,
}

/// Byte-progress bookkeeping for a single download, kept out of the callback so
/// the throttle is unit-testable on its own.
///
/// The plugin invokes the chunk callback once per network chunk — thousands of
/// times for the bundle — and every frame is an IPC message plus a signal-graph
/// update. So a frame is only produced on a *meaningful* advance: a whole
/// percentage point when the size is known, [`PROGRESS_BYTE_STEP`] bytes when it
/// isn't. The first chunk and the final byte count always produce one, so the page
/// never sits at "0 bytes" during a slow first chunk, and never freezes one step
/// short of the true total.
#[derive(Default)]
struct DownloadTracker {
    downloaded: u64,
    /// Size the server declared, remembered from the chunk callback so the
    /// end-of-stream frame reports the real value instead of inventing one.
    total: Option<u64>,
    /// Cumulative count carried by the last frame produced; `None` before the
    /// first.
    last_announced: Option<u64>,
}

impl DownloadTracker {
    /// Record `len` more bytes, returning a frame when the advance clears the
    /// throttle.
    fn chunk(&mut self, len: u64, total: Option<u64>) -> Option<DownloadFrame> {
        self.downloaded += len;
        if total.is_some() {
            self.total = total;
        }
        let Some(last) = self.last_announced else {
            return Some(self.announce());
        };
        if self.downloaded <= last {
            return None;
        }
        let worth_announcing = match self.total {
            Some(total) if total > 0 => {
                self.downloaded >= total || percent(self.downloaded, total) > percent(last, total)
            }
            _ => self.downloaded - last >= PROGRESS_BYTE_STEP,
        };
        worth_announcing.then(|| self.announce())
    }

    /// End-of-stream frame, unless the final count was already the last one
    /// announced.
    fn finish(&mut self) -> Option<DownloadFrame> {
        (self.last_announced != Some(self.downloaded)).then(|| self.announce())
    }

    fn announce(&mut self) -> DownloadFrame {
        self.last_announced = Some(self.downloaded);
        DownloadFrame {
            downloaded: self.downloaded,
            total: self.total,
        }
    }
}

/// Whole percent of `total` that `bytes` represents. `saturating_mul` keeps an
/// absurdly large payload from wrapping instead of pinning at 100.
fn percent(bytes: u64, total: u64) -> u64 {
    bytes.saturating_mul(100) / total
}

/// What the single update slot is doing. Modelled as a state machine because the
/// cancellation rule *is* a state question: a cancel must abort the download and
/// must not touch an install already under way, and no timing accident should be
/// able to blur the two.
#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    /// Slot claimed; the abortable task has not been handed over yet. A window of
    /// a few instructions, but a cancel can still land in it.
    Starting,
    /// A cancel landed during [`Phase::Starting`] — the task is aborted the moment
    /// it arrives.
    CancelPending,
    /// The check + download task, still abortable.
    Downloading(JoinHandle<()>),
    /// Past the download: the bundle swap and the restart cannot be undone, so a
    /// cancel from here is refused.
    Committed,
}

/// The single in-flight app-update run. Managed state (`lib.rs`), shared by
/// [`install_app_update_and_restart`] and [`cancel_app_update`].
#[derive(Default)]
pub struct AppUpdateRun(Mutex<Phase>);

impl AppUpdateRun {
    /// Claim the slot. `false` when a run is already under way — the caller must
    /// then bail out WITHOUT emitting a terminal phase, or it would wipe the live
    /// run's narration out of the UI.
    fn begin(&self) -> bool {
        let mut phase = self.lock();
        if !matches!(*phase, Phase::Idle) {
            return false;
        }
        *phase = Phase::Starting;
        true
    }

    /// Hand over the abortable download task. A cancel that landed during
    /// [`Phase::Starting`] is honoured here, which is what closes that window.
    fn armed(&self, task: JoinHandle<()>) {
        let mut phase = self.lock();
        match *phase {
            Phase::CancelPending => {
                task.abort();
                *phase = Phase::Idle;
            }
            _ => *phase = Phase::Downloading(task),
        }
    }

    /// The bytes are in hand — take the run past the point of no return.
    ///
    /// `false` when a cancel got there FIRST: the download resolves and the
    /// awaiting command is scheduled, but a cancel landing in that gap has
    /// already been accepted (`cancel` returned true and put the slot back to
    /// `Idle`) even though the buffered result still arrives. Committing anyway
    /// would install an update the user cancelled and told them nothing. Nothing
    /// is on disk yet, so honouring the cancel is both possible and the only
    /// honest outcome — the caller discards the bytes.
    fn commit(&self) -> bool {
        let mut phase = self.lock();
        if !matches!(*phase, Phase::Downloading(_)) {
            return false;
        }
        *phase = Phase::Committed;
        true
    }

    /// Free the slot after a run ended without restarting.
    ///
    /// Deliberately NOT called on the cancel path: [`Self::cancel`] already
    /// returned the slot to `Idle`, so a later release from the abandoned run
    /// could reset a *replacement* run that had since begun. Whoever transitions
    /// to `Idle` owns that transition.
    fn release(&self) {
        *self.lock() = Phase::Idle;
    }

    /// Abort an in-flight download. `false` when there is nothing abortable —
    /// either no run at all, or one that has already committed.
    fn cancel(&self) -> bool {
        let mut phase = self.lock();
        match std::mem::take(&mut *phase) {
            Phase::Starting => {
                *phase = Phase::CancelPending;
                true
            }
            Phase::Downloading(task) => {
                task.abort();
                *phase = Phase::Idle;
                true
            }
            other => {
                *phase = other;
                false
            }
        }
    }

    /// A poisoned lock here would mean a panic while holding it; the guarded state
    /// is a plain enum with no invariant a panic could have broken mid-write, so
    /// recovering is strictly better than propagating the panic into every later
    /// update attempt.
    fn lock(&self) -> std::sync::MutexGuard<'_, Phase> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Check GitHub Releases for a newer signed build. Returns the new version string
/// when an update is available, else `None`. The packaged workspace UI polls this
/// (gated on running in the Tauri client) to drive the in-app update toast.
/// No-op (`None`) in development.
#[tauri::command]
pub async fn check_app_update(app: AppHandle) -> Result<Option<String>, String> {
    if tauri::is_dev() {
        return Ok(None);
    }
    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater.check().await.map_err(|e| e.to_string())?;
    Ok(update.map(|u| u.version.clone()))
}

/// Resolve the available update and stream it into memory, announcing `checking`,
/// `downloading` and `verifying` as it goes. Returns the update alongside its
/// bytes; the caller installs them.
///
/// Runs inside a spawned task so it can be aborted — which is why it reports
/// failures as a value rather than emitting them itself: an aborted task never
/// gets to run cleanup, so the single terminal-phase emit lives in the caller.
async fn check_and_download(
    app: &AppHandle,
    updater: Updater,
) -> Result<(Update, Vec<u8>), UpdateFailure> {
    let update = updater
        .check()
        .await
        .map_err(|e| UpdateFailure::new(None, e.to_string()))?
        .ok_or_else(|| UpdateFailure::new(None, "No update available".to_string()))?;
    let version = update.version.clone();

    // Announce the phase flip before the first chunk lands: on a slow link the
    // first chunk can be seconds away, and until then "Checking…" would be a lie.
    emit(
        app,
        Some(&version),
        AppUpdatePhase::Downloading {
            downloaded: 0,
            total: None,
        },
    );

    // Shared by both callbacks (one `FnMut`, one `FnOnce`), neither of which
    // awaits — so the guard is never held across a suspension point.
    let tracker = Mutex::new(DownloadTracker::default());
    let bytes = update
        .download(
            |chunk, total| {
                let frame = tracker
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .chunk(chunk as u64, total);
                if let Some(frame) = frame {
                    emit(
                        app,
                        Some(&version),
                        AppUpdatePhase::Downloading {
                            downloaded: frame.downloaded,
                            total: frame.total,
                        },
                    );
                }
            },
            || {
                // Signature verification runs after the last chunk, still inside
                // `download` — so this hook is where `verifying` belongs.
                let frame = tracker.lock().unwrap_or_else(|e| e.into_inner()).finish();
                if let Some(frame) = frame {
                    emit(
                        app,
                        Some(&version),
                        AppUpdatePhase::Downloading {
                            downloaded: frame.downloaded,
                            total: frame.total,
                        },
                    );
                }
                emit(app, Some(&version), AppUpdatePhase::Verifying);
            },
        )
        .await
        .map_err(|e| UpdateFailure::new(Some(version.clone()), e.to_string()))?;

    Ok((update, bytes))
}

/// Install the available update and restart EVERYTHING onto the new version:
/// download + swap the bundle, restart the launchd background service (gateway +
/// engines + embedded Postgres) so it runs the NEW binaries, then relaunch the GUI
/// client. Never returns on success (the client re-execs).
///
/// Ordering is load-bearing: install first (new bytes on disk), then the service
/// restart, then the never-returning `app.restart()`. The service restart is
/// best-effort — a failure is logged but does not abort the client relaunch (the
/// service otherwise picks up the new binary on its next restart / reboot).
#[tauri::command]
pub async fn install_app_update_and_restart(
    app: AppHandle,
    run: State<'_, AppUpdateRun>,
) -> Result<(), String> {
    if tauri::is_dev() {
        return Err("Updates are only available in a packaged build".to_string());
    }
    if !run.begin() {
        // Deliberately no `failed` emit: that would replace the RUNNING update's
        // narration with an error about this duplicate request.
        return Err("An update is already in progress".to_string());
    }

    emit(&app, None, AppUpdatePhase::Checking);
    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(e) => {
            run.release();
            return Err(fail(&app, None, e.to_string()));
        }
    };

    // The check + download run in a spawned task so `cancel_app_update` can abort
    // them. The result comes back over a channel, and a CLOSED channel is the
    // cancellation signal: aborting drops the task, which drops the sender.
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    let emitter = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        let result = check_and_download(&emitter, updater).await;
        let _ = tx.send(result).await;
    });
    run.armed(task);

    let Some(result) = rx.recv().await else {
        // Aborted mid-download: the bytes are discarded and nothing on disk
        // changed. `cancel` already returned the slot to Idle — see
        // `AppUpdateRun::release`.
        emit(&app, None, AppUpdatePhase::Cancelled);
        return Ok(());
    };
    if !run.commit() {
        // A cancel landed in the gap between the download resolving and this
        // task being scheduled, so the buffered result arrived anyway. The
        // cancel was accepted; installing now would override it silently.
        let version = result.ok().map(|(update, _)| update.version);
        emit(&app, version.as_deref(), AppUpdatePhase::Cancelled);
        return Ok(());
    }

    // Committing BEFORE inspecting the result is deliberate, not an oversight: it
    // is what makes the `release` below unambiguously ours. Winning the commit
    // proves no cancel took the slot, so freeing it here cannot reset a
    // replacement run — the ownership rule `AppUpdateRun::release` documents.
    // Checking the result first would let a failed download release a slot a
    // cancel had already handed to someone else.
    let (update, bytes) = match result {
        Ok(downloaded) => downloaded,
        Err(e) => {
            run.release();
            return Err(fail(&app, e.version.as_deref(), e.message));
        }
    };
    let version = update.version.clone();

    emit(&app, Some(&version), AppUpdatePhase::Installing);
    // `Update::install` is synchronous — a signature-verified tar extract plus an
    // in-place `.app` bundle swap — so it goes to a blocking thread instead of
    // stalling an async runtime worker that the progress IPC also rides on.
    let installed = tauri::async_runtime::spawn_blocking(move || update.install(bytes)).await;
    let outcome = match installed {
        Ok(outcome) => outcome,
        Err(e) => {
            run.release();
            return Err(fail(&app, Some(&version), format!("install task: {e}")));
        }
    };
    if let Err(e) = outcome {
        run.release();
        return Err(fail(&app, Some(&version), e.to_string()));
    }

    // Window geometry is already current on disk: the debounced flush in `run()`
    // persists it ~600ms after the user stops moving/resizing, well within the
    // multi-second download+install above. We deliberately do NOT call
    // `save_window_state` here — this runs on a Tokio worker thread, off the main
    // thread, where that call can deadlock the UI (see persist_window_state_on_main
    // in lib.rs).
    //
    // New bytes are on disk. Restart the whole background service onto them BEFORE
    // relaunching the client — `app.restart()` never returns.
    emit(&app, Some(&version), AppUpdatePhase::RestartingServices);
    if let Err(e) = crate::desktop::restart_service() {
        eprintln!("[updater] background service restart failed: {e}");
    }
    // Relaunch the client onto its new bytes. Never returns.
    emit(&app, Some(&version), AppUpdatePhase::Relaunching);
    app.restart();
}

/// Abandon an in-flight app-update download. Only the check + download can be
/// cancelled — once the bundle swap has started the run has committed and this is
/// a no-op, which is the honest answer (there is no half-installed state to
/// return to). The page learns the outcome from the `cancelled` progress frame the
/// aborted run's command emits, not from this call.
#[tauri::command]
pub fn cancel_app_update(run: State<'_, AppUpdateRun>) {
    run.cancel();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed a whole download through the tracker the way the plugin's callback
    /// does, returning every frame it produced.
    fn run_download(chunk: u64, total: Option<u64>, size: u64) -> Vec<DownloadFrame> {
        let mut tracker = DownloadTracker::default();
        let mut frames = Vec::new();
        let mut sent = 0;
        while sent < size {
            let len = chunk.min(size - sent);
            sent += len;
            if let Some(frame) = tracker.chunk(len, total) {
                frames.push(frame);
            }
        }
        if let Some(frame) = tracker.finish() {
            frames.push(frame);
        }
        frames
    }

    // A ~100 MB bundle arrives as thousands of chunks. One IPC frame per chunk
    // would flood the bridge — the throttle is what keeps the count bounded.
    #[test]
    fn a_known_size_download_emits_about_one_frame_per_percent() {
        let size = 100 * 1024 * 1024;
        let frames = run_download(32 * 1024, Some(size), size);
        assert!(
            frames.len() <= 101,
            "expected at most one frame per percentage point, got {}",
            frames.len()
        );
        assert!(
            frames.len() >= 100,
            "expected ~100 frames, got {}",
            frames.len()
        );
    }

    #[test]
    fn the_first_chunk_always_produces_a_frame() {
        let mut tracker = DownloadTracker::default();
        let frame = tracker
            .chunk(1, Some(100 * 1024 * 1024))
            .expect("the first chunk must announce, however small");
        assert_eq!(frame.downloaded, 1);
    }

    // Without this the bar would stop just short of full and sit there while the
    // signature check runs.
    #[test]
    fn the_final_byte_count_is_always_announced() {
        let size = 10 * 1024 * 1024;
        for total in [Some(size), None] {
            let frames = run_download(32 * 1024, total, size);
            let last = frames.last().expect("a download announces at least once");
            assert_eq!(last.downloaded, size, "total={total:?}");
        }
    }

    // No `Content-Length` means no honest percentage, so the frames must carry
    // `total: None` and the byte step becomes the throttle instead.
    #[test]
    fn an_unknown_size_download_reports_bytes_without_inventing_a_total() {
        let size = 10 * 1024 * 1024;
        let frames = run_download(32 * 1024, None, size);
        assert!(
            frames.iter().all(|f| f.total.is_none()),
            "an unknown-size download must never report a total",
        );
        assert!(
            frames.len() <= 11,
            "expected ~one frame per MiB, got {}",
            frames.len()
        );
    }

    // A download too small to cross a single throttle step still gets the two
    // frames that matter: something as soon as bytes start moving, and the true
    // final count.
    #[test]
    fn a_download_smaller_than_one_step_announces_its_start_and_its_end() {
        let frames = run_download(1024, None, 4096);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].downloaded, 1024);
        assert_eq!(frames[1].downloaded, 4096);
    }

    #[test]
    fn a_zero_length_chunk_produces_no_frame() {
        let mut tracker = DownloadTracker::default();
        tracker.chunk(1024, Some(4096));
        assert!(tracker.chunk(0, Some(4096)).is_none());
    }

    #[test]
    fn only_one_run_can_claim_the_slot() {
        let run = AppUpdateRun::default();
        assert!(run.begin());
        assert!(
            !run.begin(),
            "a second run must be refused while one is live"
        );
        run.release();
        assert!(run.begin(), "the slot frees up once the run ends");
    }

    // The whole point of the state machine: a cancel that arrives after the bundle
    // swap started must not claim to have stopped anything.
    #[test]
    fn a_cancel_after_commit_is_refused() {
        let run = AppUpdateRun::default();
        run.begin();
        run.armed(tauri::async_runtime::spawn(async {}));
        assert!(run.commit());
        assert!(!run.cancel());
    }

    // The download-to-install boundary. The download resolves and its result is
    // buffered on the channel, but a cancel lands before the awaiting task is
    // scheduled: `cancel` accepts it and frees the slot, yet the result still
    // arrives. Committing anyway would install an update the user cancelled,
    // silently — nothing is on disk yet, so the commit must lose the race.
    #[test]
    fn a_cancel_that_lands_before_the_commit_wins() {
        let run = AppUpdateRun::default();
        run.begin();
        run.armed(tauri::async_runtime::spawn(async {}));
        assert!(
            run.cancel(),
            "the cancel is accepted while still downloading"
        );
        assert!(
            !run.commit(),
            "the buffered download result must not commit over an accepted cancel",
        );
    }

    #[test]
    fn a_commit_without_a_download_is_refused() {
        let run = AppUpdateRun::default();
        assert!(!run.commit(), "nothing to commit with no run in flight");
    }

    #[test]
    fn a_cancel_with_no_run_is_refused() {
        let run = AppUpdateRun::default();
        assert!(!run.cancel());
    }

    // The window between claiming the slot and handing over the task is tiny but
    // real; a cancel landing in it is honoured when the task arrives.
    #[test]
    fn a_cancel_during_startup_is_honoured_when_the_task_arrives() {
        let run = AppUpdateRun::default();
        run.begin();
        assert!(run.cancel());
        assert!(
            matches!(*run.lock(), Phase::CancelPending),
            "the cancel must be remembered until the task exists",
        );
    }
}
