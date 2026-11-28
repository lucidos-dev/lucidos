use crate::runtime::agent_runtime::SpawnArgs;
use crate::runtime::agent_sandbox::Sandbox;
use crate::runtime::{AccountPin, CodingAgent};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Clone)]
pub struct SignInContext {
    pub agent: CodingAgent,
    pub cwd: PathBuf,
    pub user_env: Vec<(String, String)>,
    pub binary_override: Option<String>,
    pub account_pin: Option<AccountPin>,
    pub sandbox: Sandbox,
}

impl SignInContext {
    pub fn capture(agent: CodingAgent, args: &SpawnArgs<'_>, pin: Option<AccountPin>) -> Self {
        Self {
            agent,
            cwd: args.worktree_path.to_path_buf(),
            user_env: args.user_env_vars.to_vec(),
            binary_override: args.binary_override.map(str::to_owned),
            account_pin: pin,
            sandbox: args.sandbox.clone(),
        }
    }

    pub fn spawn_args<'a>(&'a self, workspace: &'a Path, thread_id: Uuid) -> SpawnArgs<'a> {
        SpawnArgs {
            worktree_path: &self.cwd,
            workspace_path: workspace,
            thread_id,
            coding_agent_kind: Default::default(),
            allowed_tools: None,
            system_prompt: None,
            session_context: None,
            resume_session_id: None,
            model: None,
            reasoning_effort: None,
            spawning_event_id: None,
            repo_name: None,
            interactive: false,
            build_slot_class: lucidos_build_slot::SlotClass::Ordinary,
            user_env_vars: &self.user_env,
            account_pin: self.account_pin.as_ref(),
            binary_override: self.binary_override.as_deref(),
            permission_mode: None,
            additional_directories: &[],
            auto_compact: None,
            keep_alive: false,
            agent_run: crate::runtime::agent_run_marker::AgentRun::unrecorded(),
            sandbox: self.sandbox.clone(),
        }
    }
}

/// The profile each recent coding-agent thread last spawned with, so a
/// sign-in suggestion and a thread terminal resolve the same CLI and profile.
#[derive(Default)]
pub struct SignInContexts {
    contexts: Mutex<HashMap<Uuid, SignInContext>>,
    posted: PostedSignIns,
}

impl SignInContexts {
    // Keeps pre-init failures actionable even if the CLI has already exited
    // and no SessionStarted event was emitted. Live sessions keep their own
    // snapshot. Durable threads can reconstruct their pin on engine restart.
    pub fn remember(&self, thread_id: Uuid, context: SignInContext) {
        let mut contexts = self.contexts.lock().unwrap();
        if contexts.len() >= 128 && !contexts.contains_key(&thread_id) {
            if let Some(evicted) = contexts.keys().next().copied() {
                contexts.remove(&evicted);
            }
        }
        contexts.insert(thread_id, context);
    }

    pub fn remembered(&self, thread_id: Uuid) -> Option<SignInContext> {
        self.contexts.lock().unwrap().get(&thread_id).cloned()
    }

    pub fn posted(&self) -> PostedSignIns {
        self.posted.clone()
    }
}

/// The sign-in commands posted as terminal suggestions, per thread. Typing
/// one of them, and only one of them, marks the session for a respawn.
#[derive(Clone, Default)]
pub struct PostedSignIns(Arc<Mutex<HashMap<Uuid, HashSet<String>>>>);

impl PostedSignIns {
    pub fn record(&self, thread_id: Uuid, command: &str) {
        let mut posted = self.0.lock().unwrap();
        if posted.len() >= 128 && !posted.contains_key(&thread_id) {
            if let Some(evicted) = posted.keys().next().copied() {
                posted.remove(&evicted);
            }
        }
        posted
            .entry(thread_id)
            .or_default()
            .insert(command.to_owned());
    }

    pub fn is_sign_in(&self, thread_id: Uuid, command: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .get(&thread_id)
            .is_some_and(|commands| commands.contains(command))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_posted_sign_in_command_counts_as_one() {
        let posted = PostedSignIns::default();
        let thread = Uuid::new_v4();
        posted.record(thread, "claude auth login");
        assert!(posted.is_sign_in(thread, "claude auth login"));
        assert!(!posted.is_sign_in(thread, "npm test"));
        assert!(!posted.is_sign_in(Uuid::new_v4(), "claude auth login"));
    }
}
