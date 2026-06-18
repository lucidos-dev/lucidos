//! Auto-update wiring (`tauri-plugin-updater`).
//!
//! On launch (packaged only) the app checks its update endpoint — a
//! `latest.json` manifest served from GitHub Releases (configured in
//! `tauri.conf.json` → `plugins.updater.endpoints`). If a newer signed build is
//! available it prompts *"Update available — restart now?"*; on confirm it
//! downloads, installs, and relaunches into the new version.
//!
//! Distribution model: the `.dmg` is for first install; the updater ships the
//! `.app.tar.gz` + its `.sig` and `latest.json` (all on the same GitHub Release).
//! Update artifacts are signed with the Tauri updater key (`plugins.updater.pubkey`
//! in config; `TAURI_SIGNING_PRIVATE_KEY` at build time) — separate from Apple
//! notarization, which gates the first-install `.dmg`.
//!
//! No-op in development.

use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Spawn a background check for updates. Safe to call from `setup`.
pub fn check_on_startup(app: &AppHandle) {
    if tauri::is_dev() {
        return;
    }
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = check(&handle).await {
            // Best-effort: a transient network / endpoint error must never block
            // app startup. The next launch re-checks.
            eprintln!("[updater] check failed: {e}");
        }
    });
}

async fn check(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let Some(update) = app.updater()?.check().await? else {
        return Ok(()); // already up to date
    };

    let version = update.version.clone();
    let app_for_confirm = app.clone();
    app.dialog()
        .message(format!(
            "Lucidos {version} is available. Install it and restart now?"
        ))
        .title("Update available")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Restart now".to_string(),
            "Later".to_string(),
        ))
        .show(move |confirmed| {
            if confirmed {
                let app = app_for_confirm.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = install_and_restart(&app, update).await {
                        eprintln!("[updater] install failed: {e}");
                    }
                });
            }
        });

    Ok(())
}

async fn install_and_restart(
    app: &AppHandle,
    update: Update,
) -> Result<(), Box<dyn std::error::Error>> {
    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await?;
    // Relaunch into the freshly-installed version. `restart` never returns.
    app.restart();
}
