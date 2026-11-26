use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::Manager;

#[derive(Default)]
pub struct DeviceIdStore(pub Mutex<()>);

pub type DeviceIdMap = BTreeMap<String, String>;

pub fn device_ids_by_workspace(app_data: &Path) -> DeviceIdMap {
    load_map(&store_path(app_data))
}

pub fn resolve_device_id(map: &mut DeviceIdMap, slug: &str, candidate: &str) -> (String, bool) {
    if let Some(existing) = map.get(slug) {
        return (existing.clone(), false);
    }
    map.insert(slug.to_string(), candidate.to_string());
    (candidate.to_string(), true)
}

pub fn replace_device_id(map: &mut DeviceIdMap, slug: &str, id: &str) -> (Option<String>, bool) {
    let previous = map.insert(slug.to_string(), id.to_string());
    let changed = previous.as_deref() != Some(id);
    (previous.filter(|p| p != id), changed)
}

fn store_path(app_data: &Path) -> PathBuf {
    app_data.join("config").join("device-ids.json")
}

fn load_map(path: &Path) -> DeviceIdMap {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_map(path: &Path, map: &DeviceIdMap) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(map).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

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

#[tauri::command]
pub fn previous_device_id(
    app: tauri::AppHandle,
    store: tauri::State<'_, DeviceIdStore>,
    workspace: String,
) -> Result<Option<String>, String> {
    let workspace = workspace.trim();
    if workspace.is_empty() {
        return Err("workspace must not be empty".into());
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
    Ok(load_map(&path).get(workspace).cloned())
}

#[tauri::command]
pub fn remember_device_id(
    app: tauri::AppHandle,
    store: tauri::State<'_, DeviceIdStore>,
    workspace: String,
    id: String,
) -> Result<(), String> {
    let workspace = workspace.trim();
    let id = id.trim();
    if workspace.is_empty() {
        return Err("workspace must not be empty".into());
    }
    if id.is_empty() {
        return Err("id must not be empty".into());
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
    let (_, changed) = replace_device_id(&mut map, workspace, id);
    if changed {
        save_map(&path, &map).map_err(|e| format!("persist device-ids.json: {e}"))?;
    }
    Ok(())
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
        let mut map = DeviceIdMap::new();
        map.insert("alpha".into(), "stored".into());
        let (id, changed) = resolve_device_id(&mut map, "alpha", "fresh-candidate");
        assert_eq!(id, "stored");
        assert!(!changed);
        assert_eq!(map.get("alpha").map(String::as_str), Some("stored"));
    }

    #[test]
    fn slugs_are_isolated() {
        let mut map = DeviceIdMap::new();
        resolve_device_id(&mut map, "alpha", "id-a");
        resolve_device_id(&mut map, "beta", "id-b");
        assert_eq!(map.get("alpha").map(String::as_str), Some("id-a"));
        assert_eq!(map.get("beta").map(String::as_str), Some("id-b"));
    }

    #[test]
    fn replace_reports_the_id_it_displaced() {
        let mut map = DeviceIdMap::new();
        map.insert("alpha".into(), "before-reinstall".into());
        let (previous, changed) = replace_device_id(&mut map, "alpha", "after-reinstall");
        assert_eq!(previous.as_deref(), Some("before-reinstall"));
        assert!(changed);
        assert_eq!(
            map.get("alpha").map(String::as_str),
            Some("after-reinstall")
        );
    }

    #[test]
    fn replace_reports_nothing_for_a_first_sighting_or_an_unchanged_id() {
        let mut map = DeviceIdMap::new();
        assert_eq!(replace_device_id(&mut map, "alpha", "id-a").0, None);
        assert_eq!(replace_device_id(&mut map, "alpha", "id-a").0, None);
        assert_eq!(map.get("alpha").map(String::as_str), Some("id-a"));
    }

    #[test]
    fn replace_writes_nothing_when_the_id_is_already_what_we_store() {
        let mut map = DeviceIdMap::new();
        assert!(
            replace_device_id(&mut map, "alpha", "id-a").1,
            "a first sighting is a change"
        );
        assert!(!replace_device_id(&mut map, "alpha", "id-a").1);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = store_path(dir.path());
        let mut map = DeviceIdMap::new();
        resolve_device_id(&mut map, "alpha", "id-a");
        save_map(&path, &map).unwrap();
        let loaded = load_map(&path);
        assert_eq!(loaded.get("alpha").map(String::as_str), Some("id-a"));
    }

    #[test]
    fn load_missing_or_corrupt_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = store_path(dir.path());
        assert!(load_map(&path).is_empty());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(load_map(&path).is_empty());
    }
}
