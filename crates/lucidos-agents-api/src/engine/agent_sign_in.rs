use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::sign_in_contexts::{PostedSignIns, SignInContext};
use crate::engine::terminal::suggestion;
use crate::engine::thread_events::{
    AgentSignInTarget, CodingAgentCommand, CodingAgentMcpServerStatus, EventMeta, ThreadEvent,
};
use crate::engine::{LucidosEngine, SharedEngine};
use crate::runtime::agent_runtime::McpServerStatus;
use crate::runtime::agent_sandbox::SandboxedCommand;
use crate::runtime::agent_sign_in::{self, SignInReason};
use crate::runtime::CodingAgent;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use uuid::Uuid;

const IN_TERMINAL: &str = "Run the sign-in command below in the thread terminal.";

pub(crate) enum ContextError {
    ThreadNotFound,
    WrongAgent,
    Unavailable,
    Database(sqlx::Error),
}

/// Where a sign-in suggestion goes: the thread's events, and the registry
/// that later recognises the typed command.
#[derive(Clone, Copy)]
pub(crate) struct SuggestionSink<'a> {
    pub bus: &'a EventBus,
    pub posted: &'a PostedSignIns,
}

/// Which MCP servers a session still owes a sign-in suggestion, at most one
/// per server per session (ADR 0823). For Claude Code a status report only
/// updates the list, and a tool call on a listed server posts. Codex hides a
/// signed-out server's tools, so its report posts.
#[derive(Debug)]
pub(crate) struct McpSignInWatch {
    agent: CodingAgent,
    servers: Vec<McpServerStatus>,
    posted: HashSet<String>,
    /// `None` once a report has arrived. Before that, the MCP tool calls the
    /// first report must still judge: a turn can start before it.
    calls_before_report: Option<Vec<String>>,
}

impl McpSignInWatch {
    pub(crate) fn new(agent: CodingAgent) -> Self {
        Self {
            agent,
            servers: Vec::new(),
            posted: HashSet::new(),
            calls_before_report: Some(Vec::new()),
        }
    }

    /// Replace the servers with the latest report's. Returns the servers it
    /// now owes a suggestion, each with its reason.
    pub(crate) fn report(&mut self, servers: &[McpServerStatus]) -> Vec<(String, SignInReason)> {
        self.servers = servers.to_vec();
        let earlier = self.calls_before_report.take().unwrap_or_default();
        let mut owed: Vec<_> = earlier
            .iter()
            .filter_map(|tool| self.take_tool_call(tool))
            .map(|server| (server, SignInReason::ToolCall))
            .collect();
        if self.agent == CodingAgent::Codex {
            for server in self.servers.iter().filter(|server| server.needs_sign_in) {
                if self.posted.insert(server.name.clone()) {
                    owed.push((server.name.clone(), SignInReason::Reported));
                }
            }
        }
        owed
    }

    /// The server `tool` calls, when it needs a sign-in no suggestion has
    /// covered yet this session. Marks it covered. The longest matching name
    /// wins before the sign-in check, so a call on a connected `calendar__work`
    /// is never taken for a signed-out `calendar`.
    pub(crate) fn take_tool_call(&mut self, tool: &str) -> Option<String> {
        if let Some(earlier) = self.calls_before_report.as_mut() {
            if tool.starts_with("mcp__") {
                earlier.push(tool.to_string());
            }
            return None;
        }
        let server = self
            .servers
            .iter()
            .filter(|server| agent_sign_in::calls_mcp_server(tool, &server.name))
            .max_by_key(|server| server.name.len())
            .filter(|server| server.needs_sign_in)?;
        self.posted
            .insert(server.name.clone())
            .then(|| server.name.clone())
    }
}

/// What a sign-in suggestion offers, and why.
#[derive(Clone, Copy)]
pub(crate) struct SignInOffer<'a> {
    pub agent: CodingAgent,
    pub target: &'a AgentSignInTarget,
    pub reason: SignInReason,
    /// A Claude model login goes through an organisation's gateway.
    pub gateway: bool,
}

