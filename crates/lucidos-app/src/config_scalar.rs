use tauri::Manager;

pub(crate) fn path(app: &tauri::AppHandle, file: &str) -> Option<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("config").join(file))
}

pub(crate) fn read(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|raw| raw.trim().to_string())
}

pub(crate) fn write_if_changed(app: &tauri::AppHandle, file: &str, value: &str, what: &str) {
    let Some(path) = path(app, file) else {
        return;
    };
    if read(&path).as_deref() == Some(value) {
        return;
    }
    let Some(dir) = path.parent() else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&path, value)) {
        eprintln!("[Tauri] Failed to remember the {what}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_file_reads_as_absent_rather_than_as_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(&dir.path().join("nothing-here")), None);
    }

    #[test]
    fn a_stored_value_reads_back_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("value");
        std::fs::write(&file, "  #15549e\n").unwrap();
        assert_eq!(read(&file).as_deref(), Some("#15549e"));
    }
}
