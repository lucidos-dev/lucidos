//! Fail closed before normal boot can mutate an interrupted restore.
use super::{store::Store, BoxError};
use std::{io::ErrorKind, path::Path};

/// No keys, database connection, directories or permission changes are needed.
/// Recheck after taking the workspace owner lock: an offline worker may have
/// changed the journal while normal startup waited for that lock.
pub fn ensure_normal_boot(database_url: &str) -> Result<(), BoxError> {
    check_boot(database_url, false)
}

/// Admit only the restricted verifier at FilesRestored. This is not permission
/// to construct an engine: the owner-locked recheck must obtain RestoreBoot.
pub fn ensure_boot_entry(database_url: &str) -> Result<(), BoxError> {
    check_boot(database_url, true)
}

fn check_boot(database_url: &str, restricted: bool) -> Result<(), BoxError> {
    let database = crate::core::secret_store::boot::database_name(database_url)
        .map_err(|_| "snapshot_boot_binding_unavailable")?;
    let identity =
        super::identity::workspace_id(crate::api::base_path::workspace_id().as_deref(), &database)?;
    let root = Store::database_root(&database)?;
    check_at(&root, identity, restricted).map_err(|_| "snapshot_restore_blocks_normal_boot".into())
}

#[cfg(test)]
fn ensure_at(root: &Path, identity: String) -> Result<(), BoxError> {
    check_at(root, identity, false)
}

fn check_at(root: &Path, identity: String, restricted: bool) -> Result<(), BoxError> {
    match std::fs::symlink_metadata(root) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.is_dir() => {
            return Err("snapshot store is not a directory".into())
        }
        Ok(_) => {}
    }
    // An existing store with an absent, redirected or unreadable journal is
    // unknown state, never evidence that a restore finished.
    if !std::fs::symlink_metadata(root.join("journal"))?.is_dir() {
        return Err("snapshot journal is not a directory".into());
    }
    let store = Store::existing_at(root.to_owned(), identity)?;
    if store.journals()?.iter().any(|journal| {
        journal.phase().blocks_engine()
            && !(restricted && journal.phase() == super::journal::Phase::FilesRestored)
    }) {
        return Err("snapshot restore incomplete".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::snapshot::{
        handle::Handle,
        journal::{Journal, Phase},
    };
    use crate::engine::thread_events::MessageOrigin;

    fn journal(store: &Store) -> Journal {
        Journal::new(
            store.workspace_id.clone(),
            Handle::generate(),
            MessageOrigin::Device {
                device_id: "test-device".into(),
            },
            None,
        )
    }

    #[test]
    fn absent_store_is_allowed_without_creating_anything() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("absent");
        ensure_at(&root, "demo".into()).unwrap();
        assert!(!root.exists());
    }

    #[test]
    fn interrupted_mutation_and_attention_block_until_terminal_verification() {
        use Phase::*;
        for ending in [Completed, RolledBack, NeedsAttention] {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::at(dir.path().join("store"), "demo").unwrap();
            let mut record = journal(&store);
            store.save_journal(&record).unwrap();
            ensure_at(store.root(), "demo".into()).unwrap();
            for phase in [RecoveryCapture, Staging, DbRestore, FileSwap, FilesRestored] {
                if phase == Staging {
                    record.recovery = Some(Handle::generate());
                }
                record.advance(phase).unwrap();
                store.save_journal(&record).unwrap();
                assert_eq!(
                    ensure_at(store.root(), "demo".into()).is_err(),
                    phase.blocks_engine()
                );
                assert_eq!(
                    check_at(store.root(), "demo".into(), true).is_err(),
                    phase.blocks_engine() && phase != FilesRestored
                );
            }
            if ending != Completed {
                record.advance(RollingBack).unwrap();
                store.save_journal(&record).unwrap();
                assert!(ensure_at(store.root(), "demo".into()).is_err());
            }
            record.advance(ending).unwrap();
            store.save_journal(&record).unwrap();
            assert_eq!(
                ensure_at(store.root(), "demo".into()).is_err(),
                ending == NeedsAttention
            );
        }
    }

    #[test]
    fn missing_corrupt_and_wrong_identity_journals_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("store");
        std::fs::create_dir(&root).unwrap();
        assert!(ensure_at(&root, "demo".into()).is_err());
        let store = Store::at(root.clone(), "demo").unwrap();
        let record = journal(&store);
        store.save_journal(&record).unwrap();
        assert!(ensure_at(&root, "another".into()).is_err());
        std::fs::write(
            root.join("journal")
                .join(format!("{}.json", record.restore_id)),
            b"broken",
        )
        .unwrap();
        assert!(ensure_at(&root, "demo".into()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn redirected_store_or_journal_is_not_absent_or_safe() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("store");
        symlink(dir.path().join("missing"), &root).unwrap();
        assert!(ensure_at(&root, "demo".into()).is_err());
        std::fs::remove_file(&root).unwrap();
        std::fs::create_dir(&root).unwrap();
        let journals = dir.path().join("journals");
        std::fs::create_dir(&journals).unwrap();
        symlink(journals, root.join("journal")).unwrap();
        assert!(ensure_at(&root, "demo".into()).is_err());
    }

    #[test]
    fn production_checks_precede_workspace_mutation() {
        let main = include_str!("../../main.rs");
        let run = &main[main.find("async fn run()").unwrap()..];
        let guard = run.find("snapshot::boot_guard::ensure_boot_entry").unwrap();
        assert!(guard < run.find("ensure_storage_root(&workspace_path)").unwrap());
        let owned = run.find("own_workspace(&workspace_path)").unwrap();
        let recheck = run[owned..]
            .find("snapshot::boot_guard::ensure_normal_boot")
            .unwrap()
            + owned;
        assert!(recheck < run.find("pin_cli_for_workspace(&workspace_path").unwrap());
        let acquire = run.find("first_boot::RestoreBoot::acquire").unwrap();
        let verify = run.find("guard.verify(pool.clone())").unwrap();
        assert!(owned < acquire && acquire < verify);
        assert!(
            verify
                < run
                    .find("proxy_migration::migrate_apis_json_if_needed")
                    .unwrap()
        );
        let construction = include_str!("../../engine/engine_impl/construction.rs");
        let new = &construction[construction.find("pub async fn new(").unwrap()..];
        assert!(
            new.find("snapshot::boot_guard::ensure_normal_boot")
                .unwrap()
                < new.find("ArtifactManager::new(").unwrap()
        );
        let restored =
            &construction[construction.find("pub async fn new_with_restore(").unwrap()..];
        assert!(
            restored.find("restore_boot.validate_binding").unwrap()
                < restored.find("ArtifactManager::new(").unwrap()
        );
        let settlement = restored
            .find("restore_boot::settle_before_dispatch")
            .unwrap();
        let sequence = include_str!("../../engine/restore_boot.rs");
        assert!(
            sequence.find("restore_boot.validate_boundary").unwrap()
                < sequence.find("settle_restored_threads").unwrap()
        );
        assert!(
            sequence.find("settle_restored_checks").unwrap()
                < sequence.find("announce_restored").unwrap()
        );
        let completion = restored
            .find("restore_boot::complete_before_dispatch")
            .unwrap();
        assert!(settlement < completion);
        assert!(completion < restored.find("Session::recover_at_boot").unwrap());
    }
}
