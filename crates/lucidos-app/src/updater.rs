//! Auto-update wiring (`tauri-plugin-updater`).
//!
//! A packaged build surfaces updates INSIDE the workspace UI — not a native
//! launch dialog, not the picker. Most users have a single workspace and auto-open
//! straight into it, so the web app (running in the packaged Tauri client) polls
//! [`check_app_update`] on startup + on an interval and shows an in-app
//! "Update & restart" toast. The toast's action calls
//! [`install_app_update_and_restart`], which installs the new signed bundle and
//! restarts the WHOLE stack onto the new version — the launchd background service
//! (gateway + per-workspace engines + embedded Postgres) AND the GUI client —
//! rather than only relaunching the window.
//!
//! Distribution model: the `.dmg` is for first install; the updater ships the
//! `.app.tar.gz` + its `.sig` and `latest.json` (all on the same GitHub Release).
//! Update artifacts are signed with the Tauri updater key (`plugins.updater.pubkey`
//! in config; `TAURI_SIGNING_PRIVATE_KEY` at build time) — separate from Apple
//! notarization, which gates the first-install `.dmg`.
//!
//! No-op in development (the updater endpoint isn't reachable and there's no
//! launchd service to restart).

use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

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
pub async fn install_app_update_and_restart(app: AppHandle) -> Result<(), String> {
    if tauri::is_dev() {
        return Err("Updates are only available in a packaged build".to_string());
    }
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Err("No update available".to_string());
    };
    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    // Window geometry is already current on disk: the debounced flush in `run()`
    // persists it ~600ms after the user stops moving/resizing, well within the
    // multi-second download+install above. We deliberately do NOT call
    // `save_window_state` here — this runs on a Tokio worker thread, off the main
    // thread, where that call can deadlock the UI (see persist_window_state_on_main
    // in lib.rs).
    //
    // New bytes are on disk. Restart the whole background service onto them BEFORE
    // relaunching the client — `app.restart()` never returns.
    if let Err(e) = crate::desktop::restart_service() {
        eprintln!("[updater] background service restart failed: {e}");
    }
    // Relaunch the client onto its new bytes. Never returns.
    app.restart();
}
