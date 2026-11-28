//! Where a thread terminal's shell starts, and with which agent profile
//! (ADR 0820 decisions 2 and 3).
//!
//! INVARIANT: A chat thread's shell starts in `<workspace>/data`. A
//! coding-agent thread's shell starts in its worktree, with the profile
//! folders its agent resolves.

use std::ffi::OsString;
use std::path::PathBuf;

use uuid::Uuid;

use crate::engine::agent_sign_in::{self, ContextError};
use crate::engine::sign_in_contexts::SignInContext;
use crate::engine::LucidosEngine;
use crate::runtime::CodingAgent;

#[derive(Debug)]
pub(crate) struct TerminalFolder {
    pub cwd: PathBuf,
    pub profile: Vec<(OsString, OsString)>,
}

#[derive(Debug)]
pub(crate) enum FolderError {
    NoSuchThread,
    Archived,
    NoWorkingFolder,
    Failed(String),
}

pub(crate) async fn resolve(
    engine: &LucidosEngine,
    thread_id: Uuid,
    user_env: &[(String, String)],
) -> Result<TerminalFolder, FolderError> {
    let row: Option<(bool, Option<String>, String)> = sqlx::query_as(
        "SELECT is_coding_agent, coding_agent, archive_state FROM thread_summaries \
         WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(engine.pool())
    .await
    .map_err(|e| FolderError::Failed(e.to_string()))?;
    let (is_coding_agent, agent, archive_state) = row.ok_or(FolderError::NoSuchThread)?;
    if archive_state == "archived" {
        return Err(FolderError::Archived);
    }
    if !is_coding_agent {
        return Ok(TerminalFolder {
            cwd: engine.workspace_path().join("data"),
            profile: Vec::new(),
        });
    }
    let agent = agent
        .map(|agent| CodingAgent::parse(&agent))
        .unwrap_or(CodingAgent::ClaudeCode);
    coding_agent_folder(engine, thread_id, agent, user_env).await
}

async fn coding_agent_folder(
    engine: &LucidosEngine,
    thread_id: Uuid,
    agent: CodingAgent,
    user_env: &[(String, String)],
) -> Result<TerminalFolder, FolderError> {
    let context = match agent_sign_in::sign_in_context(engine, thread_id, agent).await {
        Ok(context) => context,
        Err(ContextError::ThreadNotFound) => return Err(FolderError::NoSuchThread),
        Err(ContextError::Database(e)) => return Err(FolderError::Failed(e.to_string())),
        Err(ContextError::WrongAgent | ContextError::Unavailable) => {
            durable_context(engine, thread_id, agent, user_env).await?
        }
    };
    let args = context.spawn_args(engine.workspace_path(), thread_id);
    let profile = crate::runtime::agent_profile::profile_env(agent, &args)
        .map_err(|e| FolderError::Failed(e.to_string()))?;
    Ok(TerminalFolder {
        cwd: context.cwd.clone(),
        profile,
    })
}

/// The context the agent's next spawn would build, for a thread that has not
/// run since the engine started.
async fn durable_context(
    engine: &LucidosEngine,
    thread_id: Uuid,
    agent: CodingAgent,
    user_env: &[(String, String)],
) -> Result<SignInContext, FolderError> {
    let failed = |e: &dyn std::fmt::Display| FolderError::Failed(e.to_string());
    let cwd = agent_sign_in::thread_cwd(engine, thread_id)
        .await
        .map_err(|e| failed(&e))?
        .ok_or(FolderError::NoWorkingFolder)?;
    let account_pin = match agent {
        CodingAgent::ClaudeCode => {
            crate::engine::agent_session::lookup_pinned_cc_config_dir(engine.pool(), thread_id)
                .await
                .map_err(|e| failed(&e))?
                .map(|pin| pin.resolve(user_env))
        }
        CodingAgent::Codex => None,
    };
    let sandbox =
        crate::runtime::agent_sandbox::Sandbox::resolve(engine.pool(), engine.workspace_path())
            .await
            .map_err(|e| failed(&e))?;
    Ok(SignInContext {
        agent,
        cwd,
        user_env: user_env.to_vec(),
        binary_override: agent_sign_in::binary_pref(agent).read(engine.pool()).await,
        account_pin,
        sandbox,
    })
}
