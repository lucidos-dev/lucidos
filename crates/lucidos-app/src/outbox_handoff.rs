use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::Manager;

#[derive(Default)]
pub struct OutboxHandoffStore(Mutex<()>);

fn payload_path(app_data: &Path) -> PathBuf {
    app_data.join("config").join("outbox-handoff.json")
}

fn load(path: &Path) -> Result<Vec<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            serde_json::from_str(&text).map_err(|e| format!("read {}: {e}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

fn save(path: &Path, payloads: &[String]) -> Result<(), String> {
    use std::io::Write as _;
    let fail = |e: std::io::Error| format!("write {}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    let json = serde_json::to_string(payloads).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp).map_err(fail)?;
    file.write_all(json.as_bytes()).map_err(fail)?;
    file.sync_data().map_err(fail)?;
    std::fs::rename(&tmp, path).map_err(fail)
}

fn done_marker_path(app_data: &Path) -> PathBuf {
    app_data.join("config").join("outbox-handoff-done")
}

pub(crate) fn handoff_done(app_data: &Path) -> bool {
    done_marker_path(app_data).exists()
}

fn store(app_data: &Path, payload: Option<String>) -> Result<(), String> {
    if let Some(payload) = payload {
        append(&payload_path(app_data), payload)?;
    }
    let marker = done_marker_path(app_data);
    let fail = |e: std::io::Error| format!("write {}: {e}", marker.display());
    if let Some(dir) = marker.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    std::fs::write(&marker, b"").map_err(fail)
}

fn append(path: &Path, payload: String) -> Result<(), String> {
    let mut payloads = load(path)?;
    payloads.push(payload);
    save(path, &payloads)
}

fn acknowledge(path: &Path, imported: &[String]) -> Result<(), String> {
    let mut rest = load(path)?;
    for payload in imported {
        if let Some(at) = rest.iter().position(|kept| kept == payload) {
            rest.remove(at);
        }
    }
    if rest.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("delete {}: {e}", path.display()))
            }
            _ => Ok(()),
        };
    }
    save(path, &rest)
}

fn app_data(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))
}

fn locked(store: &OutboxHandoffStore) -> Result<std::sync::MutexGuard<'_, ()>, String> {
    store
        .0
        .lock()
        .map_err(|e| format!("outbox hand-off lock poisoned: {e}"))
}

#[tauri::command]
pub fn store_outbox_handoff(
    app: tauri::AppHandle,
    lock: tauri::State<'_, OutboxHandoffStore>,
    payload: Option<String>,
) -> Result<(), String> {
    let app_data = app_data(&app)?;
    let _guard = locked(&lock)?;
    store(&app_data, payload)
}

#[tauri::command]
pub fn take_outbox_handoff(
    app: tauri::AppHandle,
    store: tauri::State<'_, OutboxHandoffStore>,
) -> Result<Vec<String>, String> {
    let path = payload_path(&app_data(&app)?);
    let _guard = locked(&store)?;
    load(&path)
}

#[tauri::command]
pub fn acknowledge_outbox_handoff(
    app: tauri::AppHandle,
    store: tauri::State<'_, OutboxHandoffStore>,
    payloads: Vec<String>,
) -> Result<(), String> {
    let path = payload_path(&app_data(&app)?);
    let _guard = locked(&store)?;
    acknowledge(&path, &payloads)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_is_kept_until_acknowledged() {
        let dir = tempfile::tempdir().unwrap();
        let path = payload_path(dir.path());
        assert_eq!(load(&path).unwrap(), Vec::<String>::new());

        append(&path, "first".into()).unwrap();
        append(&path, "second".into()).unwrap();
        assert_eq!(load(&path).unwrap(), ["first", "second"]);

        acknowledge(&path, &["first".into()]).unwrap();
        assert_eq!(load(&path).unwrap(), ["second"], "one stored since stays");
        acknowledge(&path, &["second".into()]).unwrap();
        assert!(!path.exists());
        acknowledge(&path, &[]).unwrap();
    }

    #[test]
    fn two_windows_acknowledging_the_same_take_keep_a_later_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = payload_path(dir.path());
        append(&path, "taken".into()).unwrap();
        let take = load(&path).unwrap();
        append(&path, "stored since".into()).unwrap();
        acknowledge(&path, &take).unwrap();
        acknowledge(&path, &take).unwrap();
        assert_eq!(load(&path).unwrap(), ["stored since"]);
    }

    #[test]
    fn a_corrupt_file_is_an_error_never_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = payload_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        assert!(load(&path).is_err());
        assert!(acknowledge(&path, &["x".into()]).is_err());
        assert!(path.exists(), "nothing unreadable is deleted");
    }

    #[test]
    fn every_store_marks_the_hand_off_done_even_an_empty_one() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!handoff_done(dir.path()));
        store(dir.path(), None).unwrap();
        assert!(handoff_done(dir.path()));
        assert_eq!(
            load(&payload_path(dir.path())).unwrap(),
            Vec::<String>::new()
        );

        let other = tempfile::tempdir().unwrap();
        store(other.path(), Some("unsent".into())).unwrap();
        assert!(handoff_done(other.path()));
        assert_eq!(load(&payload_path(other.path())).unwrap(), ["unsent"]);
    }

    #[test]
    fn a_failed_store_leaves_the_hand_off_to_run_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = payload_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        assert!(store(dir.path(), Some("unsent".into())).is_err());
        assert!(!handoff_done(dir.path()));
    }

    #[cfg(unix)]
    #[test]
    fn the_payload_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = payload_path(dir.path());
        append(&path, "unsent".into()).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
