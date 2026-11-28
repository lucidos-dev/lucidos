//! Carry a workspace delete forward through its Trash journal.
//!
//! A fresh delete and "Finish delete" share these steps. Finish delete resumes
//! a journal an interrupted delete left behind, so the user can complete the
//! delete rather than only roll it back. It never drops a database that the
//! journal holds no committed recovery copy of.

use super::{stop_engine_process, GatewayState};
use crate::cleanup_audit::{Actor, Audit};
use crate::error::ApiError;
use crate::registry::Workspace;
use crate::trash::{Entry, Phase};
use crate::{postgres, stack, BoxError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The database steps of a delete, separate so tests can observe their order.
pub(super) trait DeletionDatabase {
    async fn exists(&self, ws: &Workspace) -> Result<bool, BoxError>;
    async fn dump(&self, ws: &Workspace, path: PathBuf) -> Result<(), BoxError>;
    async fn mark_original(&self, ws: &Workspace, operation: String) -> Result<(), BoxError>;
    async fn drop_database(&self, ws: &Workspace) -> Result<(), BoxError>;
}

pub(super) struct PostgresDeletion<'a>(pub(super) &'a GatewayState);

impl DeletionDatabase for PostgresDeletion<'_> {
    async fn exists(&self, ws: &Workspace) -> Result<bool, BoxError> {
        let backend = self.0.inner.pg_backend.clone();
        let app_data = self.0.app_data().to_owned();
        let database = postgres::database_name(&ws.id)?;
        tokio::task::spawn_blocking(move || {
            postgres::cleanup_database_exists(&backend, &app_data, &database)
        })
        .await?
    }

    async fn dump(&self, ws: &Workspace, path: PathBuf) -> Result<(), BoxError> {
        let backend = self.0.inner.pg_backend.clone();
        let app_data = self.0.app_data().to_owned();
        let id = ws.id.clone();
        tokio::task::spawn_blocking(move || postgres::cleanup_dump(&backend, &app_data, &id, &path))
            .await?
    }

    async fn mark_original(&self, ws: &Workspace, operation: String) -> Result<(), BoxError> {
        let backend = self.0.inner.pg_backend.clone();
        let app_data = self.0.app_data().to_owned();
        let id = ws.id.clone();
        tokio::task::spawn_blocking(move || {
            postgres::cleanup_mark_original(&backend, &app_data, &id, &operation)
        })
        .await?
    }

    async fn drop_database(&self, ws: &Workspace) -> Result<(), BoxError> {
        self.0.teardown_postgres(ws).await
    }
}

impl GatewayState {
    /// No other registration, here or on a peer gateway, may own this
    /// workspace's folder or database.
    pub(super) fn require_unowned_by_others(
        &self,
        ws: &Workspace,
        dir: &Path,
        peers: &[PathBuf],
    ) -> Result<(), BoxError> {
        let others = self
            .inner
            .registry
            .lock()
            .unwrap()
            .workspaces
            .iter()
            .filter(|other| other.id != ws.id)
            .cloned()
            .collect::<Vec<_>>();
        crate::cleanup::registered_resources(&others, self.app_data(), peers)?
            .require_unowned(dir, &postgres::database_name(&ws.id)?)
    }