/// Post the CLI's own sign-in command as a terminal suggestion (ADR 0820).
/// Logs and returns `false` when the target has no command or the post fails.
pub(crate) async fn post_suggestion(
    sink: SuggestionSink<'_>,
    thread_id: Uuid,
    offer: SignInOffer<'_>,
    binary_override: Option<&str>,
    meta: EventMeta,
) -> bool {
    let SignInOffer {
        agent,
        target,
        reason,
        gateway,
    } = offer;
    let Some(built) = agent_sign_in::suggestion(agent, target, reason, gateway, binary_override)
    else {
        crate::log!(
            "[AgentSignIn] No sign-in command for {} on {}",
            lucidos_diagnostic_log::private(agent.as_str().to_string()),
            thread_id
        );
        return false;
    };
    match suggestion::post(
        sink.bus,
        thread_id,
        &built.command,
        Some(&built.label),
        meta,
    )
    .await
    {
        Ok(_) => {
            sink.posted.record(thread_id, &built.command);
            true
        }
        Err(error) => {
            crate::log!(
                "[AgentSignIn] Could not post the sign-in suggestion on {}: {}",
                thread_id,
                lucidos_diagnostic_log::private(format!("{error:?}"))
            );
            false
        }
    }
}

pub(crate) async fn answer_command(
    engine: &SharedEngine,
    thread_id: Uuid,
    request_id: Uuid,
    command: CodingAgentCommand,
    agent: CodingAgent,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut sign_ins = Vec::new();
    let mut gateway = false;
    let (message, servers) = match command {
        CodingAgentCommand::Mcp => {
            let servers: Option<sqlx::types::Json<Vec<CodingAgentMcpServerStatus>>> =
                sqlx::query_scalar(
                    "SELECT payload->'servers' FROM events WHERE thread_id=$1
                 AND event_type='CodingAgentMcpStatusesReported' AND payload->>'agent'=$2
                 ORDER BY sequence DESC LIMIT 1",
                )
                .bind(thread_id)
                .bind(agent.as_str())
                .fetch_optional(engine.pool())
                .await?;
            let message = match servers.as_ref().map(|servers| servers.0.as_slice()) {
                None => "MCP server status is unavailable until the coding agent reports it.",
                Some([]) => "The coding agent reported no MCP servers.",
                Some(_) => "Last reported MCP servers",
            };
            sign_ins = servers
                .iter()
                .flat_map(|servers| servers.0.iter())
                .filter(|server| server.needs_sign_in)
                .map(|server| AgentSignInTarget::McpServer {
                    name: server.name.clone(),
                })
                .collect();
            (message, servers.map(|servers| servers.0))
        }
        CodingAgentCommand::Login => {
            let message = match agent {
                CodingAgent::Codex => "Codex model sign-in is available from the Codex CLI.",
                CodingAgent::ClaudeCode => match claude_login(engine, thread_id).await? {
                    ClaudeLogin::Provider => {
                        "This Claude profile requires sign-in through its configured provider."
                    }
                    login => {
                        gateway = login == ClaudeLogin::Gateway;
                        sign_ins.push(AgentSignInTarget::ModelLogin);
                        IN_TERMINAL
                    }
                },
            };
            (message, None)
        }
    };
    let meta = EventMeta {
        request_event_id: Some(request_id),
        ..EventMeta::NONE
    };
    engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentCommandAnswered {
                agent,
                message: message.into(),
                servers,
            },
            meta: meta.clone(),
        })
        .await?;
    if sign_ins.is_empty() {
        return Ok(());
    }
    let binary_override = binary_override(engine, thread_id, agent).await;
    for target in &sign_ins {
        post_suggestion(
            SuggestionSink {
                bus: &engine.event_bus,
                posted: &engine.sign_in_contexts.posted(),
            },
            thread_id,
            SignInOffer {
                agent,
                target,
                reason: SignInReason::Requested,
                gateway,
            },
            binary_override.as_deref(),
            meta.clone(),
        )
        .await;
    }
    Ok(())
}

#[derive(PartialEq, Eq)]
enum ClaudeLogin {
    Direct,
    Gateway,
    Provider,
}

