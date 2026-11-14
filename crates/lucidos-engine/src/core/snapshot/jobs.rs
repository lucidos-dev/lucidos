//! Restart-safe capture/check results outside the database that restore rewinds.
//! Session owns mutations under both archive locks; status reads remain available
//! during a running job through atomic file publication.
use super::{handle::Handle, manifest::Manifest, store, BoxError};
use crate::engine::thread_events::MessageOrigin;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Capture,
    Check,
}

/// Failure values cannot carry primitive errors, credentials or host paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Failure {
    CaptureFailed,
    /// Free disk fell short of the floor by `needed_bytes`.
    DiskFloor {
        needed_bytes: u64,
    },
    CheckFailed,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "kebab-case", deny_unknown_fields)]
pub enum State {
    Running,
    Completed { manifest: Box<Manifest> },
    Failed { reason: Failure },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub id: Uuid,
    pub workspace_id: String,
    /// External jobs survive database rewind. Legacy jobs belong to the original log.
    #[serde(default)]
    pub restore_generation: Option<Uuid>,
    pub handle: Handle,
    pub action: Action,
    pub actor: MessageOrigin,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub state: State,
}

pub(super) fn create(
    root: &Path,
    workspace_id: &str,
    restore_generation: Option<Uuid>,
    handle: Handle,
    action: Action,
    actor: MessageOrigin,
) -> Result<Job, BoxError> {
    store::private_dir(&root.join("jobs"))?;
    let now = Utc::now();
    let job = Job {
        id: Uuid::new_v4(),
        workspace_id: workspace_id.to_owned(),
        restore_generation,
        handle,
        action,
        actor,
        created_at: now,
        updated_at: now,
        state: State::Running,
    };
    store::write_json(&root.join("jobs").join(format!("{}.json", job.id)), &job)?;
    Ok(job)
}

/// Status is an atomic file read, not an archive job. It must remain available
/// while the worker owns the archive slot. No key resolution is needed.
pub fn read(store: &store::Store, id: Uuid) -> Result<Job, BoxError> {
    get(store.root(), &store.workspace_id, id).map_err(|_| "snapshot_job_unavailable".into())
}

pub(super) fn get(root: &Path, workspace_id: &str, id: Uuid) -> Result<Job, BoxError> {
    if !std::fs::symlink_metadata(root)?.is_dir()
        || !std::fs::symlink_metadata(root.join("jobs"))?.is_dir()
    {
        return Err("snapshot_job_unavailable".into());
    }
    let path = root.join("jobs").join(format!("{id}.json"));
    store::regular_file(&path)?;
    let job: Job = serde_json::from_slice(&std::fs::read(path)?)?;
    if job.id != id || job.workspace_id != workspace_id {
        return Err("snapshot_job_unavailable".into());
    }
    Ok(job)
}

pub(super) fn finish(
    root: &Path,
    workspace_id: &str,
    id: Uuid,
    state: State,
) -> Result<Job, BoxError> {
    let mut job = get(root, workspace_id, id)?;
    if !matches!(job.state, State::Running) || matches!(state, State::Running) {
        return Err("snapshot_job_transition_refused".into());
    }
    match &state {
        State::Completed { manifest }
            if manifest.handle != job.handle || manifest.workspace_id != workspace_id =>
        {
            return Err("snapshot_job_result_refused".into());
        }
        State::Failed {
            reason: Failure::CaptureFailed | Failure::DiskFloor { .. },
        } if job.action != Action::Capture => {
            return Err("snapshot_job_result_refused".into());
        }
        State::Failed {
            reason: Failure::CheckFailed,
        } if job.action != Action::Check => {
            return Err("snapshot_job_result_refused".into());
        }
        _ => {}
    }
    job.state = state;
    job.updated_at = Utc::now();
    store::write_json(&root.join("jobs").join(format!("{id}.json")), &job)?;
    Ok(job)
}

