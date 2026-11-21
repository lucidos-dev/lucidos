//! Frozen, device-confirmed permanent removal of completed Trash entries.
use super::*;
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};

#[derive(Serialize, Deserialize)]
pub(crate) struct EmptyBatch {
    pub id: String,
    pub phrase: String,
    pub members: Vec<EmptyMember>,
    pub results: Vec<cleanup_actions::MemberResult>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct EmptyMember {
    pub id: String,
    pub bytes: u64,
    fingerprint: String,
    workspace: Option<String>,
    #[serde(default)]
    shared_files: std::collections::BTreeSet<(u64, u64)>,
}

fn entry_path(app_data: &Path, id: &str) -> Result<PathBuf, BoxError> {
    let (parent, name) = match id.strip_prefix("snapshots:") {
        Some(name) => ("deleted-snapshots", name),
        None => ("deleted", id),
    };
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err("invalid Trash identity".into());
    }
    Ok(app_data.join(parent).join(name))
}

fn observe(app_data: &Path, id: &str) -> Result<EmptyMember, BoxError> {
    observe_with_shared_files(app_data, id, None)
}

fn observe_with_shared_files(
    app_data: &Path,
    id: &str,
    frozen_shared_files: Option<&std::collections::BTreeSet<(u64, u64)>>,
) -> Result<EmptyMember, BoxError> {
    let root = entry_path(app_data, id)?;
    if !std::fs::symlink_metadata(&root)?.is_dir() {
        return Err("Trash entry is not a directory".into());
    }
    let journal = root.join("entry.json");
    let workspace = match std::fs::symlink_metadata(&journal) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
                return Err("invalid Trash journal".into());
            }
            if id.starts_with("database-") {
                let (entry, _) = crate::trash::DatabaseEntry::load(&app_data.join("deleted"), id)?;
                if !matches!(
                    entry.phase,
                    crate::trash::Phase::Completed | crate::trash::Phase::Restored
                ) {
                    return Err("interrupted database Trash entry must be recovered first".into());
                }
                entry.repair_audit(&root)?;
                Some(format!("database:{}", entry.database))
            } else {
                let entry: crate::trash::Entry =
                    serde_json::from_reader(std::fs::File::open(journal)?)?;
                if !matches!(
                    entry.phase,
                    crate::trash::Phase::Completed | crate::trash::Phase::Restored
                ) {
                    return Err("interrupted Trash entry must be recovered first".into());
                }
                entry.repair_audit(&root)?;
                Some(
                    entry
                        .workspace
                        .map(|ws| ws.id)
                        .unwrap_or_else(|| crate::cleanup::folder_id(&entry.original_dir)),
                )
            }
        }
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                && ((!id.starts_with("workspace-") && !id.starts_with("database-"))
                    || crate::trash::legacy_folder(
                        root.parent().ok_or("missing Trash parent")?,
                        id,
                    )?) =>
        {
            None
        }
        Err(e) => return Err(e.into()),
    };
    // Include every inode and its change timestamps, without following links.
    // A changed recovery copy invalidates the device's frozen confirmation.
    fn walk(
        path: &Path,
        rows: &mut Vec<serde_json::Value>,
        shared: &mut std::collections::BTreeSet<(u64, u64)>,
        discover: bool,
    ) -> Result<(), BoxError> {
        let m = std::fs::symlink_metadata(path)?;
        let identity = (m.dev(), m.ino());
        if discover && m.is_file() && m.nlink() > 1 {
            shared.insert(identity);
        }
        // Unlinking a confirmed peer changes ctime, even when bytes and all
        // remaining names are unchanged. Freeze content for these inodes so
        // our own deletion cannot invalidate the next member's confirmation.
        let content = if m.is_file() && shared.contains(&identity) {
            use sha2::{Digest, Sha256};
            use std::os::unix::fs::OpenOptionsExt;
            let mut hash = Sha256::new();
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)?;
            let opened = file.metadata()?;
            if (opened.dev(), opened.ino()) != identity {
                return Err("Trash file changed during observation".into());
            }
            std::io::copy(&mut file, &mut hash)?;
            let after = file.metadata()?;
            if (opened.ctime(), opened.ctime_nsec(), opened.len())
                != (after.ctime(), after.ctime_nsec(), after.len())
            {
                return Err("Trash file changed during observation".into());
            }
            Some(format!("{:x}", hash.finalize()))
        } else {
            None
        };
        rows.push(serde_json::json!([
            path,
            m.dev(),
            m.ino(),
            m.mode(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            if content.is_some() { 0 } else { m.ctime() },
            if content.is_some() { 0 } else { m.ctime_nsec() }
        ]));
        if let Some(content) = content {
            rows.push(serde_json::json!([content, m.uid(), m.gid()]));
        }
        if m.is_dir() {
            let mut children = std::fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
            children.sort_by_key(|child| child.file_name());
            for child in children {
                walk(&child.path(), rows, shared, discover)?;
            }
        }
        Ok(())
    }
    let mut rows = Vec::new();
    let mut shared_files = frozen_shared_files.cloned().unwrap_or_default();
    walk(
        &root,
        &mut rows,
        &mut shared_files,
        frozen_shared_files.is_none(),
    )?;
    Ok(EmptyMember {
        id: id.into(),
        bytes: crate::cleanup::allocated_bytes(&root)?,
        fingerprint: crate::auth::digest(&serde_json::to_string(&rows)?),
        workspace,
        shared_files,
    })
}