/// How the thread's Claude profile signs in, from the CLI's own status. An
/// unknown status offers the direct login: the user sees what the CLI says.
async fn claude_login(engine: &LucidosEngine, thread_id: Uuid) -> Result<ClaudeLogin, sqlx::Error> {
    let context = match sign_in_context(engine, thread_id, CodingAgent::ClaudeCode).await {
        Ok(context) => context,
        Err(ContextError::Database(error)) => return Err(error),
        Err(_) => return Ok(ClaudeLogin::Direct),
    };
    let args = context.spawn_args(engine.workspace_path(), thread_id);
    let Ok(command) = crate::runtime::claude_code::auth_status_command(&args) else {
        return Ok(ClaudeLogin::Direct);
    };
    let Some(status) = claude_status(command).await else {
        return Ok(ClaudeLogin::Direct);
    };
    Ok(if status["forcedLoginMethod"] == "gateway" {
        ClaudeLogin::Gateway
    } else if status
        .get("apiProvider")
        .is_some_and(|provider| provider != "firstParty")
    {
        ClaudeLogin::Provider
    } else {
        ClaudeLogin::Direct
    })
}

/// The CLI path the thread's agent spawn last used, else the preference.
pub(crate) async fn binary_override(
    engine: &LucidosEngine,
    thread_id: Uuid,
    agent: CodingAgent,
) -> Option<String> {
    match remembered_context(engine, thread_id).await {
        Some(context) if context.agent == agent => context.binary_override,
        _ => binary_pref(agent).read(engine.pool()).await,
    }
}

pub(crate) fn binary_pref(
    agent: CodingAgent,
) -> &'static crate::core::prefs::Pref<crate::core::prefs::Optional> {
    match agent {
        CodingAgent::ClaudeCode => &crate::core::prefs::CODING_AGENT_CLAUDE_PATH,
        CodingAgent::Codex => &crate::core::prefs::CODING_AGENT_CODEX_PATH,
    }
}

async fn remembered_context(engine: &LucidosEngine, thread_id: Uuid) -> Option<SignInContext> {
    engine
        .agent_sessions
        .lock()
        .await
        .get(&thread_id)
        .and_then(|session| session.sign_in_context.clone())
        .or_else(|| engine.sign_in_contexts.remembered(thread_id))
}

pub(crate) async fn claude_status(mut command: SandboxedCommand) -> Option<serde_json::Value> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut child = command.spawn().ok()?;
        let mut output = Vec::new();
        child
            .stdout
            .take()?
            .take(32 * 1024)
            .read_to_end(&mut output)
            .await
            .ok()?;
        child.wait().await.ok()?;
        // Logged-out Claude exits 1. The structured JSON is authoritative.
        serde_json::from_slice(&output).ok()
    })
    .await
    .ok()
    .flatten()
}

pub(crate) async fn sign_in_context(
    engine: &LucidosEngine,
    thread_id: Uuid,
    agent: CodingAgent,
) -> Result<SignInContext, ContextError> {
    let (recorded_agent, folder, kind): (Option<String>, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT coding_agent, coding_agent_folder, coding_agent_kind FROM thread_summaries
         WHERE thread_id = $1 AND archive_state <> 'archived'",
        )
        .bind(thread_id)
        .fetch_optional(engine.pool())
        .await
        .map_err(ContextError::Database)?
        .ok_or(ContextError::ThreadNotFound)?;

    if let Some(context) = remembered_context(engine, thread_id).await {
        if context.agent != agent {
            return Err(ContextError::WrongAgent);
        }
        if !tokio::fs::metadata(&context.cwd)
            .await
            .is_ok_and(|m| m.is_dir())
        {
            return Err(ContextError::Unavailable);
        }
        return Ok(context);
    }
    if recorded_agent.as_deref() != Some(agent.as_str()) {
        return Err(ContextError::WrongAgent);
    }
    if agent == CodingAgent::Codex
        || crate::engine::session_queries::lookup_latest_session_id_for_agent(
            engine.pool(),
            thread_id,
            agent,
        )
        .await
        .is_none()
    {
        return Err(ContextError::Unavailable);
    }
    let workspace = engine.workspace_path();
    let folder = folder
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.to_path_buf());
    let cwd = resolved_cwd(engine, thread_id, &folder, kind.as_deref() == Some("app"))
        .await
        .ok_or(ContextError::Unavailable)?;
    let user_env =
        crate::core::EnvironmentVariableStore::spawn_pairs(engine.pool(), "AgentSignIn").await;
    let account_pin = {
        let pin =
            crate::engine::agent_session::lookup_pinned_cc_config_dir(engine.pool(), thread_id)
                .await
                .map_err(ContextError::Database)?
                .ok_or(ContextError::Unavailable)?;
        Some(pin.resolve(&user_env))
    };
    let sandbox = crate::runtime::agent_sandbox::Sandbox::resolve(engine.pool(), workspace)
        .await
        .map_err(|_| ContextError::Unavailable)?;
    Ok(SignInContext {
        agent,
        cwd,
        user_env,
        binary_override: binary_pref(agent).read(engine.pool()).await,
        account_pin,
        sandbox,
    })
}

