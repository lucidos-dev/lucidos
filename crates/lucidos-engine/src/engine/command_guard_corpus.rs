//! Shared synthetic inputs for the deterministic and model command guard checks.

use serde::Deserialize;
use serde_json::json;
use std::path::Path;

use super::command_guard::{
    static_classify_with_paths, CommandSite, JudgeInput, SecretPaths, StaticVerdict, TrustedHeads,
};
use crate::llm::tool_names as tn;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardCase {
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub python: bool,
    pub lane: String,
    pub category: Option<String>,
    #[serde(default = "model_default")]
    pub model: bool,
}

fn model_default() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardCorpus {
    pub command: Vec<GuardCase>,
}

impl GuardCase {
    pub(crate) fn tool_input(&self) -> (&'static str, serde_json::Value) {
        if self.python {
            (tn::RUN_PYTHON, json!({"code": self.command}))
        } else {
            (tn::RUN_BASH, json!({"command": self.command}))
        }
    }

    pub(crate) fn static_verdict(&self, root: &Path) -> StaticVerdict {
        let (tool, input) = self.tool_input();
        // Nominal shipped boundary; the deterministic tier separately covers
        // fallback modes and kernels without truncation protection.
        let paths = SecretPaths::for_policy_with_truncation(
            &crate::runtime::agent_sandbox::policy_for(root),
            Some(crate::runtime::agent_sandbox::SandboxMode::Enforced),
            true,
        );
        static_classify_with_paths(
            tool,
            &input,
            CommandSite::TextOnly { root: Some(root) },
            &TrustedHeads::NONE,
            &paths,
        )
    }

    /// Both tiers use the real static pass, including its normalized command.
    pub fn judge_command(&self, root: &Path) -> (String, bool) {
        match self.static_verdict(root) {
            StaticVerdict::NeedsJudge(JudgeInput {
                command,
                out_of_workspace,
                ..
            }) => (command, out_of_workspace),
            _ => (self.command.clone(), false),
        }
    }
}