    /// Stop the workspace's engine and hold its owner lock for the delete.
    pub(super) async fn stop_for_deletion(
        &self,
        id: &str,
        dir: &Path,
    ) -> Result<lucidos_owner_lock::OwnerLock, BoxError> {
        // Take the stack out from under the map lock first, so the blocking
        // process signal does not pin the whole map.
        let removed = self.inner.stacks.lock().await.remove(id);
        if let Some(stack) = removed {
            let mut s = stack.lock().await;
            stop_engine_process(&mut s);
        }
        stack::reclaim_stale_engine(dir);
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(owner) = lucidos_owner_lock::OwnerLock::try_acquire(
                &lucidos_owner_lock::engine::lock_path(dir),
            )? {
                return Ok(owner);
            }
            if Instant::now() >= deadline {
                return Err("workspace stop timed out".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Advance a journal from any active phase to `completed`. The caller
    /// holds the owner lock, and any recovery copy is already committed.
    pub(super) async fn advance_deletion(
        &self,
        ws: &Workspace,
        entry: &mut Entry,
        root: &Path,
        database: &impl DeletionDatabase,
    ) -> Result<(), BoxError> {
        let id = ws.id.as_str();
        if entry.phase == Phase::Prepared {
            let operation = entry
                .deletion_operation
                .clone()
                .ok_or("missing deletion identity")?;
            database.mark_original(ws, operation).await?;
            if entry.original_dir.try_exists()? {
                entry.move_folder(root)?;
            } else {
                entry.reconcile_moved(root)?;
            }
        }
        if entry.phase == Phase::Moved {
            self.trash_snapshots(id, root)?;
            // A failed drop leaves both registry and journal for explicit recovery.
            database.drop_database(ws).await?;
            entry.phase = Phase::DatabaseDropped;
            entry.save(root)?;
        }
        if entry.phase != Phase::DatabaseDropped {
            return Err("cleanup journal requires explicit Trash recovery".into());
        }
        let prune_entry = entry.clone();
        let prune_root = root.to_owned();
        tokio::task::spawn_blocking(move || prune_entry.prune_worktrees(&prune_root)).await??;
        {
            let mut reg = self.inner.registry.lock().unwrap();
            if reg.contains(id) {
                let mut updated = reg.clone();
                updated.remove(id)?;
                updated.save(&self.inner.registry_path)?;
                *reg = updated;
            }
        }
        self.forget_workspace_order(id);
        entry.phase = Phase::Completed;
        entry.save(root)?;
        self.clear_runtime_state(id);
        self.clear_stop(id);
        Ok(())
    }

    /// Complete the delete an interrupted journal began, instead of restoring it.
    pub(crate) async fn finish_trash_delete(
        &self,
        trash_id: &str,
        device_id: &str,
    ) -> Result<(), ApiError> {
        self.finish_trash_delete_with(trash_id, device_id, &PostgresDeletion(self))
            .await
    }

    async fn finish_trash_delete_with(
        &self,
        trash_id: &str,
        device_id: &str,
        database: &impl DeletionDatabase,
    ) -> Result<(), ApiError> {
        let conflict = |e: BoxError| ApiError::conflict(e.to_string());
        let _review = self.cleanup_review_mutation();
        let parent = self.app_data().join("deleted");
        let workspace = Entry::load(&parent, trash_id)
            .map_err(|_| ApiError::not_found(format!("Trash entry {trash_id} not found")))?
            .0
            .workspace
            .ok_or_else(|| ApiError::conflict("only a workspace delete can be finished"))?;
        let _operation = self.workspace_operation(&workspace.id).await;
        let _publication = self.resource_publication_lock()?;
        let (mut entry, root) = Entry::load(&parent, trash_id).map_err(conflict)?;
        if entry.workspace.as_ref() != Some(&workspace)
            || !matches!(
                entry.phase,
                Phase::Prepared | Phase::Moved | Phase::DatabaseDropped
            )
        {
            return Err(ApiError::conflict(format!(
                "the Trash entry for \"{}\" is not an interrupted delete",
                workspace.name
            )));
        }
        let registered = self
            .inner
            .registry
            .lock()
            .unwrap()
            .get(&workspace.id)
            .cloned();
        if registered.is_some_and(|current| current != workspace) {
            return Err(ApiError::conflict(format!(
                "\"{}\" was registered again after its delete began",
                workspace.name
            )));
        }
        let folder = if entry
            .original_dir
            .try_exists()
            .map_err(|e| conflict(e.into()))?
        {
            entry.original_dir.clone()
        } else {
            root.join("workspace")
        };
        self.require_unowned_by_others(&workspace, &folder, &self.cleanup_peer_paths())
            .map_err(conflict)?;
        let audit = match entry.audit.clone() {
            Some(audit) => audit,
            None => {
                let audit = Audit::begin(
                    self.app_data(),
                    &workspace.id,
                    Actor::Device {
                        device_id: device_id.into(),
                    },
                )
                .map_err(conflict)?;
                entry.audit = Some(audit.clone());
                entry.save(&root).map_err(conflict)?;
                audit
            }
        };
        let _owner = self
            .stop_for_deletion(&workspace.id, &folder)
            .await
            .map_err(conflict)?;
        entry.complete_worktree_backup(&root).map_err(conflict)?;
        if !entry.dump_ready
            && workspace.cleanup.backs_up_database_on_delete()
            && matches!(entry.phase, Phase::Prepared | Phase::Moved)
            && database.exists(&workspace).await.map_err(conflict)?
        {
            let fresh = root.join(".database.dump.redo");
            match std::fs::remove_file(&fresh) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                other => other.map_err(|e| conflict(e.into()))?,
            }
            database
                .dump(&workspace, fresh.clone())
                .await
                .map_err(|e| {
                    ApiError::conflict(format!(
                        "could not copy the database of \"{}\", so nothing was dropped: {e}",
                        workspace.name
                    ))
                })?;
            entry.adopt_dump(&root, &fresh).map_err(conflict)?;
        }
        self.advance_deletion(&workspace, &mut entry, &root, database)
            .await
            .map_err(conflict)?;
        audit.completed(self.app_data()).map_err(conflict)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeDatabase {
        present: Mutex<bool>,
        dump_fails: bool,
        steps: Mutex<Vec<&'static str>>,
    }

    impl FakeDatabase {
        fn present() -> Self {
            Self {
                present: Mutex::new(true),
                ..Self::default()
            }
        }
        fn steps(&self) -> Vec<&'static str> {
            self.steps.lock().unwrap().clone()
        }
    }

    impl DeletionDatabase for FakeDatabase {
        async fn exists(&self, _: &Workspace) -> Result<bool, BoxError> {
            Ok(*self.present.lock().unwrap())
        }
        async fn dump(&self, _: &Workspace, path: PathBuf) -> Result<(), BoxError> {
            self.steps.lock().unwrap().push("dump");
            if self.dump_fails {
                return Err("injected dump failure".into());
            }
            std::fs::write(path, "fresh copy")?;
            Ok(())
        }
        async fn mark_original(&self, _: &Workspace, _: String) -> Result<(), BoxError> {
            self.steps.lock().unwrap().push("mark");
            Ok(())
        }
        async fn drop_database(&self, _: &Workspace) -> Result<(), BoxError> {
            self.steps.lock().unwrap().push("drop");
            *self.present.lock().unwrap() = false;
            Ok(())
        }
    }

    /// The state a cancelled delete request leaves: a published journal with
    /// no audit and no committed dump, the folder in place, still registered.
    async fn interrupted_delete(
        phase: Phase,
    ) -> (GatewayState, tempfile::TempDir, PathBuf, String) {
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let dir = temp.path().join("personal");
        std::fs::create_dir_all(&dir).unwrap();
        state
            .adopt_workspace_with_cleanup(
                &dir,
                Some("personal"),
                None,
                None,
                &Default::default(),
                false,
            )
            .await
            .unwrap();
        let workspace = state
            .inner
            .registry
            .lock()
            .unwrap()
            .get("personal")
            .unwrap()
            .clone();
        let (mut entry, root) = Entry::prepare(
            &state.app_data().join("deleted"),
            &workspace,
            dir.clone(),
            |_| Ok(()),
        )
        .unwrap();
        assert!(!entry.dump_ready);
        if phase == Phase::Moved {
            entry.move_folder(&root).unwrap();
        }
        let id = root.file_name().unwrap().to_str().unwrap().to_owned();
        (state, temp, dir, id)
    }

    fn journal(state: &GatewayState, id: &str) -> Entry {
        Entry::load(&state.app_data().join("deleted"), id)
            .unwrap()
            .0
    }

    #[tokio::test]
    async fn finish_delete_completes_a_cancelled_delete_and_empties() {
        let (state, _temp, dir, id) = interrupted_delete(Phase::Prepared).await;
        assert!(state.snapshot_restore_blocks("personal"));
        let finishable = |state: &GatewayState| {
            crate::trash::list(state.app_data())
                .unwrap()
                .iter()
                .any(|row| row.id == id && row.finish_available)
        };
        assert!(finishable(&state));
        assert!(crate::trash::list(state.app_data())
            .unwrap()
            .iter()
            .any(|row| row.id == id && row.database_backup));
        let database = FakeDatabase::present();
        state
            .finish_trash_delete_with(&id, "device", &database)
            .await
            .unwrap();
        assert_eq!(database.steps(), ["dump", "mark", "drop"]);
        let entry = journal(&state, &id);
        assert_eq!(entry.phase, Phase::Completed);
        assert!(entry.dump_ready);
        assert!(entry.audit.is_some());
        assert!(!dir.exists());
        assert!(!state.inner.registry.lock().unwrap().contains("personal"));
        assert!(!state.snapshot_restore_blocks("personal"));
        assert!(!finishable(&state));
        let empty = state.freeze_empty_trash().await.unwrap();
        assert!(empty.members.iter().any(|member| member.id == id));
    }

    #[tokio::test]
    async fn a_failed_copy_drops_nothing_and_keeps_the_journal() {
        let (state, _temp, dir, id) = interrupted_delete(Phase::Prepared).await;
        let database = FakeDatabase {
            dump_fails: true,
            ..FakeDatabase::present()
        };
        let error = state
            .finish_trash_delete_with(&id, "device", &database)
            .await
            .unwrap_err();
        assert!(error.message().contains("nothing was dropped"));
        assert_eq!(database.steps(), ["dump"]);
        assert!(*database.present.lock().unwrap());
        assert_eq!(journal(&state, &id).phase, Phase::Prepared);
        assert!(dir.exists());
        assert!(state.inner.registry.lock().unwrap().contains("personal"));
    }

    #[tokio::test]
    async fn a_moved_journal_whose_database_is_gone_completes_without_a_copy() {
        let (state, _temp, _dir, id) = interrupted_delete(Phase::Moved).await;
        let database = FakeDatabase::default();
        state
            .finish_trash_delete_with(&id, "device", &database)
            .await
            .unwrap();
        assert_eq!(database.steps(), ["drop"]);
        assert_eq!(journal(&state, &id).phase, Phase::Completed);
    }

    #[tokio::test]
    async fn a_finished_journal_cannot_be_finished_again() {
        let (state, _temp, _dir, id) = interrupted_delete(Phase::Prepared).await;
        let database = FakeDatabase::present();
        state
            .finish_trash_delete_with(&id, "device", &database)
            .await
            .unwrap();
        let error = state
            .finish_trash_delete_with(&id, "device", &database)
            .await
            .unwrap_err();
        assert!(error.message().contains("not an interrupted delete"));
        assert_eq!(database.steps(), ["dump", "mark", "drop"]);
    }
}
