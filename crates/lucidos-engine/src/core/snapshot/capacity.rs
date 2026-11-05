//! Conservative file-volume reservation before recovery capture and mutation.
use super::{store::check_floor, BoxError};
use std::{collections::BTreeMap, os::unix::fs::MetadataExt, path::Path};

/// Count a volume only once when the external store, workspace and linked data
/// share it. Reserve enough for both forward and rollback staging; renaming
/// retained originals into trash does not require another copy of their bytes.
pub fn preflight(
    store: &Path,
    workspace: &Path,
    incoming: u64,
    current: u64,
) -> Result<(), BoxError> {
    preflight_database(store, workspace, incoming, current, None)
}

pub(super) fn preflight_database(
    store: &Path,
    workspace: &Path,
    incoming: u64,
    current: u64,
    database: Option<(&super::database_capacity::DatabaseCapacity, &str)>,
) -> Result<(), BoxError> {
    let staging = incoming.checked_add(current).ok_or("snapshot_disk_floor")?;
    let mut volumes = BTreeMap::<u64, (std::path::PathBuf, u64)>::new();
    let mut reserve = |path: &Path, bytes: u64| -> Result<(), BoxError> {
        let device = std::fs::metadata(path)?.dev();
        let entry = volumes.entry(device).or_insert((path.to_owned(), 0));
        entry.1 = entry.1.checked_add(bytes).ok_or("snapshot_disk_floor")?;
        Ok(())
    };
    // Unpacked archive staging plus a conservative uncompressed recovery copy.
    reserve(
        store,
        staging.checked_add(current).ok_or("snapshot_disk_floor")?,
    )?;
    reserve(workspace, staging)?;
    let data = workspace.join("data");
    if data.try_exists()? && std::fs::metadata(&data)?.dev() != std::fs::metadata(workspace)?.dev()
    {
        reserve(&data, staging)?;
    }
    if let Some((database, url)) = database {
        reserve(store, database.recovery_bytes)?;
        if let Some(paths) = database.native_paths()? {
            // One full replay reservation per distinct database volume. WAL
            // and tablespaces may reside on different mounts.
            let mut devices = std::collections::BTreeSet::new();
            for path in paths {
                if devices.insert(std::fs::metadata(path)?.dev()) {
                    reserve(path, database.reservation)?;
                }
            }
        } else {
            let host_total = volumes.values().try_fold(0u64, |sum, (_, bytes)| {
                sum.checked_add(*bytes).ok_or("snapshot_disk_floor")
            })?;
            database.check_docker(url, host_total)?;
            // A Docker VM image can grow on any host volume. Without an
            // authoritative backing-file mapping, conservatively charge replay
            // to every involved host volume as well as the container volumes.
            for (_, bytes) in volumes.values_mut() {
                *bytes = bytes
                    .checked_add(database.reservation)
                    .ok_or("snapshot_disk_floor")?;
            }
        }
    }
    for (_, (path, estimate)) in volumes {
        check_floor(
            fs2::available_space(&path)?,
            fs2::total_space(&path)?,
            estimate,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overflow_and_insufficient_capacity_refuse_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        assert!(preflight(temp.path(), temp.path(), u64::MAX, 1).is_err());
        assert!(preflight(temp.path(), temp.path(), u64::MAX / 4, 0).is_err());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}