/// Boot calls this only after acquiring both locks. An interrupted capture may
/// already have published: preserve its handle so callers can inspect it.
/// Return all terminal jobs from this generation, including results whose event
/// append was interrupted. Event identities make repeating announcements safe.
/// An unreadable record is skipped: a job file must never stop the engine boot.
pub(super) fn settle(
    root: &Path,
    workspace_id: &str,
    restore_generation: Option<Uuid>,
) -> Result<Vec<Job>, BoxError> {
    let dir = root.join("jobs");
    if !dir.try_exists()? {
        return Ok(Vec::new());
    }
    store::private_dir(&dir)?;
    let mut settled = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|s| s.strip_suffix(".json"))
            .and_then(|s| Uuid::parse_str(s).ok())
        else {
            continue;
        };
        let settled_job = get(root, workspace_id, id).and_then(|job| {
            if matches!(job.state, State::Running) {
                finish(
                    root,
                    workspace_id,
                    id,
                    State::Failed {
                        reason: Failure::Interrupted,
                    },
                )
            } else {
                Ok(job)
            }
        });
        let job = match settled_job {
            Ok(job) => job,
            Err(error) => {
                crate::log!("[Snapshot] Skipped job record {id} at boot: {error}");
                continue;
            }
        };
        if job.restore_generation == restore_generation {
            settled.push(job);
        }
    }
    Ok(settled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::secret_store::key_store::TestKeyStore;
    use crate::core::snapshot::{service::Session, store::Store, ArchiveJobKind, ArchiveJobSlot};

    fn session(root: &Path, id: &str) -> Session {
        Session::open(
            Store::at(root.into(), id).unwrap(),
            &ArchiveJobSlot::default(),
            ArchiveJobKind::SnapshotCapture,
            &TestKeyStore,
            "demo",
        )
        .unwrap()
    }

    fn actor() -> MessageOrigin {
        MessageOrigin::Device {
            device_id: "test-device".into(),
        }
    }

    #[test]
    fn failure_decodes_stored_unit_strings_and_the_disk_floor_shape() {
        for (wire, failure) in [
            ("\"capture-failed\"", Failure::CaptureFailed),
            ("\"check-failed\"", Failure::CheckFailed),
            ("\"interrupted\"", Failure::Interrupted),
            (
                r#"{"disk-floor":{"needed_bytes":42}}"#,
                Failure::DiskFloor { needed_bytes: 42 },
            ),
        ] {
            assert_eq!(serde_json::to_string(&failure).unwrap(), wire);
            assert_eq!(serde_json::from_str::<Failure>(wire).unwrap(), failure);
        }
    }

    #[test]
    fn a_disk_floor_result_is_refused_for_a_check_job() {
        let root = tempfile::tempdir().unwrap();
        let session = session(root.path(), "demo-workspace");
        let check = session
            .create_job(Handle::generate(), Action::Check, actor())
            .unwrap();
        assert!(session
            .finish_job(
                check.id,
                State::Failed {
                    reason: Failure::DiskFloor { needed_bytes: 1 }
                }
            )
            .is_err());
        let capture = session
            .create_job(Handle::generate(), Action::Capture, actor())
            .unwrap();
        assert!(session
            .finish_job(
                capture.id,
                State::Failed {
                    reason: Failure::DiskFloor { needed_bytes: 1 }
                }
            )
            .is_ok());
    }

    #[test]
    fn restart_discovers_terminal_jobs_and_keeps_identity() {
        let root = tempfile::tempdir().unwrap();
        let workspace = "demo-workspace";
        let original = session(root.path(), workspace);
        let capture = original
            .create_job(Handle::generate(), Action::Capture, actor())
            .unwrap();
        let check = original
            .create_job(Handle::generate(), Action::Check, actor())
            .unwrap();
        let failed = original
            .finish_job(
                check.id,
                State::Failed {
                    reason: Failure::CheckFailed,
                },
            )
            .unwrap();
        drop(original);
        let restarted = session(root.path(), workspace);
        let recovered = restarted.recover_jobs().unwrap();
        assert_eq!(recovered.len(), 2);
        let recovered = recovered.iter().find(|job| job.id == capture.id).unwrap();
        assert_eq!(recovered.id, capture.id);
        assert_eq!(recovered.handle, capture.handle);
        assert_eq!(recovered.created_at, capture.created_at);
        assert!(matches!(
            recovered.state,
            State::Failed {
                reason: Failure::Interrupted
            }
        ));
        assert_eq!(
            serde_json::to_value(restarted.job(check.id).unwrap()).unwrap(),
            serde_json::to_value(failed).unwrap()
        );
        assert_eq!(restarted.recover_jobs().unwrap().len(), 2);
        assert!(restarted
            .finish_job(
                capture.id,
                State::Failed {
                    reason: Failure::Interrupted
                }
            )
            .is_err());
    }

    #[test]
    fn recovery_filters_external_jobs_by_durable_generation_not_clock() {
        let root = tempfile::tempdir().unwrap();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let owner = session(root.path(), "example");
        let legacy = owner
            .create_job(Handle::generate(), Action::Capture, actor())
            .unwrap();
        drop(owner);
        let owner = session(root.path(), "example").with_restore_generation(Some(first));
        let prior = owner
            .create_job(Handle::generate(), Action::Check, actor())
            .unwrap();
        drop(owner);
        let owner = session(root.path(), "example").with_restore_generation(Some(second));
        let current = owner
            .create_job(Handle::generate(), Action::Capture, actor())
            .unwrap();
        // Equal timestamps and a future-dated old job cannot confer log membership.
        for mut job in [legacy.clone(), prior.clone(), current.clone()] {
            job.created_at = Utc::now() + chrono::Duration::days(1);
            store::write_json(
                &root.path().join("jobs").join(format!("{}.json", job.id)),
                &job,
            )
            .unwrap();
        }
        for _ in 0..2 {
            let recovered = owner.recover_jobs().unwrap();
            assert_eq!(recovered.len(), 1);
            assert_eq!(recovered[0].id, current.id);
            assert_eq!(recovered[0].restore_generation, Some(second));
        }
        for job in [legacy, prior] {
            assert!(matches!(
                owner.job(job.id).unwrap().state,
                State::Failed {
                    reason: Failure::Interrupted
                }
            ));
        }
        // Older job records decode as the original, pre-restore generation.
        let mut value = serde_json::to_value(current).unwrap();
        value.as_object_mut().unwrap().remove("restore_generation");
        assert_eq!(
            serde_json::from_value::<Job>(value)
                .unwrap()
                .restore_generation,
            None
        );
    }

    #[test]
    fn refuses_wrong_workspace_identity_transitions_and_raw_failures() {
        let root = tempfile::tempdir().unwrap();
        let workspace = "demo-workspace";
        let owner = session(root.path(), workspace);
        let job = owner
            .create_job(Handle::generate(), Action::Check, actor())
            .unwrap();
        assert!(owner.finish_job(job.id, State::Running).is_err());
        assert!(owner
            .finish_job(
                job.id,
                State::Failed {
                    reason: Failure::CaptureFailed
                }
            )
            .is_err());
        assert!(serde_json::from_value::<State>(
            serde_json::json!({"phase":"failed", "reason":"postgres://secret"})
        )
        .is_err());
        assert!(get(root.path(), "foreign-workspace", job.id).is_err());
        let mut changed = job.clone();
        changed.id = Uuid::new_v4();
        store::write_json(
            &root.path().join("jobs").join(format!("{}.json", job.id)),
            &changed,
        )
        .unwrap();
        assert_eq!(
            owner.job(job.id).unwrap_err().to_string(),
            "snapshot_job_unavailable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn records_are_private_and_symlinks_are_refused() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = tempfile::tempdir().unwrap();
        let owner = session(root.path(), "demo-workspace");
        let job = owner
            .create_job(Handle::generate(), Action::Capture, actor())
            .unwrap();
        let dir = root.path().join("jobs");
        let path = dir.join(format!("{}.json", job.id));
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let other = tempfile::NamedTempFile::new().unwrap();
        std::fs::remove_file(&path).unwrap();
        symlink(other.path(), &path).unwrap();
        assert!(owner.job(job.id).is_err());
        assert!(owner.recover_jobs().unwrap().is_empty());
    }

    /// One bad record must not stop the engine boot, which runs this settle.
    #[test]
    fn recovery_skips_unreadable_and_foreign_records() {
        let root = tempfile::tempdir().unwrap();
        let foreign = session(root.path(), "other-workspace")
            .create_job(Handle::generate(), Action::Capture, actor())
            .unwrap();
        let owner = session(root.path(), "demo-workspace");
        let good = owner
            .create_job(Handle::generate(), Action::Capture, actor())
            .unwrap();
        let jobs = root.path().join("jobs");
        std::fs::write(jobs.join(format!("{}.json", Uuid::new_v4())), b"{not json").unwrap();
        let recovered = owner.recover_jobs().unwrap();
        assert_eq!(
            recovered.iter().map(|job| job.id).collect::<Vec<_>>(),
            [good.id]
        );
        assert!(owner.job(foreign.id).is_err());
    }
}
