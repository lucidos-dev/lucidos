//! Gateway-owned, immutable cleanup events, independent of workspace engines
//! and Trash retention. Authorization is durable before any destructive effect.
use serde::{Deserialize, Serialize};
use std::path::Path;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum Actor {
    Device {
        device_id: String,
    },
    /// Proof was verified against this workspace's ephemeral owner digest.
    /// Never retain the bearer token itself.
    Owner {
        workspace_id: String,
    },
    /// An agent thread through the workspace broker, after its engine spent
    /// the owner's Allow once.
    Agent {
        workspace_id: String,
        thread_id: String,
    },
}

#[derive(Serialize)]
struct Event<'a> {
    event_type: &'a str,
    operation: &'a str,
    resource: &'a str,
    actor: &'a Actor,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Audit {
    operation: String,
    resource: String,
    actor: Actor,
}

impl Audit {
    pub(crate) fn begin(home: &Path, resource: &str, actor: Actor) -> Result<Self, BoxError> {
        let audit = Self {
            operation: lucidos_local_token::mint_hex(16)?,
            resource: resource.into(),
            actor,
        };
        audit.emit(home, "WorkspaceDeletionAuthorized")?;
        Ok(audit)
    }

    pub(crate) fn require_authorized(&self, home: &Path, resource: &str) -> Result<(), BoxError> {
        if self.resource != resource || !self.existing_event(home, "WorkspaceDeletionAuthorized")? {
            return Err("cleanup journal authorization mismatch or missing".into());
        }
        Ok(())
    }

    pub(crate) fn completed(&self, home: &Path) -> Result<(), BoxError> {
        self.emit(home, "WorkspaceDeletionAuthorized")?;
        self.emit(home, "WorkspaceDeleted")
    }

    pub(crate) fn matches(&self, resource: &str, actor: &Actor) -> bool {
        self.resource == resource && &self.actor == actor
    }

    pub(crate) fn is_completed(&self, home: &Path) -> Result<bool, BoxError> {
        if !self.existing_event(home, "WorkspaceDeleted")? {
            return Ok(false);
        }
        if !self.existing_event(home, "WorkspaceDeletionAuthorized")? {
            return Err("cleanup completion has no authorization".into());
        }
        Ok(true)
    }

