//! Durable, per-workspace device identity for the packaged desktop app.
//!
//! The frontend's device id used to live only in the WKWebView's `localStorage`
//! (`ws:<slug>:lucidos-device-id`). WebKit's storage bucket is keyed to the app's
//! code-signing identity / bundle version, so a new DMG install hands the webview a
//! fresh, empty bucket even though `~/Library/Application Support/<id>/` survives —
//! the frontend then mints a new UUID and the user shows up as a brand-new device
//! after every update (the same class of OS-state-keyed-by-code-identity problem the
//! engine binary hits with TCC grants; see `.claude/rules/scripts.md`).
//!
//! We persist the id natively as a plain JSON map under the App Support data dir —
//! path-based storage, exactly like `config/engine-port` and `config/workspaces.json`,
//! both of which already survive a reinstall — and the frontend reconciles it back
//! into `localStorage` at startup so the synchronous `getDeviceId()` keeps working
//! unchanged. A delete-data uninstall removes the whole data dir (`desktop.rs`
//! `support_data_paths`), so the id is forgotten exactly when the user asks for it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::Manager;

/// Serializes the read-modify-write so two windows of the same workspace booting
/// concurrently can't both observe "absent" and persist divergent ids. Managed as
/// Tauri state.
#[derive(Default)]
pub struct DeviceIdStore(pub Mutex<()>);

/// The on-disk store: workspace slug → device id. `BTreeMap` for stable key order
/// (deterministic file content, friendlier diffs).
type DeviceIdMap = BTreeMap<String, String>;

/// Get-or-create the id for `slug` in `map`. Returns the resolved id and whether the
/// map changed (i.e. the candidate was inserted). Pure — the unit of behavior, with
/// no filesystem or Tauri coupling. An existing slug keeps its stored id and IGNORES
/// the candidate (the durability guarantee that fixes reinstall churn); a new slug
/// adopts the candidate.
pub fn resolve_device_id(map: &mut DeviceIdMap, slug: &str, candidate: &str) -> (String, bool) {
    if let Some(existing) = map.get(slug) {
        return (existing.clone(), false);
    }
    map.insert(slug.to_string(), candidate.to_string());
    (candidate.to_string(), true)
}

/// `<app_data>/config/device-ids.json` — beside `engine-port` and `workspaces.json`.
fn store_path(app_data: &Path) -> PathBuf {
    app_data.join("config").join("device-ids.json")
}

/// Load the map, treating a missing OR corrupt file as empty: a parse error must
/// never wedge boot — the id is simply (re)created from the candidate.
fn load_map(path: &Path) -> DeviceIdMap {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist the map. Creates `config/` if absent (like `resolve_engine_port`), and
/// writes to a temp sibling then renames so a crash mid-write can't truncate the
/// existing map.
fn save_map(path: &Path, map: &DeviceIdMap) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(map).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

/// Durable get-or-create of this device's id for `workspace` (its gateway slug).
/// Returns the stored id when one already exists for the slug, else stores and
/// returns `candidate` — a UUID the frontend minted, OR the value it already had in
/// `localStorage` (so an existing install adopts its current id with no churn). The
/// frontend seeds the result back into `localStorage`, keeping the synchronous
/// `getDeviceId()` unchanged. Desktop-only: browser/PWA never call this.
#[tauri::command]
pub fn get_or_create_device_id(
    app: tauri::AppHandle,
    store: tauri::State<'_, DeviceIdStore>,
    workspace: String,
    candidate: String,
) -> Result<String, String> {
    let workspace = workspace.trim();
    let candidate = candidate.trim();
    if workspace.is_empty() {
        return Err("workspace must not be empty".into());
    }
    if candidate.is_empty() {
        return Err("candidate must not be empty".into());
    }
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))?;
    let path = store_path(&app_data);

    let _guard = store
        .0
        .lock()
        .map_err(|e| format!("device-id store lock poisoned: {e}"))?;
    let mut map = load_map(&path);
    let (id, changed) = resolve_device_id(&mut map, workspace, candidate);
    if changed {
        save_map(&path, &map).map_err(|e| format!("persist device-ids.json: {e}"))?;
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_slug_inserts_and_returns_candidate() {
        let mut map = DeviceIdMap::new();
        let (id, changed) = resolve_device_id(&mut map, "alpha", "cand-1");
        assert_eq!(id, "cand-1");
        assert!(changed);
        assert_eq!(map.get("alpha").map(String::as_str), Some("cand-1"));
    }

    #[test]
    fn present_slug_returns_stored_ignoring_candidate() {
        // The durability guarantee: a reinstall passes a fresh candidate, but the
        // stored id wins, so the device is recognized as the same one.
        let mut map = DeviceIdMap::new();
        map.insert("alpha".into(), "stored".into());
        let (id, changed) = resolve_device_id(&mut map, "alpha", "fresh-candidate");
        assert_eq!(id, "stored");
        assert!(!changed);
        assert_eq!(map.get("alpha").map(String::as_str), Some("stored"));
    }

    #[test]
    fn slugs_are_isolated() {
        // Per-workspace identity: each slug maps to its own id.
        let mut map = DeviceIdMap::new();
        resolve_device_id(&mut map, "alpha", "id-a");
        resolve_device_id(&mut map, "beta", "id-b");
        assert_eq!(map.get("alpha").map(String::as_str), Some("id-a"));
        assert_eq!(map.get("beta").map(String::as_str), Some("id-b"));
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = std::env::temp_dir().join(format!("lucidos-devid-rt-{}", std::process::id()));
        let path = store_path(&dir);
        let mut map = DeviceIdMap::new();
        resolve_device_id(&mut map, "alpha", "id-a");
        save_map(&path, &map).unwrap();
        let loaded = load_map(&path);
        assert_eq!(loaded.get("alpha").map(String::as_str), Some("id-a"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_or_corrupt_is_empty() {
        let dir = std::env::temp_dir().join(format!("lucidos-devid-bad-{}", std::process::id()));
        let path = store_path(&dir);
        // Missing → empty.
        assert!(load_map(&path).is_empty());
        // Corrupt → empty (never wedges boot).
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(load_map(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
