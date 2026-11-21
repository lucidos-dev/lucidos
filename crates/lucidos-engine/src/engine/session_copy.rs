//! Inputs a read-only copy of a coding-agent session starts with.

use std::path::PathBuf;

use uuid::Uuid;

/// Why a side question got no answer.
#[derive(Debug, PartialEq, Eq)]
pub enum SideQuestionFailure {
    /// The thread cannot take side questions. The text tells the user why.
    Refused(&'static str),
    /// The ask names an image this workspace never received.
    UnknownImage(String),
    /// A side question with this id is running or was answered. Only a
    /// failed one may be asked again.
    AlreadyAsked,
    /// Nothing asked with this id on the thread, so there is nothing to dismiss.
    NotAsked,
    /// The agent could not answer. The text says what went wrong.
    Failed(String),
}

/// What a session copy needs: the thread's session and the flags its main
/// session runs with. Side questions and the cache keep-alive ask one.
pub struct SessionCopy {
    pub cwd: PathBuf,
    pub session_id: String,
    /// What the session's latest spawn appended, when this engine saw it.
    pub system_prompt: Option<String>,
    pub account_pin: Option<crate::runtime::AccountPin>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub allowed_tools: String,
    pub user_env: Vec<(String, String)>,
    pub build_slot_class: lucidos_build_slot::SlotClass,
    pub binary_override: Option<String>,
    pub permission_mode: Option<String>,
    pub additional_directories: Vec<PathBuf>,
    pub sandbox: crate::runtime::agent_sandbox::Sandbox,
}

impl SessionCopy {
    /// Whether the copy carries what the session's spawn appended. Without it
    /// the copy's prefix differs from the session's, so it misses the cache.
    pub(crate) fn shares_the_prefix(&self) -> bool {
        self.system_prompt.is_some()
    }

    /// The copy's spawn: the session's own flags, resuming its session.
    pub fn spawn_args<'a>(
        &'a self,
        workspace_path: &'a std::path::Path,
        thread_id: Uuid,
    ) -> crate::runtime::SpawnArgs<'a> {
        crate::runtime::SpawnArgs {
            worktree_path: &self.cwd,
            coding_agent_kind: Default::default(),
            workspace_path,
            allowed_tools: Some(&self.allowed_tools),
            system_prompt: self.system_prompt.as_deref(),
            session_context: None,
            resume_session_id: Some(&self.session_id),
            model: self.model.as_deref(),
            reasoning_effort: self.effort.as_deref(),
            thread_id,
            spawning_event_id: None,
            repo_name: None,
            interactive: false,
            build_slot_class: self.build_slot_class,
            user_env_vars: &self.user_env,
            account_pin: self.account_pin.as_ref(),
            binary_override: self.binary_override.as_deref(),
            permission_mode: self.permission_mode.as_deref(),
            additional_directories: &self.additional_directories,
            auto_compact: None,
            keep_alive: false,
            agent_run: crate::runtime::agent_run_marker::AgentRun::unrecorded(),
            sandbox: self.sandbox.clone(),
        }
    }

    /// The model to price a reply at: the one its copy named, else the
    /// session's own.
    pub fn model_of(&self, reply: &crate::runtime::claude_code::SessionCopyReply) -> String {
        reply
            .model
            .clone()
            .or_else(|| self.model.clone())
            .unwrap_or_else(|| "unknown".to_string())
    }

    pub(crate) fn effort(&self) -> Option<&str> {
        self.effort.as_deref()
    }
}
