use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TriggerFolderGrant {
    pub folder: PathBuf,
    device: u64,
    inode: u64,
}

impl TriggerFolderGrant {
    pub fn inspect(folder: &Path, workspace: &Path) -> Option<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let folder = folder.canonicalize().ok()?;
            let workspace = workspace.canonicalize().ok()?;
            if folder.starts_with(&workspace) || workspace.starts_with(&folder) {
                return None;
            }
            let policy = super::agent_sandbox::policy_for(&workspace).resolved();
            if policy.hides_resolved(&folder)
                || policy
                    .private
                    .iter()
                    .chain(&policy.hidden)
                    .chain(&policy.read_only)
                    .any(|root| root.starts_with(&folder) || folder.starts_with(root))
            {
                return None;
            }
            let meta = folder.metadata().ok()?;
            meta.is_dir().then(|| Self {
                folder,
                device: meta.dev(),
                inode: meta.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (folder, workspace);
            None
        }
    }

    pub fn still_valid(&self, workspace: &Path) -> bool {
        Self::inspect(&self.folder, workspace).as_ref() == Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_folder_loses_its_grant() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let folder = temp.path().join("outside");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&folder).unwrap();
        let grant = TriggerFolderGrant::inspect(&folder, &workspace).unwrap();
        assert!(grant.still_valid(&workspace));
        std::fs::rename(&folder, temp.path().join("original")).unwrap();
        std::fs::create_dir(&folder).unwrap();
        assert!(!grant.still_valid(&workspace));
    }

    struct Outside {
        _temp: tempfile::TempDir,
        workspace: PathBuf,
        folder: PathBuf,
        sandbox: crate::core::agent_sandbox::Sandbox,
    }

    fn outside() -> Outside {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let folder = temp.path().join("outside");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&folder).unwrap();
        let sandbox =
            crate::core::agent_sandbox::Sandbox::enforced_for_test(lucidos_agent_sandbox::Policy {
                write_scope: Some(vec![workspace.canonicalize().unwrap()]),
                ..Default::default()
            });
        Outside {
            _temp: temp,
            workspace,
            folder: folder.canonicalize().unwrap(),
            sandbox,
        }
    }

    fn refused(outside: &Outside, path: &Path) -> Option<TriggerFolderGrant> {
        let diagnostic = format!("sh: {}: Permission denied", path.display());
        folder_refusal(&outside.sandbox, &outside.workspace, &diagnostic)
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn a_folder_the_engine_can_write_gets_a_card() {
        let outside = outside();
        let grant = refused(&outside, &outside.folder.join("new")).unwrap();
        assert_eq!(grant.folder, outside.folder);
        let existing = outside.folder.join("existing");
        std::fs::write(&existing, "keep").unwrap();
        assert_eq!(refused(&outside, &existing), Some(grant));
        assert_eq!(refused(&outside, &outside.workspace.join("new")), None);
    }

    #[cfg(unix)]
    #[test]
    fn refusals_the_engine_shares_get_no_card() {
        // SAFETY: `geteuid` reads the caller's id and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let outside = outside();
        let read_only_file = outside.folder.join("read-only");
        std::fs::write(&read_only_file, "keep").unwrap();
        set_mode(&read_only_file, 0o444);
        assert_eq!(refused(&outside, &read_only_file), None);

        let unreadable = outside.folder.join("unreadable");
        std::fs::write(&unreadable, "keep").unwrap();
        set_mode(&unreadable, 0o200);
        assert_eq!(refused(&outside, &unreadable), None);

        let locked = outside.folder.join("locked");
        std::fs::create_dir(&locked).unwrap();
        set_mode(&locked, 0o555);
        assert_eq!(refused(&outside, &locked.join("new")), None);
        set_mode(&locked, 0o755);
    }

    #[test]
    fn a_script_refusal_keeps_the_script_output() {
        let outside = outside();
        let grant = refused(&outside, &outside.folder.join("new")).unwrap();
        let refusal = ScriptFolderRefusal {
            grant,
            output: "Shell script error (exit 1):\nprogress line".to_string(),
        };
        let text = refusal.to_string();
        assert!(text.starts_with("Shell script error (exit 1):\nprogress line"));
        assert!(text.ends_with(&folder_refusal_reason(&refusal.grant)));
    }
}

