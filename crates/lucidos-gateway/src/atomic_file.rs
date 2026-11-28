use crate::BoxError;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub fn write(path: &Path, bytes: &[u8]) -> Result<(), BoxError> {
    write_with_mode(path, bytes, 0o666)
}

pub fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<(), BoxError> {
    write_with_mode(path, bytes, 0o600)
}

fn write_with_mode(path: &Path, bytes: &[u8], mode: u32) -> Result<(), BoxError> {
    use std::os::unix::fs::OpenOptionsExt;

    static SAVES: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{} has no file name", path.display()))?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let tmp = path.with_file_name(format!(
        ".{name}.{}.{nanos}.{}.tmp",
        std::process::id(),
        SAVES.fetch_add(1, Ordering::Relaxed)
    ));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)
        .and_then(|mut f| f.write_all(bytes).and_then(|()| f.sync_all()))
        .and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("save {}: {e}", path.display()).into());
    }
    Ok(())
}