    fn existing_event(&self, home: &Path, event_type: &str) -> Result<bool, BoxError> {
        use std::os::unix::fs::OpenOptionsExt;
        if self.operation.len() != 32 || !self.operation.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid cleanup audit operation".into());
        }
        let target = home
            .join("cleanup-events")
            .join(format!("{}-{event_type}.json", self.operation));
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(target)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > 1024 * 1024 {
            return Err("invalid existing cleanup audit event".into());
        }
        let existing: serde_json::Value = serde_json::from_reader(&file)?;
        if existing["event_type"] != event_type
            || existing["operation"] != self.operation
            || existing["resource"] != self.resource
            || existing["actor"] != serde_json::to_value(&self.actor)?
        {
            return Err("cleanup audit event identity mismatch".into());
        }
        file.sync_all()?;
        Ok(true)
    }

    fn emit(&self, home: &Path, event_type: &str) -> Result<(), BoxError> {
        use std::os::unix::fs::DirBuilderExt;
        if self.operation.len() != 32 || !self.operation.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid cleanup audit operation".into());
        }
        let root = home.join("cleanup-events");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&root)?;
        let mut file = tempfile::NamedTempFile::new_in(&root)?;
        serde_json::to_writer(
            file.as_file_mut(),
            &Event {
                event_type,
                operation: &self.operation,
                resource: &self.resource,
                actor: &self.actor,
                created_at: chrono::Utc::now(),
            },
        )?;
        file.as_file().sync_all()?;
        let target = root.join(format!("{}-{event_type}.json", self.operation));
        match file.persist_noclobber(&target) {
            Ok(_) => {}
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
                if !self.existing_event(home, event_type)? {
                    return Err("cleanup audit event disappeared".into());
                }
            }
            Err(e) => return Err(e.into()),
        }
        std::fs::File::open(root)?.sync_all()?;
        std::fs::File::open(home)?.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_requires_original_resource_and_durable_authorization() {
        let temp = tempfile::tempdir().unwrap();
        let audit = Audit::begin(
            temp.path(),
            "scratch",
            Actor::Owner {
                workspace_id: "scratch".into(),
            },
        )
        .unwrap();
        audit.require_authorized(temp.path(), "scratch").unwrap();
        assert!(audit.require_authorized(temp.path(), "another").is_err());
        let other = tempfile::tempdir().unwrap();
        assert!(audit.require_authorized(other.path(), "scratch").is_err());
    }

    #[test]
    fn durable_events_survive_trash_removal_without_recording_bearer_secrets() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let trash = temp.path().join("deleted");
        std::fs::create_dir(&trash).unwrap();
        let audit = Audit::begin(
            temp.path(),
            "scratch",
            Actor::Owner {
                workspace_id: "scratch".into(),
            },
        )
        .unwrap();
        audit.completed(temp.path()).unwrap();
        std::fs::remove_dir(&trash).unwrap();
        let events: Vec<_> = std::fs::read_dir(temp.path().join("cleanup-events"))
            .unwrap()
            .map(|f| {
                let path = f.unwrap().path();
                assert_eq!(
                    std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
                serde_json::from_slice::<serde_json::Value>(&std::fs::read(path).unwrap()).unwrap()
            })
            .collect();
        assert_eq!(events.len(), 2);
        assert!(events.iter().any(|e| e["event_type"] == "WorkspaceDeleted"));
        assert!(events
            .iter()
            .all(|e| e["actor"] == serde_json::json!({"type":"owner", "workspace_id":"scratch"})));
        assert_eq!(events[0]["operation"], events[1]["operation"]);
    }

    #[test]
    fn replay_is_idempotent_but_conflicting_identity_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let audit = Audit::begin(
            temp.path(),
            "scratch",
            Actor::Device {
                device_id: "first-device".into(),
            },
        )
        .unwrap();
        assert!(!audit.is_completed(temp.path()).unwrap());
        audit.completed(temp.path()).unwrap();
        assert!(audit.is_completed(temp.path()).unwrap());
        let path = temp
            .path()
            .join("cleanup-events")
            .join(format!("{}-WorkspaceDeleted.json", audit.operation));
        let original = std::fs::read(&path).unwrap();
        let recovered: Audit =
            serde_json::from_slice(&serde_json::to_vec(&audit).unwrap()).unwrap();
        recovered.completed(temp.path()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let mut changed = recovered;
        changed.actor = Actor::Device {
            device_id: "second-device".into(),
        };
        assert!(changed.is_completed(temp.path()).is_err());
        assert!(changed.completed(temp.path()).is_err());
    }

    #[test]
    fn completion_lookup_refuses_symlinks_and_missing_authorization() {
        let temp = tempfile::tempdir().unwrap();
        let audit = Audit::begin(
            temp.path(),
            "scratch",
            Actor::Device {
                device_id: "device".into(),
            },
        )
        .unwrap();
        audit.completed(temp.path()).unwrap();
        let root = temp.path().join("cleanup-events");
        let completion = root.join(format!("{}-WorkspaceDeleted.json", audit.operation));
        let saved = temp.path().join("saved.json");
        std::fs::rename(&completion, &saved).unwrap();
        std::os::unix::fs::symlink(&saved, &completion).unwrap();
        assert!(audit.is_completed(temp.path()).is_err());
        std::fs::remove_file(&completion).unwrap();
        std::fs::rename(saved, completion).unwrap();
        std::fs::remove_file(root.join(format!(
            "{}-WorkspaceDeletionAuthorized.json",
            audit.operation
        )))
        .unwrap();
        assert!(audit.is_completed(temp.path()).is_err());
    }

    #[test]
    fn unavailable_audit_storage_refuses_authorization() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("cleanup-events"), b"occupied").unwrap();
        assert!(Audit::begin(
            temp.path(),
            "scratch",
            Actor::Device {
                device_id: "test-device".into(),
            }
        )
        .is_err());
    }
}