pub fn folder_refusal(
    sandbox: &super::agent_sandbox::Sandbox,
    workspace: &Path,
    diagnostic: &str,
) -> Option<TriggerFolderGrant> {
    if !sandbox.has_trigger_write_scope() {
        return None;
    }
    let policy = sandbox.policy_for_execution(workspace).resolved();
    let roots = policy.write_scope.as_ref()?;
    let path = lucidos_agent_sandbox::write_refusal::refused_path(diagnostic)?;
    if policy.hides_resolved(path) {
        return None;
    }
    let existing = path
        .ancestors()
        .find_map(|ancestor| ancestor.canonicalize().ok())?;
    let folder = if existing.is_dir() {
        existing.as_path()
    } else {
        existing.parent()?
    };
    if roots.iter().any(|root| folder.starts_with(root)) {
        return None;
    }
    let engine_may_write =
        engine_can_write(folder) && (existing.is_dir() || engine_can_write(&existing));
    if !engine_may_write {
        return None;
    }
    TriggerFolderGrant::inspect(folder, workspace)
}

fn engine_can_write(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(meta) = path.metadata() else {
            return false;
        };
        let (opens, mode) = if meta.is_dir() {
            (std::fs::read_dir(path).is_ok(), libc::W_OK | libc::X_OK)
        } else if meta.is_file() {
            (std::fs::File::open(path).is_ok(), libc::W_OK)
        } else {
            return false;
        };
        opens
            && std::ffi::CString::new(path.as_os_str().as_bytes())
                // SAFETY: `access` reads a NUL-terminated path and keeps no pointer.
                .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), mode) } == 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

#[derive(Debug)]
pub struct ScriptFolderRefusal {
    pub grant: TriggerFolderGrant,
    pub output: String,
}

impl std::fmt::Display for ScriptFolderRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}\n\n{}",
            self.output,
            folder_refusal_reason(&self.grant)
        )
    }
}

pub fn script_failure(
    sandbox: &super::agent_sandbox::Sandbox,
    workspace: &Path,
    output: String,
) -> Box<dyn std::error::Error + Send + Sync> {
    match folder_refusal(sandbox, workspace, &output) {
        Some(grant) => Box::new(ScriptFolderRefusal { grant, output }),
        None => output.into(),
    }
}

pub fn folder_refusal_reason(grant: &TriggerFolderGrant) -> String {
    format!(
        "The trigger was refused a write in {}.",
        grant.folder.display()
    )
}

impl std::error::Error for ScriptFolderRefusal {}

pub fn script_refusal_thread_events(
    message: String,
    title: String,
    reason: &str,
) -> Vec<(
    crate::engine::thread_events::ThreadEvent,
    crate::engine::thread_events::EventMeta,
)> {
    use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};

    let request_event_id = uuid::Uuid::new_v4();
    let meta = EventMeta {
        channel: Some(EventChannel::Trigger),
        ..EventMeta::NONE
    };
    vec![
        (
            ThreadEvent::MessageReceived {
                text: message,
                user_image_hashes: vec![],
                device_id: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                wake_mode: None,
                mode: ActorMode::Engine,
                model: None,
                reasoning_effort: None,
                provider: None,
                origin: None,
                voice_session_id: None,
            },
            EventMeta {
                event_id: Some(request_event_id),
                ..meta.clone()
            },
        ),
        (ThreadEvent::ThreadTitleRenamed { title }, meta.clone()),
        (
            ThreadEvent::ResponseFailed {
                error: reason.into(),
                spend_cap: None,
                agent_exit: None,
                provider_error: None,
            },
            EventMeta {
                request_event_id: Some(request_event_id),
                ..meta
            },
        ),
    ]
}
