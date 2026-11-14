//! Small journal projection only. Keys and archive verification stay in the
//! offline engine, preserving ADR 0014's gateway dependency boundary.
use super::*;
use serde::{Deserialize, Serialize};

const RESTORE_FAILURE: &str =
    "Snapshot restore needs attention. The workspace remains blocked to protect its data.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Phase {
    Requested,
    Refused,
    RecoveryCapture,
    Staging,
    DbRestore,
    FileSwap,
    FilesRestored,
    Completed,
    FailedUnchanged,
    RollingBack,
    RolledBack,
    NeedsAttention,
}

impl Phase {
    /// Phases only the offline restore child runs in. It holds the owner lock
    /// but publishes no pid, so the pidfile names a stopped engine whose pid
    /// the system may have reused.
    fn offline_child_owns(self) -> bool {
        matches!(
            self,
            Self::RecoveryCapture
                | Self::Staging
                | Self::DbRestore
                | Self::FileSwap
                | Self::RollingBack
        )
    }

    fn active(self) -> bool {
        !matches!(
            self,
            Self::Refused | Self::Completed | Self::FailedUnchanged | Self::RolledBack
        )
    }
}

/// Deliberately omits actor, reason, filesystem policy and all archive metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Status {
    restore_id: String,
    workspace_id: String,
    pub(super) phase: Phase,
    updated_at: chrono::DateTime<chrono::Utc>,
}

fn valid_restore_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_digit() || matches!(b, b'a'..=b'f')
            }
        })
}

