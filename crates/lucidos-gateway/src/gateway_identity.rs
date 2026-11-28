use crate::BoxError;
use std::io::{Read, Write};
use std::path::Path;

pub fn load_or_create(app_data: &Path) -> Result<String, BoxError> {
    let path = app_data.join("gateway-identity");
    match std::fs::read_to_string(&path) {
        Ok(value) => return validate(value),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("read gateway identity: {e}").into()),
    }
    std::fs::create_dir_all(app_data)?;
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let identity: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let mut tmp = tempfile::NamedTempFile::new_in(app_data)?;
    tmp.write_all(identity.as_bytes())?;
    tmp.as_file().sync_all()?;
    match tmp.persist_noclobber(&path) {
        Ok(_) => {}
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("save gateway identity: {e}").into()),
    }
    std::fs::File::open(app_data)?.sync_all()?;
    validate(std::fs::read_to_string(path)?)
}

fn validate(value: String) -> Result<String, BoxError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(value)
    } else {
        Err("gateway identity is corrupt; restore gateway-identity from backup".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_survives_reload_and_is_scoped_to_the_data_directory() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let first = load_or_create(a.path()).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(first, load_or_create(a.path()).unwrap());
        assert_ne!(first, load_or_create(b.path()).unwrap());
    }

    #[test]
    fn concurrent_initialization_agrees_on_one_complete_identity() {
        let dir = tempfile::tempdir().unwrap();
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        load_or_create(dir.path()).unwrap()
                    })
                })
                .collect();
            let ids: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            assert!(ids.iter().all(|id| id == &ids[0]));
            assert_eq!(ids[0], load_or_create(dir.path()).unwrap());
        });
    }

    #[test]
    fn corrupt_or_unreadable_identity_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gateway-identity");
        for invalid in ["", "partial", &"g".repeat(64), &"a".repeat(63)] {
            std::fs::write(&path, invalid).unwrap();
            assert!(load_or_create(dir.path()).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(load_or_create(dir.path()).is_err());
        assert!(path.is_dir());
    }
}

#[cfg(all(test, unix))]
#[test]
fn identity_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    load_or_create(dir.path()).unwrap();
    let mode = std::fs::metadata(dir.path().join("gateway-identity"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}
