use std::path::PathBuf;

use uuid::Uuid;

#[derive(Debug, PartialEq, Eq)]
pub enum SideQuestionFailure {
    Refused(&'static str),
    UnknownImage(String),
    AlreadyAsked,
    NotAsked,
    Failed(String),
}

pub struct SessionCopy {
    pub cwd: PathBuf,
    pub session_id: String,
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
    pub(crate) fn shares_the_prefix(&self) -> bool {
        self.system_prompt.is_some()
    }

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