fn records(root: &Path, workspace: &str) -> Result<Vec<Status>, BoxError> {
    match std::fs::symlink_metadata(root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Ok(m) if m.is_dir() => {}
        _ => return Err("snapshot journal unavailable".into()),
    }
    let journal = root.join("journal");
    if !std::fs::symlink_metadata(&journal)?.is_dir() {
        return Err("snapshot journal unavailable".into());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(journal)? {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let meta = std::fs::symlink_metadata(&path)?;
        if !meta.is_file() || meta.len() > 16 * 1024 * 1024 {
            return Err("snapshot journal unavailable".into());
        }
        let status: Status = serde_json::from_reader(std::fs::File::open(&path)?)?;
        if !valid_restore_id(&status.restore_id)
            || status.workspace_id != workspace
            || path.file_stem().and_then(|s| s.to_str()) != Some(status.restore_id.as_str())
        {
            return Err("snapshot journal identity mismatch".into());
        }
        out.push(status);
    }
    out.sort_by_key(|s| s.updated_at);
    if out.iter().filter(|s| s.phase.active()).count() > 1 {
        return Err("multiple active snapshot restores".into());
    }
    Ok(out)
}

impl GatewayState {
    pub(super) async fn workspace_operation(&self, id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        self.workspace_operation_mutex(id).lock_owned().await
    }

    pub(super) fn try_workspace_operation(
        &self,
        id: &str,
    ) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        self.workspace_operation_mutex(id).try_lock_owned().ok()
    }

    fn workspace_operation_mutex(&self, id: &str) -> Arc<AsyncMutex<()>> {
        self.inner
            .workspace_operations
            .lock()
            .unwrap()
            .entry(id.to_owned())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    pub(super) fn snapshot_root(&self, id: &str) -> Result<PathBuf, BoxError> {
        let database = postgres::database_name(id)?;
        if database.len() > lucidos_pg_url::PG_MAX_IDENTIFIER_BYTES {
            return Err("snapshot database identity invalid".into());
        }
        let snapshots = match &self.inner.relocated_private {
            Some(private) => private.join(lucidos_home::private::SNAPSHOTS_DIR),
            None => lucidos_home::private::entry_in(
                &self.inner.snapshot_home,
                lucidos_home::private::SNAPSHOTS_DIR,
            ),
        };
        Ok(snapshots.join(database))
    }

    pub(super) fn trash_snapshots(&self, id: &str, trash: &Path) -> Result<(), BoxError> {
        let root = self.snapshot_root(id)?;
        if root.try_exists()? {
            crate::trash::rename_exclusive(&root, &trash.join("snapshots"))?;
            std::fs::File::open(trash)?.sync_all()?;
            std::fs::File::open(root.parent().ok_or("snapshot root unavailable")?)?.sync_all()?;
        }
        Ok(())
    }

    pub(crate) fn snapshot_restore_status(&self, id: &str) -> Result<Option<Status>, ApiError> {
        self.snapshot_restore_status_for(id, None)
    }

    pub(crate) fn snapshot_restore_status_for(
        &self,
        id: &str,
        restore_id: Option<&str>,
    ) -> Result<Option<Status>, ApiError> {
        if restore_id.is_some_and(|value| !valid_restore_id(value)) {
            return Err(ApiError::bad_request("snapshot_restore_id_invalid"));
        }
        if !self.inner.registry.lock().unwrap().contains(id) {
            return Err(ApiError::not_found("workspace not found"));
        }
        self.snapshot_records(id)
            .map(|mut records| {
                let mut selected = if let Some(restore_id) = restore_id {
                    records.into_iter().find(|s| s.restore_id == restore_id)
                } else {
                    let active = records.iter().position(|s| s.phase.active());
                    active.map(|i| records.remove(i)).or_else(|| records.pop())
                };
                // A failed offline attempt may not have reached a journal
                // write. Expose the observed failure, without rewriting the
                // journal or treating process exit as completion proof.
                // Startup reconstructs this cache by recovering the still
                // active journal before allowing an engine to start.
                if let Some(status) = selected.as_mut() {
                    if status.phase.active()
                        && self.boot_failure(id) == Some(BootFailure::terminal(RESTORE_FAILURE))
                    {
                        status.phase = Phase::NeedsAttention;
                    }
                }
                selected
            })
            .map_err(|_| ApiError::conflict("snapshot_restore_needs_attention"))
    }

    fn snapshot_records(&self, id: &str) -> Result<Vec<Status>, BoxError> {
        records(&self.snapshot_root(id)?, id)
    }

    pub(super) fn snapshot_restore_failure(&self, id: &str) -> Option<&'static str> {
        match self.snapshot_restore_status(id) {
            Ok(Some(status)) if status.phase == Phase::NeedsAttention => Some(RESTORE_FAILURE),
            Err(_) => Some(RESTORE_FAILURE),
            _ => None,
        }
    }

    pub(super) fn cleanup_failure(&self, id: &str) -> Option<&'static str> {
        match crate::trash::blocks(&self.app_data().join("deleted"), id) {
            Ok(false) => None,
            Ok(true) => Some("Workspace cleanup was interrupted. Open Manage workspaces and recover this entry in Trash before opening the workspace."),
            Err(_) => Some("Workspace cleanup records could not be read. Recovery is required before opening the workspace to protect its data."),
        }
    }

    pub(crate) fn snapshot_restore_blocks(&self, id: &str) -> bool {
        // Recovery may have returned the directory but not yet republished its
        // registration. The durable journal still owns this identity.
        if self.cleanup_failure(id).is_some() {
            return true;
        }
        if !self.inner.registry.lock().unwrap().contains(id) {
            return false;
        }
        self.inner.snapshot_restores.lock().unwrap().contains(id)
            || self
                .snapshot_records(id)
                .map(|r| r.iter().any(|s| s.phase.active()))
                .unwrap_or(true)
    }

    /// Only the authenticated local engine submits a journal after enforcing
    /// restore authority. A browser cannot substitute a journal or an archive.
    pub(crate) async fn begin_snapshot_restore(
        &self,
        id: &str,
        restore_id: &str,
    ) -> Result<(), ApiError> {
        let _operation = self.workspace_operation(id).await;
        if !valid_restore_id(restore_id) {
            return Err(ApiError::bad_request("invalid restore id"));
        }
        let status = self
            .snapshot_restore_status(id)?
            .ok_or_else(|| ApiError::not_found("restore not found"))?;
        if status.restore_id != restore_id || status.phase != Phase::Requested {
            return Err(ApiError::conflict("snapshot restore phase refused"));
        }
        {
            let mut starting = self.inner.starting.lock().await;
            if starting.contains(id) {
                return Err(ApiError::conflict("workspace is starting"));
            }
            let mut restoring = self.inner.snapshot_restores.lock().unwrap();
            if !restoring.insert(id.to_owned()) {
                return Err(ApiError::conflict("workspace is restoring"));
            }
            starting.insert(id.to_owned());
        }
        let state = self.clone();
        let id = id.to_owned();
        let restore_id = restore_id.to_owned();
        tokio::spawn(async move {
            state
                .execute_snapshot_restore(&id, &restore_id, false)
                .await;
        });
        Ok(())
    }

    async fn execute_snapshot_restore(&self, id: &str, restore_id: &str, recovering: bool) {
        let result = self.snapshot_offline(id, restore_id, recovering).await;
        if result.is_err() && !recovering {
            // Process exit and output are observations, never completion proof.
            // The same offline recovery path also settles pre-mutation failures.
            if self.snapshot_offline(id, restore_id, true).await.is_err() {
                crate::log!(
                    "[Gateway] snapshot recovery for '{id}' could not finish; engine blocked"
                );
            }
        }
        if result.is_err() {
            // Never include child stderr, a URL or a journal's user text.
            crate::log!("[Gateway] snapshot restore for '{id}' needs journal reconciliation");
        }
        let status = self.snapshot_restore_status(id);
        let spawn = matches!(status, Ok(Some(ref s)) if matches!(s.phase,
            Phase::FilesRestored | Phase::Completed | Phase::RolledBack | Phase::FailedUnchanged | Phase::Refused));
        if spawn {
            let ws = self.inner.registry.lock().unwrap().get(id).cloned();
            if let Some(ws) = ws {
                self.bring_up_after_snapshot(ws).await;
            }
        } else {
            self.set_boot_failure(id, BootFailure::terminal(RESTORE_FAILURE));
        }
        self.inner.starting.lock().await.remove(id);
        self.inner.snapshot_restores.lock().unwrap().remove(id);
    }

    async fn snapshot_offline(
        &self,
        id: &str,
        restore_id: &str,
        recovering: bool,
    ) -> Result<(), BoxError> {
        #[cfg(test)]
        {
            let probe = self.inner.snapshot_test.lock().unwrap().clone();
            if let Some(probe) = probe {
                assert!(recovering);
                assert_eq!(
                    self.snapshot_restore_status(id).unwrap().unwrap().phase,
                    Phase::FileSwap
                );
                probe.entered.notify_one();
                probe.release.notified().await;
                let path = self
                    .snapshot_root(id)?
                    .join("journal")
                    .join(format!("{restore_id}.json"));
                let mut journal: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path)?)?;
                journal["phase"] = "rolled-back".into();
                std::fs::write(path, serde_json::to_vec(&journal)?)?;
                return Ok(());
            }
        }
        let ws = self
            .inner
            .registry
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("workspace not found")?;
        // No restart-intent device attribution: a restore must never gain the
        // automatic-resume semantics of a user's binary switch.
        self.stop_for_snapshot(id, None)
            .await
            .map_err(|_| "workspace stop failed")?;
        let dir = ws.resolve_dir(self.app_data());
        let deadline = Instant::now() + Duration::from_secs(60);
        // A gateway that died mid-restore may leave its offline child running.
        // Signalling the pidfile's pid then could stop another workspace.
        let child_may_run = recovering
            && self
                .snapshot_restore_status_for(id, Some(restore_id))
                .ok()
                .flatten()
                .is_some_and(|status| status.phase.offline_child_owns());
        if !child_may_run {
            // Also covers a detached engine not yet adopted by the gateway. The
            // offline runner independently requires the workspace owner lock.
            stack::reclaim_stale_engine(&dir);
            while stack::live_engine_pid(&dir).is_some() {
                if Instant::now() >= deadline {
                    return Err("workspace stop timed out".into());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        // A gateway crash can leave the old offline child alive. Observe its
        // owner lock before launching recovery; never compete with its replay.
        loop {
            if let Some(owner) = lucidos_owner_lock::OwnerLock::try_acquire(
                &lucidos_owner_lock::engine::lock_path(&dir),
            )? {
                drop(owner);
                break;
            }
            if !recovering && Instant::now() >= deadline {
                return Err("workspace stop timed out".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let provisioned = self
            .ensure_postgres(&ws)
            .await
            .map_err(|_| "snapshot database unavailable")?;
        let exit = tokio::process::Command::new(&self.inner.engine_bin)
            .arg(if recovering {
                "snapshot-recover"
            } else {
                "snapshot-restore"
            })
            .arg("--workspace-dir")
            .arg(&dir)
            .arg("--snapshot-root")
            .arg(self.snapshot_root(id)?)
            .arg("--restore-id")
            .arg(restore_id)
            .env("DATABASE_URL", &provisioned.database_url)
            .env("LUCIDOS_WORKSPACE_ID", id)
            .env("LUCIDOS_WORKSPACE", &dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await?;
        if !exit.success() {
            return Err("snapshot offline command failed".into());
        }
        Ok(())
    }

    pub(super) async fn recover_snapshot_restores(&self) {
        let workspaces = self.inner.registry.lock().unwrap().workspaces.clone();
        for ws in workspaces {
            match self.snapshot_records(&ws.id) {
                Ok(records) => {
                    for status in records
                        .into_iter()
                        .filter(|s| s.phase.active() && s.phase != Phase::NeedsAttention)
                    {
                        self.inner
                            .snapshot_restores
                            .lock()
                            .unwrap()
                            .insert(ws.id.clone());
                        self.inner.starting.lock().await.insert(ws.id.clone());
                        let state = self.clone();
                        let id = ws.id.clone();
                        tokio::spawn(async move {
                            state
                                .execute_snapshot_restore(&id, &status.restore_id, true)
                                .await;
                        });
                    }
                }
                Err(_) => crate::log!(
                    "[Gateway] snapshot journal for '{}' needs attention; engine blocked",
                    ws.id
                ),
            }
        }
    }
}

// Replaces only external process execution in the startup ordering test.
// The actual startup scan, admission, reconciliation and spawn decision run.
#[cfg(test)]
pub(super) struct OrderingProbe {
    pub entered: tokio::sync::Notify,
    pub release: tokio::sync::Notify,
    pub spawned: tokio::sync::Notify,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_ids_without_accepting_paths_or_unknown_phases() {
        assert!(valid_restore_id("12345678-1234-1234-1234-123456789abc"));
        for id in ["../journal", "", "12345678-1234-1234-1234-123456789abZ"] {
            assert!(!valid_restore_id(id));
        }
        assert!(serde_json::from_str::<Phase>("\"future-phase\"").is_err());
    }

    /// Only phases the engine never runs in may skip reclaiming the pidfile's
    /// engine. A requested restore still has a live engine to stop, and a
    /// restored engine's restricted boot publishes its own pid.
    #[test]
    fn only_offline_phases_skip_engine_reclaim() {
        for phase in [
            Phase::RecoveryCapture,
            Phase::Staging,
            Phase::DbRestore,
            Phase::FileSwap,
            Phase::RollingBack,
        ] {
            assert!(phase.offline_child_owns(), "{phase:?}");
        }
        for phase in [
            Phase::Requested,
            Phase::FilesRestored,
            Phase::NeedsAttention,
            Phase::Completed,
        ] {
            assert!(!phase.offline_child_owns(), "{phase:?}");
        }
    }
    #[test]
    fn missing_store_is_empty_but_redirected_or_corrupt_journal_is_not() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("snapshots");
        assert!(records(&root, "demo").unwrap().is_empty());
        std::fs::create_dir(&root).unwrap();
        assert!(records(&root, "demo").is_err());
        std::fs::create_dir(root.join("journal")).unwrap();
        let id = "12345678-1234-1234-1234-123456789abc";
        let path = root.join("journal").join(format!("{id}.json"));
        let record = serde_json::json!({"restore_id":id,"workspace_id":"demo","phase":"file-swap","updated_at":"2026-01-01T00:00:00Z", "reason":"private-sentinel"});
        std::fs::write(&path, record.to_string()).unwrap();
        let status = records(&root, "demo").unwrap().remove(0);
        assert!(status.phase.active());
        assert!(!serde_json::to_string(&status)
            .unwrap()
            .contains("private-sentinel"));
        assert!(records(&root, "other").is_err());
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(temp.path().join("absent"), &path).unwrap();
        assert!(records(&root, "demo").is_err());
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use axum::response::IntoResponse;

    fn fixture() -> (tempfile::TempDir, GatewayState, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let ws = Workspace::gateway_provisioned("snapshot-demo".into(), "Demo".into(), 5555);
        state.inner.registry.lock().unwrap().add(ws).unwrap();
        let root = state.snapshot_root("snapshot-demo").unwrap();
        std::fs::create_dir_all(root.join("journal")).unwrap();
        (temp, state, root)
    }

    fn write(root: &Path, phase: &str) -> String {
        let id = "12345678-1234-1234-1234-123456789abc";
        std::fs::write(
            root.join("journal").join(format!("{id}.json")),
            serde_json::json!({"restore_id":id,"workspace_id":"snapshot-demo", "phase":phase,
                "updated_at":"2026-01-01T00:00:00Z"})
            .to_string(),
        )
        .unwrap();
        id.to_owned()
    }

    #[test]
    fn a_relocated_private_dir_holds_the_snapshots() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
        let private = temp.path().join("moved-private");
        Arc::get_mut(&mut state.inner).unwrap().relocated_private = Some(private.clone());
        let root = state.snapshot_root("snapshot-demo").unwrap();
        assert!(root.starts_with(private.join(lucidos_home::private::SNAPSHOTS_DIR)));
    }

    #[tokio::test]
    async fn offline_failure_is_observable_without_rewriting_the_authoritative_journal() {
        let (_temp, state, root) = fixture();
        let restore_id = write(&root, "requested");
        let journal = root.join("journal").join(format!("{restore_id}.json"));
        let before = std::fs::read(&journal).unwrap();
        // An unusable workspace directory makes both offline attempts fail
        // before provisioning or changing a journal. No shared service is used.
        let workspace = state
            .inner
            .registry
            .lock()
            .unwrap()
            .get("snapshot-demo")
            .unwrap()
            .resolve_dir(state.app_data());
        std::fs::create_dir_all(workspace.parent().unwrap()).unwrap();
        std::fs::write(&workspace, b"not a directory").unwrap();
        state
            .execute_snapshot_restore("snapshot-demo", &restore_id, false)
            .await;
        assert_eq!(
            state
                .snapshot_restore_status_for("snapshot-demo", Some(&restore_id))
                .unwrap()
                .unwrap()
                .phase,
            Phase::NeedsAttention
        );
        assert_eq!(std::fs::read(&journal).unwrap(), before);
        assert!(state.snapshot_restore_blocks("snapshot-demo"));
        assert!(state.start_workspace("snapshot-demo").is_err());
        let status = state.list_status().await;
        assert_eq!(status[0].health, Health::Restoring);
        assert!(status[0]
            .last_error
            .as_ref()
            .unwrap()
            .contains("needs attention"));
        // A retained completed journal must never inherit the failure of a
        // different active restore, nor become evidence that it completed.
        let older = "12345678-1234-1234-1234-123456789abd";
        std::fs::write(
            root.join("journal").join(format!("{older}.json")),
            serde_json::json!({"restore_id":older,"workspace_id":"snapshot-demo", "phase":"completed",
                "updated_at":"2025-01-01T00:00:00Z"}).to_string(),
        ).unwrap();
        assert_eq!(
            state
                .snapshot_restore_status_for("snapshot-demo", Some(older))
                .unwrap()
                .unwrap()
                .phase,
            Phase::Completed
        );
        let response = serve_workspace(
            &state,
            "snapshot-demo",
            "/snapshot-demo/",
            axum::http::Request::builder()
                .uri("/snapshot-demo/api/v1/health")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(
            response.status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(error["reason"], "snapshot_restore_needs_attention");
    }

    #[tokio::test]
    async fn journal_blocks_proxy_start_restart_stop_and_delete_without_mutation() {
        let (_temp, state, root) = fixture();
        for phase in [
            "requested",
            "db-restore",
            "file-swap",
            "files-restored",
            "rolling-back",
            "needs-attention",
        ] {
            write(&root, phase);
            assert!(state.snapshot_restore_blocks("snapshot-demo"));
            assert!(state.start_workspace("snapshot-demo").is_err());
            assert!(state
                .restart_workspace("snapshot-demo", None)
                .await
                .is_err());
            assert!(state.stop_workspace("snapshot-demo", None).await.is_err());
            {
                let mut registry = state.inner.registry.lock().unwrap();
                let workspace = registry
                    .workspaces
                    .iter_mut()
                    .find(|ws| ws.id == "snapshot-demo")
                    .unwrap();
                workspace.cleanup.origin = crate::registry::WorkspaceOrigin::Eval;
                workspace.cleanup.ephemeral = true;
                workspace.cleanup.owner_token_digest = Some(crate::auth::digest("test-owner"));
            }
            let response = state
                .delete_workspace_authorized("snapshot-demo", Some("Demo"), Some("test-owner"))
                .await
                .unwrap_err()
                .into_response();
            let body = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"],
                "workspace_restoring"
            );
            state.lazy_start("snapshot-demo").await;
            assert!(state.inner.stacks.lock().await.is_empty());
            let status = state.list_status().await;
            assert_eq!(status[0].health, Health::Restoring);
            let response = serve_workspace(
                &state,
                "snapshot-demo",
                "/snapshot-demo/",
                axum::http::Request::builder()
                    .uri("/snapshot-demo/api/v1/health")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(
                response.status(),
                axum::http::StatusCode::SERVICE_UNAVAILABLE
            );
            assert!(state
                .inner
                .registry
                .lock()
                .unwrap()
                .contains("snapshot-demo"));
        }
        write(&root, "rolled-back");
        assert!(!state.snapshot_restore_blocks("snapshot-demo"));
    }

    #[tokio::test]
    async fn startup_snapshot_rollback_finishes_before_engine_spawn() {
        let (_temp, state, root) = fixture();
        write(&root, "file-swap");
        let probe = Arc::new(OrderingProbe {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            spawned: tokio::sync::Notify::new(),
        });
        *state.inner.snapshot_test.lock().unwrap() = Some(probe.clone());
        state.recover_snapshot_restores().await;
        tokio::time::timeout(Duration::from_secs(5), probe.entered.notified())
            .await
            .unwrap();
        assert!(state.inner.stacks.lock().await.is_empty());
        assert!(state.inner.starting.lock().await.contains("snapshot-demo"));
        assert_eq!(
            state
                .snapshot_restore_status("snapshot-demo")
                .unwrap()
                .unwrap()
                .phase,
            Phase::FileSwap
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), probe.spawned.notified())
                .await
                .is_err()
        );
        probe.release.notify_one();
        tokio::time::timeout(Duration::from_secs(5), probe.spawned.notified())
            .await
            .unwrap();
        assert_eq!(
            state
                .snapshot_restore_status("snapshot-demo")
                .unwrap()
                .unwrap()
                .phase,
            Phase::RolledBack
        );
    }

    #[test]
    fn trash_snapshot_retry_never_replaces_an_occupied_archive() {
        let (_temp, state, root) = fixture();
        write(&root, "completed");
        let trash = tempfile::tempdir_in(state.app_data()).unwrap();
        std::fs::create_dir(trash.path().join("snapshots")).unwrap();
        assert!(state
            .trash_snapshots("snapshot-demo", trash.path())
            .is_err());
        assert!(root.exists());
        assert_eq!(
            std::fs::read_dir(trash.path().join("snapshots"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn trash_keeps_snapshots_and_journals_out_of_a_reused_slug() {
        let (_temp, state, root) = fixture();
        write(&root, "completed");
        std::fs::write(root.join("archive-sentinel"), "retained").unwrap();
        let trash = tempfile::tempdir_in(state.app_data()).unwrap();
        state
            .trash_snapshots("snapshot-demo", trash.path())
            .unwrap();
        assert!(!root.exists());
        let retained = trash.path().join("snapshots");
        assert_eq!(
            std::fs::read_to_string(retained.join("archive-sentinel")).unwrap(),
            "retained"
        );
        assert_eq!(records(&retained, "snapshot-demo").unwrap().len(), 1);
        assert!(!state.snapshot_restore_blocks("snapshot-demo"));
    }

    #[test]
    fn exact_status_retains_older_terminal_journals_and_refuses_invalid_identity() {
        let (_temp, state, root) = fixture();
        let older = write(&root, "completed");
        let newer = "22345678-1234-1234-1234-123456789abc";
        std::fs::write(
            root.join("journal").join(format!("{newer}.json")),
            serde_json::json!({"restore_id":newer,"workspace_id":"snapshot-demo",
                "phase":"requested","updated_at":"2026-01-02T00:00:00Z"})
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            state
                .snapshot_restore_status("snapshot-demo")
                .unwrap()
                .unwrap()
                .restore_id,
            newer
        );
        let status = state
            .snapshot_restore_status_for("snapshot-demo", Some(&older))
            .unwrap()
            .unwrap();
        assert_eq!(status.restore_id, older);
        assert_eq!(status.phase, Phase::Completed);
        assert!(state
            .snapshot_restore_status_for(
                "snapshot-demo",
                Some("32345678-1234-1234-1234-123456789abc")
            )
            .unwrap()
            .is_none());
        assert!(state
            .snapshot_restore_status_for("snapshot-demo", Some("../escape"))
            .is_err());
        assert!(state
            .snapshot_restore_status_for("other", Some(&older))
            .is_err());
        // Exact lookup still refuses a corrupt sibling rather than concealing
        // an ambiguous or unreadable workspace restore state.
        std::fs::write(root.join("journal").join(format!("{newer}.json")), "{}").unwrap();
        assert!(state
            .snapshot_restore_status_for("snapshot-demo", Some(&older))
            .is_err());
    }

    #[tokio::test]
    async fn control_status_selects_exact_journal_without_starting_engine() {
        use tower::ServiceExt as _;
        let (_temp, state, root) = fixture();
        let older = write(&root, "completed");
        let newer = "22345678-1234-1234-1234-123456789abc";
        std::fs::write(
            root.join("journal").join(format!("{newer}.json")),
            serde_json::json!({"restore_id":newer,"workspace_id":"snapshot-demo",
                "phase":"requested","updated_at":"2026-01-02T00:00:00Z"})
            .to_string(),
        )
        .unwrap();
        for (query, credential, expected) in [
            (
                format!("?restore_id={older}"),
                true,
                axum::http::StatusCode::OK,
            ),
            (
                format!("?restore_id={older}"),
                false,
                axum::http::StatusCode::UNAUTHORIZED,
            ),
            (
                "?restore_id=invalid".into(),
                true,
                axum::http::StatusCode::BAD_REQUEST,
            ),
            (String::new(), true, axum::http::StatusCode::OK),
        ] {
            let mut request = axum::http::Request::builder().uri(format!(
                "/~/api/v1/control/workspaces/snapshot-demo/snapshot-restore-status{query}"
            ));
            if credential {
                request = request.header(crate::auth::HEADER_LOCAL_TOKEN, state.local_token());
            }
            let response = crate::server::gateway_router(state.clone())
                .oneshot(request.body(axum::body::Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
            if expected == axum::http::StatusCode::OK {
                let bytes = axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap();
                let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(
                    body["restore"]["restore_id"],
                    if query.is_empty() { newer } else { &older }
                );
                assert!(body["restore"].get("actor").is_none());
            }
        }
        assert!(state.inner.stacks.lock().await.is_empty());
        assert!(state.inner.starting.lock().await.is_empty());
    }

    #[tokio::test]
    async fn missing_and_terminal_journals_cannot_launch_an_offline_command() {
        let (_temp, state, root) = fixture();
        let id = write(&root, "completed");
        assert!(state
            .begin_snapshot_restore("snapshot-demo", &id)
            .await
            .is_err());
        assert!(state
            .begin_snapshot_restore("snapshot-demo", "../escape")
            .await
            .is_err());
        assert!(state.begin_snapshot_restore("unknown", &id).await.is_err());
        assert!(state.inner.snapshot_restores.lock().unwrap().is_empty());
        assert!(state.inner.starting.lock().await.is_empty());
        let error = state
            .snapshot_restore_status("unknown")
            .unwrap_err()
            .into_response();
        assert_eq!(error.status(), axum::http::StatusCode::NOT_FOUND);
    }
}