/// The folder a coding-agent thread works in, resolved from its durable
/// record alone. `None` when the thread names no folder that exists.
pub(crate) async fn thread_cwd(
    engine: &LucidosEngine,
    thread_id: Uuid,
) -> Result<Option<PathBuf>, sqlx::Error> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT coding_agent_folder, coding_agent_kind FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(engine.pool())
    .await?;
    let Some((folder, kind)) = row else {
        return Ok(None);
    };
    let folder = folder
        .map(PathBuf::from)
        .unwrap_or_else(|| engine.workspace_path().to_path_buf());
    Ok(resolved_cwd(engine, thread_id, &folder, kind.as_deref() == Some("app")).await)
}

async fn resolved_cwd(
    engine: &LucidosEngine,
    thread_id: Uuid,
    folder: &Path,
    app: bool,
) -> Option<PathBuf> {
    let branch =
        crate::engine::session_queries::lookup_session_branch_for_thread(engine.pool(), thread_id)
            .await;
    let mut cwd = crate::engine::session_queries::resolve_worktree_path(
        engine.pool(),
        thread_id,
        engine.workspace_path(),
        if app { engine.workspace_path() } else { folder },
        branch.as_deref(),
    )
    .await;
    if app {
        if let Some(id) = folder.file_name() {
            cwd = cwd.join("data").join("apps").join(id);
        }
    }
    tokio::fs::metadata(&cwd)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
        .then_some(cwd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        boot_mock_engine, in_engine_child, seed_chat_thread, seed_preference, setup_test_db,
        teardown_test_db,
    };

    async fn suggestions(engine: &LucidosEngine, thread_id: Uuid) -> Vec<serde_json::Value> {
        sqlx::query_scalar(
            "SELECT payload FROM events \
             WHERE thread_id = $1 AND event_type = 'TerminalCommandSuggested' ORDER BY sequence",
        )
        .bind(thread_id)
        .fetch_all(engine.pool())
        .await
        .unwrap()
    }

    fn status(name: &str, needs_sign_in: bool) -> CodingAgentMcpServerStatus {
        CodingAgentMcpServerStatus {
            name: name.into(),
            status: if needs_sign_in {
                "needs-auth"
            } else {
                "connected"
            }
            .into(),
            needs_sign_in,
        }
    }

    #[test]
    fn a_status_report_alone_owes_no_suggestion() {
        let mut watch = McpSignInWatch::new(CodingAgent::ClaudeCode);
        watch.report(&[status("calendar", true), status("search", false)]);
        assert_eq!(watch.take_tool_call("Bash"), None);
        assert_eq!(watch.take_tool_call("mcp__search__query"), None);
        assert_eq!(watch.take_tool_call("mcp__other__query"), None);
    }

    #[test]
    fn a_tool_call_on_a_server_needing_sign_in_is_owed_once() {
        let mut watch = McpSignInWatch::new(CodingAgent::ClaudeCode);
        watch.report(&[status("calendar", true), status("search", false)]);
        assert_eq!(
            watch
                .take_tool_call("mcp__calendar__list_events")
                .as_deref(),
            Some("calendar")
        );
        assert_eq!(watch.take_tool_call("mcp__calendar__authenticate"), None);
        watch.report(&[status("calendar", true)]);
        assert_eq!(watch.take_tool_call("mcp__calendar__list_events"), None);
    }

    #[test]
    fn a_server_signed_in_since_is_owed_nothing() {
        let mut watch = McpSignInWatch::new(CodingAgent::ClaudeCode);
        watch.report(&[status("calendar", true)]);
        watch.report(&[status("calendar", false)]);
        assert_eq!(watch.take_tool_call("mcp__calendar__list_events"), None);
    }

    #[test]
    fn a_call_before_the_first_report_is_owed_when_it_arrives() {
        let mut watch = McpSignInWatch::new(CodingAgent::ClaudeCode);
        assert_eq!(watch.take_tool_call("mcp__calendar__list_events"), None);
        assert_eq!(watch.take_tool_call("mcp__search__query"), None);
        assert_eq!(
            watch.report(&[status("calendar", true), status("search", false)]),
            vec![("calendar".to_string(), SignInReason::ToolCall)]
        );
        assert!(watch.report(&[status("calendar", true)]).is_empty());
        assert_eq!(watch.take_tool_call("mcp__calendar__list_events"), None);
    }

    #[test]
    fn the_longest_matching_server_name_wins() {
        let mut watch = McpSignInWatch::new(CodingAgent::ClaudeCode);
        watch.report(&[status("calendar", true), status("calendar__work", true)]);
        assert_eq!(
            watch
                .take_tool_call("mcp__calendar__work__list_events")
                .as_deref(),
            Some("calendar__work")
        );
        assert_eq!(
            watch
                .take_tool_call("mcp__calendar__list_events")
                .as_deref(),
            Some("calendar")
        );
    }

    #[test]
    fn a_call_on_a_connected_longer_name_owes_nothing() {
        let mut watch = McpSignInWatch::new(CodingAgent::ClaudeCode);
        assert_eq!(
            watch.take_tool_call("mcp__calendar__work__list_events"),
            None
        );
        assert!(watch
            .report(&[status("calendar", true), status("calendar__work", false)])
            .is_empty());
        assert_eq!(
            watch.take_tool_call("mcp__calendar__work__list_events"),
            None
        );
    }

    #[test]
    fn a_codex_report_owes_each_server_once() {
        let mut watch = McpSignInWatch::new(CodingAgent::Codex);
        assert_eq!(
            watch.report(&[status("calendar", true), status("search", false)]),
            vec![("calendar".to_string(), SignInReason::Reported)]
        );
        assert!(watch.report(&[status("calendar", true)]).is_empty());
        assert_eq!(watch.take_tool_call("mcp__calendar__list_events"), None);
    }

    #[tokio::test]
    async fn a_sign_in_need_posts_one_suggestion_with_the_resolved_cli() {
        if !in_engine_child(
            "engine::agent_sign_in::tests::a_sign_in_need_posts_one_suggestion_with_the_resolved_cli",
        )
        .await
        {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        seed_preference(
            &pool,
            crate::core::prefs::CODING_AGENT_CLAUDE_PATH.key(),
            "/opt/agents/claude",
        )
        .await
        .unwrap();
        let thread_id = seed_chat_thread(&engine.event_bus, "hello").await;
        engine
            .event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::CodingAgentMcpStatusesReported {
                    agent: CodingAgent::ClaudeCode,
                    servers: vec![
                        CodingAgentMcpServerStatus {
                            name: "example".into(),
                            status: "needs-auth".into(),
                            needs_sign_in: true,
                        },
                        CodingAgentMcpServerStatus {
                            name: "ready".into(),
                            status: "connected".into(),
                            needs_sign_in: false,
                        },
                    ],
                },
                meta: EventMeta::NONE,
            })
            .await
            .unwrap();

        let request_id = Uuid::new_v4();
        answer_command(
            &engine,
            thread_id,
            request_id,
            CodingAgentCommand::Mcp,
            CodingAgent::ClaudeCode,
        )
        .await
        .unwrap();
        assert!(
            post_suggestion(
                SuggestionSink {
                    bus: &engine.event_bus,
                    posted: &engine.sign_in_contexts.posted(),
                },
                thread_id,
                SignInOffer {
                    agent: CodingAgent::ClaudeCode,
                    target: &AgentSignInTarget::ModelLogin,
                    reason: SignInReason::Requested,
                    gateway: false,
                },
                Some("/opt/agents/claude"),
                EventMeta::NONE,
            )
            .await
        );

        let posted = suggestions(&engine, thread_id).await;
        let commands: Vec<&str> = posted
            .iter()
            .map(|payload| payload["command"].as_str().unwrap())
            .collect();
        assert_eq!(
            commands,
            [
                "/opt/agents/claude mcp login example --no-browser",
                "env BROWSER=false /opt/agents/claude auth login",
            ]
        );
        assert_eq!(posted[0]["request_event_id"], request_id.to_string());
        let helpers: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE thread_id = $1 \
             AND event_type IN ('AgentSignInRequested', 'AgentSignInNeeded')",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(helpers, 0, "no sign-in card or helper request");
        teardown_test_db(&db).await;
    }
}