fn batch_root(app_data: &Path, id: &str) -> Result<PathBuf, BoxError> {
    if !id.starts_with("empty-") || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err("invalid Empty Trash batch".into());
    }
    Ok(app_data.join("cleanup-batches").join(id))
}

fn save(batch: &EmptyBatch, root: &Path) -> Result<(), BoxError> {
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer(file.as_file_mut(), batch)?;
    file.as_file().sync_all()?;
    file.persist(root.join("empty.json"))?;
    std::fs::File::open(root)?.sync_all()?;
    Ok(())
}

impl GatewayState {
    pub(crate) async fn freeze_empty_trash(&self) -> Result<EmptyBatch, ApiError> {
        let app_data = self.app_data().clone();
        tokio::task::spawn_blocking(move || -> Result<EmptyBatch, BoxError> {
            let mut members = Vec::new();
            for row in crate::trash::list(&app_data)? {
                if let Ok(member) = observe(&app_data, &row.id) {
                    members.push(member);
                }
            }
            if members.is_empty() {
                return Err("no completed Trash entries are available to empty".into());
            }
            let paths = members
                .iter()
                .map(|member| entry_path(&app_data, &member.id))
                .collect::<Result<Vec<_>, _>>()?;
            let bytes = crate::cleanup::allocated_bytes_many(paths.iter().map(PathBuf::as_path))?;
            let parent = app_data.join("cleanup-batches");
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&parent)?;
            let root = tempfile::Builder::new()
                .prefix("empty-")
                .tempdir_in(&parent)?
                .keep();
            let batch = EmptyBatch {
                id: root
                    .file_name()
                    .ok_or("batch name missing")?
                    .to_string_lossy()
                    .into(),
                phrase: format!(
                    "Permanently delete {} Trash entries ({bytes} bytes)",
                    members.len()
                ),
                members,
                results: vec![],
            };
            save(&batch, &root)?;
            std::fs::File::open(parent)?.sync_all()?;
            Ok(batch)
        })
        .await
        .map_err(|e| ApiError::conflict(e.to_string()))?
        .map_err(|e| ApiError::conflict(e.to_string()))
    }

    pub(crate) async fn confirm_empty_trash(
        &self,
        id: &str,
        phrase: &str,
    ) -> Result<EmptyBatch, ApiError> {
        let failure = |e: BoxError| ApiError::conflict(e.to_string());
        let _review = self.cleanup_review_mutation();
        let _batch = self.workspace_operation(&format!("empty-trash:{id}")).await;
        // Serialize different frozen batches, including legacy entries without a workspace id.
        let _trash = self.workspace_operation("empty-trash").await;
        let root = batch_root(self.app_data(), id).map_err(failure)?;
        let mut batch: EmptyBatch = (|| -> Result<_, BoxError> {
            Ok(serde_json::from_reader(std::fs::File::open(
                root.join("empty.json"),
            )?)?)
        })()
        .map_err(failure)?;
        if batch.id != id || batch.phrase != phrase {
            return Err(ApiError::bad_request("confirmation does not match"));
        }
        for member in &batch.members {
            if batch.results.iter().any(|r| r.id == member.id) {
                continue;
            }
            let _workspace = match &member.workspace {
                Some(id) => Some(self.workspace_operation(id).await),
                None => None,
            };
            let app_data = self.app_data().clone();
            let id = member.id.clone();
            let fingerprint = member.fingerprint.clone();
            let shared_files = member.shared_files.clone();
            let result = tokio::task::spawn_blocking(move || -> Result<(), BoxError> {
                if observe_with_shared_files(&app_data, &id, Some(&shared_files))?.fingerprint
                    != fingerprint
                {
                    return Err("Trash entry changed; skipped".into());
                }
                let path = entry_path(&app_data, &id)?;
                std::fs::remove_dir_all(&path)?;
                std::fs::File::open(path.parent().ok_or("missing Trash parent")?)?.sync_all()?;
                Ok(())
            })
            .await
            .map_err(|e| ApiError::conflict(e.to_string()))?;
            batch.results.push(cleanup_actions::MemberResult {
                id: member.id.clone(),
                status: if result.is_ok() { "purged" } else { "skipped" }.into(),
                detail: result.err().map(|e| e.to_string()),
            });
            save(&batch, &root).map_err(failure)?;
        }
        Ok(batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_prefixed_trash_is_available_for_explicit_emptying() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["database-20250101-120000", "workspace-2-20250101-120000"] {
            std::fs::create_dir_all(temp.path().join("deleted").join(name)).unwrap();
            assert!(observe(temp.path(), name).is_ok());
        }
        assert!(crate::trash::list(temp.path())
            .unwrap()
            .iter()
            .all(|row| row.status == "legacy"));
        std::fs::create_dir(temp.path().join("deleted/database-abcdef")).unwrap();
        assert!(observe(temp.path(), "database-abcdef").is_err());
    }

    #[test]
    fn folder_trash_uses_the_restore_operation_key() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("folder");
        std::fs::create_dir(&source).unwrap();
        let (mut entry, root) =
            crate::trash::Entry::prepare_folder(&temp.path().join("deleted"), source.clone())
                .unwrap();
        entry.move_folder(&root).unwrap();
        entry.phase = crate::trash::Phase::Completed;
        entry.save(&root).unwrap();
        let member = observe(temp.path(), root.file_name().unwrap().to_str().unwrap()).unwrap();
        assert_eq!(member.workspace, Some(crate::cleanup::folder_id(&source)));
    }

    #[tokio::test]
    async fn empty_trash_repairs_completion_and_refuses_conflicting_audit() {
        for corrupt in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
            let home = state.app_data();
            let source = home.join("audit-folder");
            std::fs::create_dir_all(&source).unwrap();
            let audit = crate::cleanup_audit::Audit::begin(
                home,
                "audit-folder",
                crate::cleanup_audit::Actor::Device {
                    device_id: "test-device".into(),
                },
            )
            .unwrap();
            let (mut entry, root) =
                crate::trash::Entry::prepare_folder(&home.join("deleted"), source).unwrap();
            entry.audit = Some(audit);
            entry.save(&root).unwrap();
            entry.move_folder(&root).unwrap();
            entry.phase = crate::trash::Phase::Completed;
            entry.save(&root).unwrap();
            let completion = std::fs::read_dir(home.join("cleanup-events"))
                .unwrap()
                .map(|e| e.unwrap().path())
                .find(|p| {
                    p.file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .ends_with("-WorkspaceDeleted.json")
                })
                .unwrap();
            let expected = std::fs::read(&completion).unwrap();
            // Simulate failure between terminal journal sync and event publication.
            std::fs::remove_file(&completion).unwrap();
            let id = root.file_name().unwrap().to_str().unwrap();
            observe(home, id).unwrap();
            let repaired: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&completion).unwrap()).unwrap();
            let expected: serde_json::Value = serde_json::from_slice(&expected).unwrap();
            assert_eq!(repaired["operation"], expected["operation"]);
            assert_eq!(repaired["actor"], expected["actor"]);
            let batch = state.freeze_empty_trash().await.unwrap();
            assert_eq!(batch.members.len(), 1);
            if corrupt {
                std::fs::write(&completion, b"{}").unwrap();
            } else {
                std::fs::remove_file(&completion).unwrap();
            }
            let result = state
                .confirm_empty_trash(&batch.id, &batch.phrase)
                .await
                .unwrap();
            assert_eq!(
                result.results[0].status,
                if corrupt { "skipped" } else { "purged" }
            );
            assert_eq!(root.exists(), corrupt);
            if !corrupt {
                assert!(completion.exists());
            }
        }
    }

    #[tokio::test]
    async fn empty_confirmation_deduplicates_shared_files_across_entries() {
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let first = state.app_data().join("deleted/legacy-first");
        let second = state.app_data().join("deleted/legacy-second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("payload"), vec![42; 65536]).unwrap();
        std::fs::hard_link(first.join("payload"), second.join("linked")).unwrap();
        let bytes =
            crate::cleanup::allocated_bytes_many([first.as_path(), second.as_path()]).unwrap();
        let batch = state.freeze_empty_trash().await.unwrap();
        assert_eq!(batch.members.len(), 2);
        assert_eq!(
            batch.phrase,
            format!("Permanently delete 2 Trash entries ({bytes} bytes)")
        );
        let result = state
            .confirm_empty_trash(&batch.id, &batch.phrase)
            .await
            .unwrap();
        assert!(result
            .results
            .iter()
            .all(|member| member.status == "purged"));
        assert!(!first.exists());
        assert!(!second.exists());
    }

    #[tokio::test]
    async fn shared_file_content_change_invalidates_empty_confirmation() {
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let first = state.app_data().join("deleted/legacy-first");
        let second = state.app_data().join("deleted/legacy-second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let payload = first.join("payload");
        std::fs::write(&payload, b"original").unwrap();
        std::fs::hard_link(&payload, second.join("linked")).unwrap();
        let modified = std::fs::metadata(&payload).unwrap().modified().unwrap();
        let batch = state.freeze_empty_trash().await.unwrap();
        std::fs::write(&payload, b"modified").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&payload)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let result = state
            .confirm_empty_trash(&batch.id, &batch.phrase)
            .await
            .unwrap();
        assert!(result
            .results
            .iter()
            .all(|member| member.status == "skipped"));
        assert!(first.exists());
        assert!(second.exists());
    }

    #[tokio::test]
    async fn database_trash_empty_preserves_interrupted_entries() {
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let parent = state.app_data().join("deleted");
        let (_, interrupted) =
            crate::trash::DatabaseEntry::prepare(&parent, "lucidos_interrupted").unwrap();
        let (mut entry, finished) =
            crate::trash::DatabaseEntry::prepare(&parent, "lucidos_finished").unwrap();
        std::fs::write(finished.join("database.dump"), b"recovery").unwrap();
        entry.commit_dump(&finished).unwrap();
        entry.phase = crate::trash::Phase::Completed;
        entry.save(&finished).unwrap();
        let batch = state.freeze_empty_trash().await.unwrap();
        assert_eq!(batch.members.len(), 1);
        let result = state
            .confirm_empty_trash(&batch.id, &batch.phrase)
            .await
            .unwrap();
        assert_eq!(result.results[0].status, "purged");
        assert!(!finished.exists());
        assert!(interrupted.join("entry.json").exists());
    }

    #[test]
    fn only_finished_journals_are_eligible_and_paths_are_server_owned() {
        let temp = tempfile::tempdir().unwrap();
        let ws = Workspace::gateway_provisioned("scratch".into(), "Scratch".into(), 9100);
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let (mut entry, root) =
            crate::trash::Entry::prepare(&temp.path().join("deleted"), &ws, source, |_| Ok(()))
                .unwrap();
        let id = root.file_name().unwrap().to_str().unwrap();
        for phase in [
            crate::trash::Phase::Prepared,
            crate::trash::Phase::Moved,
            crate::trash::Phase::DatabaseDropped,
            crate::trash::Phase::Restoring,
        ] {
            entry.phase = phase;
            entry.save(&root).unwrap();
            assert!(observe(temp.path(), id).is_err());
        }
        for phase in [
            crate::trash::Phase::Completed,
            crate::trash::Phase::Restored,
        ] {
            entry.phase = phase;
            entry.save(&root).unwrap();
            assert_eq!(
                observe(temp.path(), id).unwrap().workspace.as_deref(),
                Some("scratch")
            );
        }
        for id in ["..", ".", "snapshots:../outside", "/outside", "snapshots:"] {
            assert!(entry_path(temp.path(), id).is_err());
        }
        for id in ["../empty-x", "empty-../outside", "empty-x.json"] {
            assert!(batch_root(temp.path(), id).is_err());
        }
    }

    #[tokio::test]
    async fn empty_trash_requires_exact_phrase_and_skips_changed_entries() {
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let legacy = state.app_data().join("deleted/legacy-folder");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("data"), "keep").unwrap();
        let batch = state.freeze_empty_trash().await.unwrap();
        assert_eq!(batch.members.len(), 1);
        assert!(batch.phrase.contains("bytes"));
        assert!(state.confirm_empty_trash(&batch.id, "wrong").await.is_err());
        std::fs::write(legacy.join("new"), "changed").unwrap();
        let result = state
            .confirm_empty_trash(&batch.id, &batch.phrase)
            .await
            .unwrap();
        assert_eq!(result.results[0].status, "skipped");
        assert!(legacy.join("data").exists());
    }

    #[tokio::test]
    async fn empty_trash_removes_legacy_snapshots_but_preserves_corrupt_and_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let snapshots = state.app_data().join("deleted-snapshots/old");
        std::fs::create_dir_all(&snapshots).unwrap();
        let corrupt = state.app_data().join("deleted/workspace-corrupt");
        std::fs::create_dir_all(&corrupt).unwrap();
        std::fs::write(corrupt.join("entry.json"), "broken").unwrap();
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, state.app_data().join("deleted/link")).unwrap();
        let batch = state.freeze_empty_trash().await.unwrap();
        assert_eq!(batch.members.len(), 1);
        let result = state
            .confirm_empty_trash(&batch.id, &batch.phrase)
            .await
            .unwrap();
        assert_eq!(result.results[0].status, "purged");
        assert!(!snapshots.exists());
        assert!(corrupt.exists());
        assert!(outside.exists());
        let retry = state
            .confirm_empty_trash(&batch.id, &batch.phrase)
            .await
            .unwrap();
        assert_eq!(retry.results.len(), 1);
    }
}
