use async_trait::async_trait;
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub const UNREQUESTED_INTERRUPT_ERROR: &str =
    "Codex interrupted this turn. Lucidos did not ask for it. The work is incomplete.";

use super::agent_runtime::{
    AgentEvent, AgentExitFacts, AgentInput, AgentRuntime, CodingAgent, ControlRequest,
    InputWithdrawal, RunningAgent, SpawnArgs, WithdrawRequest,
};
use super::agent_sandbox::SandboxedCommand;
use super::claude_code::{format_exit_status, GROUP_TEARDOWN_GRACE};
use super::codex_parse::{parse_codex_line, TurnTracker};
use super::lucidos_cli::{ensure_workspace_bin_symlink, lucidos_cli_dir};
use super::spawn_env::{apply_lucidos_env, command_secret_values, AgentSandbox, StderrTail};
use crate::engine::thread_events::{TurnEndFacts, TurnEndStatus};

pub use crate::llm::coding_agent_models::{codex_model_options, codex_reasoning_effort_options};

/// INVARIANT: Reasoning-summary mode both Codex drivers request. Codex's default
/// (`auto`) resolves to NO summaries over the app-server/exec protocols —
/// verified live against codex-cli 0.142.5: zero `item/reasoning/*`
/// notifications and an empty `summary` on the completed reasoning item, even
/// at `high` effort with real reasoning tokens billed — which left the
/// `CodingAgentThoughtStreamed` capture dormant for Codex. `detailed` makes
/// codex stream `item/reasoning/summaryTextDelta`, so a long reasoning pass
/// renders as a live Thinking step instead of a silent gap.
pub(super) const CODEX_REASONING_SUMMARY: &str = "detailed";

pub(super) const CODEX_PROJECT_DOC_FALLBACKS: &[&str] = &["CLAUDE.md"];
pub(super) const CODEX_PROJECT_DOC_MAX_BYTES: u64 = 65536;

pub(super) fn validate_codex_effort<'a>(
    model: Option<&str>,
    effort: Option<&'a str>,
) -> Option<&'a str> {
    let e = effort.filter(|e| !e.is_empty())?;
    let supported = codex_reasoning_effort_options()
        .iter()
        .find(|o| o.value == e)
        .is_some_and(|option| {
            option.supported_models.as_ref().is_none_or(|models| {
                model.is_some_and(|model| models.iter().any(|supported| supported == model))
            })
        });
    if supported {
        Some(e)
    } else {
        log_legacy!(
            "[Codex] dropping unsupported reasoning effort '{}' for model '{}' — codex default applies",
            e,
            model.unwrap_or("default/unknown"),
        );
        None
    }
}

pub fn codex_command_definitions() -> serde_json::Value {
    let (model_options, effort_options) = super::claude_code::model_and_effort_options(
        codex_model_options(),
        codex_reasoning_effort_options(),
    );
    serde_json::json!([
        {
            "subtype": "set_model",
            "label": "Model",
            "params": [{ "key": "model", "label": "Model", "options": model_options }]
        },
        {
            "subtype": "set_reasoning_effort",
            "label": "Reasoning Effort",
            "params": [{ "key": "effort", "label": "Reasoning Effort", "options": effort_options }]
        }
    ])
}

pub(super) struct CodexConfig {
    pub(super) codex_bin: std::ffi::OsString,
    pub(super) worktree_path: PathBuf,
    pub(super) system_prompt: Option<String>,
    pub(super) session_context: Option<super::agent_runtime::SessionContextBlock>,
    pub(super) model: Option<String>,
    pub(super) reasoning_effort: Option<String>,
    pub(super) sandbox_writable_roots: Vec<PathBuf>,
    pub(super) sandbox: super::agent_sandbox::Sandbox,
    /// INVARIANT: False while the `ps` shim is on PATH. A macOS login shell runs
    /// `path_helper`, which moves `/bin` ahead of the shim.
    pub(super) login_shell: bool,
    pub(super) env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    pub(super) env_removed: Vec<std::ffi::OsString>,
}

/// INVARIANT: Resolve the `codex` executable: the user-configured override (already
/// validated by the spawn path — see `spawn_env::resolve_binary_override`)
/// wins outright; otherwise probe the common install locations; otherwise
/// fall back to a bare PATH lookup.
///
/// Why probing: a launchd-, IDE-, or any non-interactive-shell-launched
/// engine inherits a PATH that can omit the Homebrew / npm bin dirs where
/// codex lives — a bare `Command::new("codex")` then ENOENTs even though the
/// CLI is installed. `home` is injected to keep the function pure and
/// testable.
pub(crate) fn resolve_codex_binary(
    home: Option<&Path>,
    override_path: Option<&Path>,
) -> std::ffi::OsString {
    if let Some(p) = override_path {
        return p.as_os_str().to_os_string();
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = home {
        candidates.push(home.join(".local/bin/codex"));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin/codex"));
    candidates.push(PathBuf::from("/usr/local/bin/codex"));
    for c in candidates {
        if c.exists() {
            return c.into_os_string();
        }
    }
    std::ffi::OsString::from("codex")
}

pub struct CodexRuntime;

#[async_trait]
impl AgentRuntime for CodexRuntime {
    fn kind(&self) -> CodingAgent {
        CodingAgent::Codex
    }

    async fn spawn(
        &self,
        args: SpawnArgs<'_>,
        cancel: CancellationToken,
    ) -> Result<RunningAgent, Box<dyn std::error::Error + Send + Sync>> {
        let cli_dir = lucidos_cli_dir();
        if cli_dir.is_none() {
            if crate::runtime::is_packaged() {
                return Err("The Lucidos CLI is required for coding-agent \
                    permission/question tools but was not found alongside the engine \
                    (expected LUCIDOS_CLI_BIN or a sibling `lucidos` binary)"
                    .into());
            }
            crate::log_legacy!(
                "[Codex] lucidos CLI binary not found near current_exe — \
                 spawned Codex sessions won't have the `lucidos` command on PATH \
                 (build with `cargo build -p lucidos-cli`)"
            );
        }
        ensure_workspace_bin_symlink(args.worktree_path, cli_dir);

        let sandbox_writable_roots =
            sandbox_writable_roots(args.worktree_path, args.workspace_path).await;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let protocol =
            codex_protocol_from_env(std::env::var("LUCIDOS_CODEX_PROTOCOL").ok().as_deref());
        let sandbox = match protocol {
            CodexProtocol::AppServer => args.sandbox.clone(),
            // INVARIANT: No card channel, so the allowlist is the only write guard.
            CodexProtocol::Exec => args.sandbox.writes_only_under(&confined_write_roots(
                args.worktree_path,
                &sandbox_writable_roots,
                codex_home(args.user_env_vars, home.as_deref()),
            )),
        };

        let (env, env_removed) = baked_lucidos_env(&args, cli_dir, &sandbox);
        let login_shell = sandbox.ps_shim_dir(args.workspace_path, cli_dir).is_none();
        let agent_run = args.agent_run.clone();

        if let Some(path) = args.binary_override {
            super::spawn_env::resolve_binary_override(
                path,
                "Codex (`codex`)",
                crate::core::prefs::CODING_AGENT_CODEX_PATH.key(),
            )?;
        }
        let config = CodexConfig {
            codex_bin: resolve_codex_binary(home.as_deref(), args.binary_override.map(Path::new)),
            worktree_path: args.worktree_path.to_path_buf(),
            system_prompt: args.system_prompt.map(str::to_string),
            session_context: args.session_context.cloned(),
            model: args.model.map(str::to_string),
            reasoning_effort: args.reasoning_effort.map(str::to_string),
            sandbox_writable_roots,
            sandbox,
            login_shell,
            env,
            env_removed,
        };

        if protocol == CodexProtocol::Exec {
            require_exec_command_hook(&config).await?;
        }

        let (events_tx, events_rx) = mpsc::unbounded_channel::<AgentEvent>();
        let (input_tx, input_rx) = mpsc::unbounded_channel::<AgentInput>();
        let (control_tx, control_rx) = mpsc::unbounded_channel::<ControlRequest>();
        let (withdraw_tx, withdraw_rx) = mpsc::unbounded_channel::<WithdrawRequest>();

        let resume_session_id = args.resume_session_id.map(str::to_string);
        let permission_rx = match protocol {
            CodexProtocol::AppServer => {
                let (permission_tx, permission_rx) = mpsc::unbounded_channel();
                tokio::spawn(async move {
                    super::codex_app_server::app_server_driver_task(
                        config,
                        resume_session_id,
                        events_tx,
                        input_rx,
                        control_rx,
                        withdraw_rx,
                        permission_tx,
                        cancel,
                    )
                    .await;
                    agent_run.finish(GROUP_TEARDOWN_GRACE).await;
                });
                Some(permission_rx)
            }
            CodexProtocol::Exec => {
                tokio::spawn(async move {
                    driver_task(
                        config,
                        resume_session_id,
                        events_tx,
                        input_rx,
                        control_rx,
                        withdraw_rx,
                        cancel,
                    )
                    .await;
                    agent_run.finish(GROUP_TEARDOWN_GRACE).await;
                });
                None
            }
        };

        Ok(RunningAgent {
            kind: CodingAgent::Codex,
            events_rx,
            input_tx,
            control_tx,
            permission_rx,
            withdraw_tx,
        })
    }
}

/// INVARIANT: The extra writable roots a Codex session needs beyond its own worktree,
/// which `--sandbox workspace-write` already grants. Every entry is a
/// deliberate hole in the sandbox:
///
/// 1. **The worktree's shared git dir.** A linked worktree's `.git` is a file
///    pointing at `<main>/.git/worktrees/<x>`, so an in-agent `git commit`
///    writes outside the worktree. Omitted when git can't tell us (degrade).
/// 2. **The workspace's `data/` tree.** Writing there is the documented
///    contract for a coding-agent thread: `lucidos data path` resolves under
///    `<workspace>/data/` (and creates dirs with `--mkdir`), and the workspace
///    knowhow instructs agents to log follow-ups to
///    `artifacts/work-tracker/data.json`. That path is OUTSIDE the worktree, so
///    the macOS seatbelt refused it with `EPERM (os error 1)` — which is how
///    the 2026-07-26 nightly's Codex security pass silently failed to persist
///    two high-severity findings. Claude Code runs unsandboxed and never hit
///    it, so the contract looked like it worked.
///
///    `lucidos data write` was the headline case and no longer is: it PUTs to
///    the engine's data API rather than touching the filesystem, so it clears
///    the seatbelt on the network axis (`network_access=true`, set below) with
///    no writable root at all. The grant stays for everything that still writes
///    `data/` directly: `data path --mkdir`, and an agent's own editor tools or
///    a script writing the resolved path.
///
/// Scoped to `<workspace>/data`, deliberately **not** the workspace root: the
/// root also holds `.lucidos/` (engine runtime, logs, pid files, the gateway
/// registry) and every sibling worktree — none of which a session has any
/// business writing. A missing `data/` dir is skipped rather than created;
/// the engine provisions it at boot, so its absence means something is off and
/// a spawn-time `mkdir` would only paper over it.
async fn sandbox_writable_roots(worktree: &Path, workspace: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(git_dir) = resolve_git_common_dir(worktree).await {
        // INVARIANT: Canonicalized for the same reason the data dir below is: the seatbelt
        // matches REAL paths, so a workspace reached through a symlink (a
        // `/tmp/...` one resolves to `/private/tmp/...` on macOS) would grant a
        // root the kernel never matches — and in-agent `git commit`, the only
        // reason this entry exists, would stay blocked. Falls back to the
        // as-resolved path if it has since vanished.
        roots.push(std::fs::canonicalize(&git_dir).unwrap_or(git_dir));
    }
    let data_dir = workspace.join(crate::core::DATA_DIR);
    // INVARIANT: `canonicalize` does three jobs here, all load-bearing:
    //   * proves the dir exists (replacing a bare `is_dir` probe);
    //   * makes the path ABSOLUTE — `LUCIDOS_WORKSPACE` is used verbatim and is
    //     routinely relative (the Makefile passes `./test-workspace`; the boot
    //     fallback is `./workspace`). codex resolves a relative `--add-dir`
    //     against the CHILD's cwd, which is the worktree — so a relative root
    //     would punch the hole somewhere meaningless and leave the real `data/`
    //     blocked, i.e. silently no fix at all;
    //   * resolves symlinks, which the macOS seatbelt matches on (`/var` →
    //     `/private/var` is the classic one).
    match std::fs::canonicalize(&data_dir).ok().filter(|d| d.is_dir()) {
        Some(dir) if grants_more_than_the_data_tree(&dir, workspace) => crate::log_legacy!(
            "[Codex] {} resolves to {}, which contains the workspace — refusing \
             to grant it; the sandbox will block direct writes into the parent \
             workspace's data/ tree (`lucidos data path --mkdir`, editor writes \
             to a resolved data path). `lucidos data write` is unaffected: it \
             goes over HTTP",
            data_dir.display(),
            dir.display()
        ),
        Some(dir) => roots.push(dir),
        None => crate::log_legacy!(
            "[Codex] {} is not a reachable directory — the sandbox will block \
             direct writes into the parent workspace's data/ tree \
             (`lucidos data path --mkdir`, editor writes to a resolved data \
             path). `lucidos data write` is unaffected: it goes over HTTP",
            data_dir.display()
        ),
    }
    roots
}

fn confined_write_roots(
    worktree: &Path,
    writable_roots: &[PathBuf],
    codex_home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut roots = vec![worktree.to_path_buf()];
    roots.extend(writable_roots.iter().cloned());
    roots.extend(codex_home);
    roots
}

fn codex_home(user_env_vars: &[(String, String)], home: Option<&Path>) -> Option<PathBuf> {
    const VAR: &str = "CODEX_HOME";
    user_env_vars
        .iter()
        .find(|(key, _)| key == VAR)
        .map(|(_, value)| PathBuf::from(value))
        .or_else(|| std::env::var_os(VAR).map(PathBuf::from))
        .or_else(|| home.map(|home| home.join(".codex")))
}

pub(super) fn codex_sandbox_mode(config: &CodexConfig) -> &'static str {
    if config.sandbox.is_active() {
        "danger-full-access"
    } else {
        "workspace-write"
    }
}

pub(super) fn codex_approval_policy(config: &CodexConfig) -> &'static str {
    if config.sandbox.is_active() {
        "untrusted"
    } else {
        "on-request"
    }
}

pub(crate) use crate::core::grants::grants_more_than_the_data_tree;

async fn resolve_git_common_dir(worktree: &Path) -> Option<PathBuf> {
    match crate::engine::git_ops::git_common_dir(worktree).await {
        Ok(dir) => Some(dir),
        // INVARIANT: Never degrade the sandbox silently: without this line the user sees
        // in-agent `git commit` blocked with nothing in the log saying why.
        Err(e) => {
            crate::log_legacy!(
                "[Codex] {} in {}: sandbox will block in-agent git commits",
                e,
                worktree.display()
            );
            None
        }
    }
}

pub(super) const COMMAND_DENY_HOOK: &str = r#"lucidos command-deny-hook || {
    printf '%s\n' 'Command guard did not authorize this call.' >&2
    exit 2
}"#;

/// INVARIANT: The legacy exec protocol has no hook discovery request. Require a CLI
/// release with shell PreToolUse hooks and session-scoped hook trust.
async fn require_exec_command_hook(config: &CodexConfig) -> std::io::Result<()> {
    let binary = Path::new(&config.codex_bin);
    let mut command = config.sandbox.command(binary)?;
    command.current_dir(&config.worktree_path);
    let supported = super::probe_agent_version_command(
        CodingAgent::Codex,
        binary,
        std::time::Duration::from_secs(5),
        &mut command,
    )
    .await
    .as_deref()
    .is_some_and(supports_exec_command_hook);
    if supported {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "Codex exec requires version 0.170.0 or later for standing denies. Update Codex or use the app-server protocol.",
        ))
    }
}

fn supports_exec_command_hook(version: &str) -> bool {
    semver::Version::parse(version).is_ok_and(|version| version >= semver::Version::new(0, 170, 0))
}

pub(super) fn command_deny_hooks() -> serde_json::Value {
    super::codex_side_question::trusted_pre_tool_hook(
        "Bash",
        serde_json::json!({
            "type": "command", "command": COMMAND_DENY_HOOK, "async": false, "timeout": 10,
        }),
    )
}

/// INVARIANT: Build one per-turn `codex exec` command. Pure over its inputs so the unit
/// tests can pin flag layout without spawning.
///
/// The turn's prompt goes to stdin ([`send_prompt_on_stdin`]), never argv.
/// Linux caps one argv string at 128 KiB, which a first turn carrying a long
/// thread's history outgrows. Argv is also world-readable in `/proc`.
fn build_codex_turn_command(
    config: &CodexConfig,
    model: Option<&str>,
    effort: Option<&str>,
    resume_session_id: Option<&str>,
    image_paths: &[PathBuf],
) -> std::io::Result<SandboxedCommand> {
    let mut cmd = config.sandbox.command(&config.codex_bin)?;
    cmd.arg("exec").arg("--json");
    let hooks = command_deny_hooks();
    let key = super::codex_side_question::GUARD_KEY;
    let hash = &hooks["state"][key]["trusted_hash"];
    let hook_command = serde_json::Value::from(COMMAND_DENY_HOOK);
    cmd.arg("-c").arg("features.hooks=true");
    cmd.arg("-c").arg(format!(
        "hooks={{ PreToolUse = [{{ matcher = \"Bash\", hooks = [{{ type = \"command\", command = {hook_command}, async = false, timeout = 10 }}] }}], state = {{ \"{key}\" = {{ enabled = true, trusted_hash = {hash} }} }} }}"
    ));
    cmd.arg("--sandbox").arg(codex_sandbox_mode(config));
    cmd.arg("-c")
        .arg("sandbox_workspace_write.network_access=true");
    cmd.arg("-c").arg(format!(
        "model_reasoning_summary=\"{CODEX_REASONING_SUMMARY}\""
    ));
    cmd.arg("-c").arg(format!(
        "project_doc_fallback_filenames={}",
        serde_json::to_string(CODEX_PROJECT_DOC_FALLBACKS).expect("static list serializes")
    ));
    cmd.arg("-c").arg(format!(
        "project_doc_max_bytes={CODEX_PROJECT_DOC_MAX_BYTES}"
    ));
    if !config.login_shell {
        cmd.arg("-c").arg("allow_login_shell=false");
    }
    for flag in lucidos_mcp_server_config_overrides(&config.env) {
        cmd.arg("-c").arg(flag);
    }
    for root in &config.sandbox_writable_roots {
        cmd.arg("--add-dir").arg(root);
    }
    if let Some(m) = model.filter(|m| !m.is_empty() && *m != "default") {
        cmd.arg("-m").arg(m);
    }
    if let Some(e) = validate_codex_effort(model, effort) {
        cmd.arg("-c")
            .arg(format!("model_reasoning_effort=\"{}\"", e));
    }
    for img in image_paths {
        cmd.arg("-i").arg(img);
    }
    // INVARIANT: Global exec flags must precede the `resume` subcommand. In both forms a
    // `-` positional reads the prompt from stdin. The `--` keeps `-i` from
    // taking that `-` as one more image.
    if let Some(sid) = resume_session_id {
        cmd.arg("resume").arg(sid);
    }
    cmd.arg("--").arg("-");
    cmd.current_dir(&config.worktree_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for k in &config.env_removed {
        cmd.env_remove(k);
    }
    for (k, v) in &config.env {
        cmd.env(k, v);
    }
    Ok(cmd)
}

pub(super) fn lucidos_mcp_server_config_overrides(
    env: &[(std::ffi::OsString, std::ffi::OsString)],
) -> Vec<String> {
    let config = lucidos_mcp_server_config_json(env);
    let map = config
        .as_object()
        .expect("lucidos MCP server config is an object");
    map.iter()
        .map(|(key, value)| {
            let value = if key == "env" {
                render_toml_inline_table(value)
            } else {
                serde_json::to_string(value).expect("config value serializes")
            };
            format!("mcp_servers.lucidos.{key}={value}")
        })
        .collect()
}

pub(super) fn baked_lucidos_env(
    args: &SpawnArgs<'_>,
    cli_dir: Option<&Path>,
    sandbox: &super::agent_sandbox::Sandbox,
) -> (Vec<(OsString, OsString)>, Vec<OsString>) {
    let mut probe = tokio::process::Command::new("true");
    apply_lucidos_env(
        &mut probe,
        args,
        cli_dir,
        match sandbox.compile_cache() {
            AgentSandbox::Unsandboxed => AgentSandbox::WritesConfined,
            confined => confined,
        },
        "Codex",
    );
    let mut env = Vec::new();
    let mut env_removed = Vec::new();
    for (k, v) in probe.as_std().get_envs() {
        match v {
            Some(v) => env.push((k.to_owned(), v.to_owned())),
            None => env_removed.push(k.to_owned()),
        }
    }
    (env, env_removed)
}

/// INVARIANT: Wire name of the question tool as it appears in `CodingAgentToolCalled`
/// events — codex prefixes MCP tools with `mcp__<server>__`. The engine's
/// run loop suppresses the tool-call event for this name (the
/// `UserQuestionAsked` emitted by the internal endpoint renders the card; a
/// tool-call step on top would double-surface the same question).
pub const CODEX_ASK_USER_QUESTION_TOOL: &str = "mcp__lucidos__ask_user_question";

/// INVARIANT: The lucidos MCP server config (`lucidos mcp-permission-server`, same
/// binary CC spawns) — the SINGLE source for both Codex drivers: the
/// app-server driver embeds it in `thread/start`'s `config` object, the
/// exec driver derives `-c` flags from it via
/// [`lucidos_mcp_server_config_overrides`]. It gives the model a
/// QuestionCard path via the `ask_user_question` tool:
///
/// - `enabled_tools` hides the CC-only `approve` permission tool — without it
///   Codex would advertise `mcp__lucidos__approve` to the model as an
///   ordinary callable tool. The mirror-image filter for CC lives inside the
///   server instead (`--permission-only`, see `permission_mcp_config_json`),
///   because CC has no `enabled_tools` equivalent for a server it spawns.
/// - `tools.ask_user_question.approval_mode = "approve"` trusts that ONE MCP
///   tool so non-interactive Codex sessions don't auto-cancel it as an
///   unapproved MCP call before the Lucidos QuestionCard endpoint can run.
/// - `tool_timeout_sec` lifts codex's default per-tool cap to 24h (the codex
///   analog of CC's `MCP_TIMEOUT` env pair) — the user may take hours to
///   answer; a timeout would error the tool call and make the model re-ask.
/// - `env` explicitly forwards the small Lucidos contract the MCP child needs
///   (`LUCIDOS_WORKSPACE`, `LUCIDOS_THREAD_ID`, optional loopback base URL).
///   Codex does not inherit the app-server/exec process env into stdio MCP
///   children, so without this the server exits before `initialize` and the
///   model sees no `ask_user_question` tool.
/// - `env_vars` forwards the origin token by name from Codex's own env, so
///   the secret stays out of the config and the `-c` argv. It is the child's
///   only credential: the agent sandbox denies the local token file.
/// - `command` is the bare binary name: `apply_lucidos_env` prepends the
///   bundled CLI dir to `PATH`, which codex itself uses to find the command.
///   The dir resolves via `LUCIDOS_CLI_BIN` in a packaged build (the staged
///   `<resources>/lucidos`) and via the engine's pinned copy in dev — see
///   `lucidos_cli::lucidos_cli_dir`. A packaged build with the CLI unstaged
///   fails fast at spawn (`is_packaged()` guard), so this never silently 404s.
pub(super) fn lucidos_mcp_server_config_json(
    env: &[(std::ffi::OsString, std::ffi::OsString)],
) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("command".to_string(), "lucidos".into());
    obj.insert(
        "args".to_string(),
        serde_json::json!(["mcp-permission-server"]),
    );
    let mcp_env = lucidos_mcp_child_env(env);
    if !mcp_env.is_empty() {
        obj.insert("env".to_string(), serde_json::Value::Object(mcp_env));
    }
    obj.insert(
        "env_vars".to_string(),
        serde_json::json!([crate::core::subprocess_origin::ENV_AGENT_ORIGIN_TOKEN]),
    );
    obj.insert(
        "enabled_tools".to_string(),
        serde_json::json!(["ask_user_question"]),
    );
    obj.insert(
        "tools".to_string(),
        serde_json::json!({
            "ask_user_question": {
                "approval_mode": "approve",
            },
        }),
    );
    obj.insert(
        "tool_timeout_sec".to_string(),
        crate::engine::cc_settings::USER_ANSWER_WAIT_SECS.into(),
    );
    serde_json::Value::Object(obj)
}

const LUCIDOS_MCP_CHILD_ENV_KEYS: &[&str] = &[
    "LUCIDOS_BUILD_SLOT_CLASS",
    "LUCIDOS_WORKSPACE",
    "LUCIDOS_THREAD_ID",
    "LUCIDOS_API_BASE_URL",
];

fn lucidos_mcp_child_env(
    env: &[(std::ffi::OsString, std::ffi::OsString)],
) -> serde_json::Map<String, serde_json::Value> {
    let mut out = serde_json::Map::new();
    for key in LUCIDOS_MCP_CHILD_ENV_KEYS {
        let Some((_, value)) = env.iter().find(|(k, _)| k == OsStr::new(key)) else {
            continue;
        };
        let Some(value) = value.to_str().filter(|v| !v.is_empty()) else {
            continue;
        };
        out.insert((*key).to_string(), value.into());
    }
    out
}

fn render_toml_inline_table(value: &serde_json::Value) -> String {
    let Some(map) = value.as_object() else {
        return "{}".to_string();
    };
    let pairs = LUCIDOS_MCP_CHILD_ENV_KEYS
        .iter()
        .filter_map(|key| {
            map.get(*key).and_then(|value| value.as_str()).map(|value| {
                format!(
                    "{key} = {}",
                    serde_json::to_string(value).expect("env value serializes")
                )
            })
        })
        .collect::<Vec<_>>();
    format!("{{ {} }}", pairs.join(", "))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CodexProtocol {
    AppServer,
    Exec,
}

pub(super) fn codex_protocol_from_env(value: Option<&str>) -> CodexProtocol {
    match value {
        Some("exec") => CodexProtocol::Exec,
        Some("app-server") | None => CodexProtocol::AppServer,
        Some(other) => {
            crate::log_legacy!(
                "[Codex] Unknown LUCIDOS_CODEX_PROTOCOL={:?} — defaulting to app-server",
                other
            );
            CodexProtocol::AppServer
        }
    }
}

fn compose_first_turn_prompt(system_prompt: Option<&str>, user_text: &str) -> String {
    match system_prompt {
        Some(sp) if !sp.is_empty() => format!(
            "# Session instructions (from the Lucidos engine)\n\n{}\n\n---\n\n{}",
            sp, user_text
        ),
        _ => user_text.to_string(),
    }
}

/// INVARIANT: Write `prompt` to the turn's stdin, then close it so codex sees the end.
/// A task, so a prompt bigger than the pipe buffer never stalls the turn loop.
fn send_prompt_on_stdin(stdin: Option<tokio::process::ChildStdin>, prompt: String) {
    let Some(mut stdin) = stdin else {
        log_legacy!("[Codex] the turn has no stdin to send its prompt on");
        return;
    };
    tokio::spawn(async move {
        if let Err(e) = stdin.write_all(prompt.as_bytes()).await {
            log_legacy!("[Codex] failed to send the turn's prompt on stdin: {}", e);
        }
    });
}

pub(super) fn write_image_files(
    images: &[crate::engine::types::ChatImage],
) -> (Vec<PathBuf>, Vec<tempfile::TempPath>) {
    let mut paths = Vec::new();
    let mut guards = Vec::new();
    for img in images {
        let ext = match img.mime_type.as_str() {
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => "png",
        };
        use base64::Engine as _;
        let bytes = match base64::engine::general_purpose::STANDARD.decode(&img.base64) {
            Ok(b) => b,
            Err(e) => {
                crate::log_legacy!("[Codex] dropping undecodable pasted image: {}", e);
                continue;
            }
        };
        let file = match tempfile::Builder::new()
            .prefix("lucidos-codex-img-")
            .suffix(&format!(".{ext}"))
            .tempfile()
        {
            Ok(f) => f,
            Err(e) => {
                crate::log_legacy!("[Codex] failed to create temp image file: {}", e);
                continue;
            }
        };
        if let Err(e) = std::fs::write(file.path(), &bytes) {
            crate::log_legacy!("[Codex] failed to write temp image file: {}", e);
            continue;
        }
        let path = file.path().to_path_buf();
        paths.push(path);
        guards.push(file.into_temp_path());
    }
    (paths, guards)
}

/// INVARIANT: Answer a withdraw for a Codex driver. An input still in `queue` never
/// reached Codex, so it is dropped. Any other input went into a turn.
/// Callers must hold no input outside `queue` and `input_rx`.
pub(super) fn answer_withdraw(
    queue: &mut VecDeque<AgentInput>,
    input_rx: &mut mpsc::UnboundedReceiver<AgentInput>,
    request: WithdrawRequest,
) {
    while let Ok(input) = input_rx.try_recv() {
        queue.push_back(input);
    }
    let at = queue
        .iter()
        .position(|input| input.uuid == request.input_uuid);
    let outcome = match at {
        Some(at) => {
            queue.remove(at);
            InputWithdrawal::Withdrawn
        }
        None => InputWithdrawal::AlreadyRead,
    };
    let _ = request.reply.send(outcome);
}

/// INVARIANT: Outcome of one turn — tells the driver loop what to do next.
enum TurnOutcome {
    Continue,
    Shutdown,
}

async fn driver_task(
    config: CodexConfig,
    resume_session_id: Option<String>,
    events_tx: mpsc::UnboundedSender<AgentEvent>,
    mut input_rx: mpsc::UnboundedReceiver<AgentInput>,
    mut control_rx: mpsc::UnboundedReceiver<ControlRequest>,
    mut withdraw_rx: mpsc::UnboundedReceiver<WithdrawRequest>,
    cancel: CancellationToken,
) {
    let mut tracker = TurnTracker::new(resume_session_id);
    let mut queue: VecDeque<AgentInput> = VecDeque::new();

    'session: loop {
        while queue.is_empty() {
            tokio::select! {
                input = input_rx.recv() => {
                    match input {
                        Some(i) => queue.push_back(i),
                        None => break 'session,
                    }
                }
                req = control_rx.recv() => {
                    match req {
                        Some(ControlRequest::Interrupt) => {}
                        None => break 'session,
                    }
                }
                Some(withdraw) = withdraw_rx.recv() => {
                    answer_withdraw(&mut queue, &mut input_rx, withdraw);
                }
                _ = cancel.cancelled() => {
                    log_legacy!("[Codex] cancellation signalled while idle — ending session");
                    break 'session;
                }
            }
        }
        let input = queue.pop_front().expect("queue non-empty");

        let outcome = run_turn(
            &config,
            &mut tracker,
            input,
            &events_tx,
            &mut input_rx,
            &mut control_rx,
            &mut withdraw_rx,
            &mut queue,
            &cancel,
        )
        .await;
        match outcome {
            TurnOutcome::Continue => {}
            TurnOutcome::Shutdown => break 'session,
        }
    }

    let _ = events_tx.send(AgentEvent::Exited {
        exit: AgentExitFacts::default(),
    });
}

struct TurnOutput<'a> {
    events_tx: &'a mpsc::UnboundedSender<AgentEvent>,
    input: uuid::Uuid,
    read_reported: bool,
}

impl TurnOutput<'_> {
    fn forward(&mut self, tracker: &mut TurnTracker, line: &[u8], elapsed_ms: u64) -> bool {
        if !self.read_reported {
            self.read_reported = true;
            let _ = self
                .events_tx
                .send(AgentEvent::InputRead { input: self.input });
        }
        tracker
            .map_line(parse_codex_line(&String::from_utf8_lossy(line)), elapsed_ms)
            .into_iter()
            .all(|ev| self.events_tx.send(ev).is_ok())
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_turn(
    config: &CodexConfig,
    tracker: &mut TurnTracker,
    input: AgentInput,
    events_tx: &mpsc::UnboundedSender<AgentEvent>,
    input_rx: &mut mpsc::UnboundedReceiver<AgentInput>,
    control_rx: &mut mpsc::UnboundedReceiver<ControlRequest>,
    withdraw_rx: &mut mpsc::UnboundedReceiver<WithdrawRequest>,
    queue: &mut VecDeque<AgentInput>,
    cancel: &CancellationToken,
) -> TurnOutcome {
    // INVARIANT: Exec starts another process for every turn. Its JSON stream exposes no
    // effective auth mode; never borrow a previous process's billing evidence.
    let _ = events_tx.send(AgentEvent::BillingObserved {
        attribution: super::billing_provider::BillingAttribution::default(),
    });
    let turn_effort =
        validate_codex_effort(config.model.as_deref(), config.reasoning_effort.as_deref())
            .map(String::from);
    tracker.begin_turn(turn_effort.clone());
    let turn_start = std::time::Instant::now();

    let resume_sid = tracker.session_id.clone();
    let input = match (&resume_sid, &config.session_context) {
        (None, Some(context)) => context.lead(input),
        _ => input,
    };
    let (image_paths, _image_guards) = write_image_files(&input.images);
    let text = if input.text.is_empty() && !image_paths.is_empty() {
        "See the attached image(s).".to_string()
    } else {
        input.text
    };
    let prompt = if resume_sid.is_none() {
        compose_first_turn_prompt(config.system_prompt.as_deref(), &text)
    } else {
        text
    };

    let spawned = build_codex_turn_command(
        config,
        config.model.as_deref(),
        turn_effort.as_deref(),
        resume_sid.as_deref(),
        &image_paths,
    )
    .and_then(|mut cmd| {
        let secrets = command_secret_values(&cmd);
        super::spawn_env::spawn_below_engine(&mut cmd).map(|child| (child, secrets))
    });
    let (mut child, secrets) = match spawned {
        Ok(spawned) => spawned,
        Err(e) => {
            log_legacy!("[Codex] failed to spawn codex CLI: {}", e);
            let _ = events_tx.send(AgentEvent::InputRejected { input: input.uuid });
            let _ = events_tx.send(AgentEvent::Result {
                text: String::new(),
                duration_ms: 0,
                error: Some(format!("Failed to start Codex: {e}")),
                provider_error: None,
                turn_end: Some(TurnEndFacts::of_status(TurnEndStatus::Failed)),
            });
            return TurnOutcome::Continue;
        }
    };
    send_prompt_on_stdin(child.stdin.take(), prompt);
    let child_pid = child.id();
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = StderrTail::spawn(child.stderr.take().expect("piped stderr"), secrets);
    let mut stdout_reader = BufReader::new(stdout);

    // INVARIANT: Bytes, read with the cancel-safe `read_until`. A mid-turn input or a
    // control request drops the read. The part of a line already read must
    // stay in `line_buf`. The CC driver's `driver_task` explains more.
    let mut line_buf: Vec<u8> = Vec::new();
    let mut interrupted = false;
    let mut shutdown = false;
    let mut output = TurnOutput {
        events_tx,
        input: input.uuid,
        read_reported: false,
    };
    'turn: loop {
        tokio::select! {
            read_result = stdout_reader.read_until(b'\n', &mut line_buf) => {
                match read_result {
                    Ok(0) => break 'turn,
                    Ok(_) => {
                        let alive = output.forward(tracker, &line_buf, turn_start.elapsed().as_millis() as u64);
                        line_buf.clear();
                        if !alive {
                            shutdown = true;
                            break 'turn;
                        }
                    }
                    Err(e) => {
                        log_legacy!("[Codex] stdout read error: {}", e);
                        break 'turn;
                    }
                }
            }
            input = input_rx.recv() => {
                match input {
                    Some(i) => queue.push_back(i),
                    None => {
                        shutdown = true;
                        break 'turn;
                    }
                }
            }
            Some(withdraw) = withdraw_rx.recv() => {
                answer_withdraw(queue, input_rx, withdraw);
            }
            req = control_rx.recv() => {
                match req {
                    Some(ControlRequest::Interrupt) => {
                        log_legacy!("[Codex] interrupt — killing in-flight codex turn (pid={:?})", child_pid);
                        interrupted = true;
                        break 'turn;
                    }
                    None => {
                        shutdown = true;
                        break 'turn;
                    }
                }
            }
            _ = cancel.cancelled() => {
                log_legacy!("[Codex] cancellation signalled — killing codex turn");
                shutdown = true;
                break 'turn;
            }
        }
    }

    let wait_result = if interrupted || shutdown {
        // INVARIANT: The child is its own process-group leader (`spawn_below_engine`
        // at spawn), so `start_kill` signals ONLY the leader: everything the
        // Codex turn spawned (cargo/rustc, a Playwright runner and its browsers)
        // survives as an orphan burning CPU. Tear the whole group down first,
        // exactly as the CC driver does after the 2026-06-24 WebKit pile-up.
        #[cfg(unix)]
        if let Some(pid) = child_pid {
            super::spawn_env::graceful_kill_child_process_group(pid, GROUP_TEARDOWN_GRACE).await;
        }
        let _ = child.start_kill();
        child.wait().await
    } else {
        if !line_buf.is_empty() {
            output.forward(tracker, &line_buf, turn_start.elapsed().as_millis() as u64);
            line_buf.clear();
        }
        let drain_deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            match tokio::time::timeout_at(
                drain_deadline,
                stdout_reader.read_until(b'\n', &mut line_buf),
            )
            .await
            {
                Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
                Ok(Ok(_)) => {
                    output.forward(tracker, &line_buf, turn_start.elapsed().as_millis() as u64);
                    line_buf.clear();
                }
            }
        }
        match tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await {
            Ok(r) => r,
            Err(_) => {
                let _ = child.kill().await;
                child.wait().await
            }
        }
    };

    let stderr_tail = stderr.finish().await;
    if !stderr_tail.is_empty() {
        log_legacy!("[Codex] codex stderr: {}", stderr_tail);
    }
    log_legacy!(
        "[Codex] turn child exited (pid={} status={})",
        child_pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "?".to_string()),
        format_exit_status(&wait_result),
    );

    if shutdown {
        return TurnOutcome::Shutdown;
    }
    if !output.read_reported {
        // INVARIANT: Named before the `Result`, so the idle that follows owes nothing.
        let _ = events_tx.send(AgentEvent::InputRejected { input: input.uuid });
    }
    if interrupted {
        // INVARIANT: The engine's interrupt arm waits for a Result to close the turn as
        // Canceled — synthesize it (the killed child never printed one).
        // Close in-flight tools first so the engine's paired-tool counter
        // re-arms its watchdog.
        for ev in tracker.close_open_tools() {
            let _ = events_tx.send(ev);
        }
        let _ = events_tx.send(AgentEvent::Result {
            text: tracker.turn_text(),
            duration_ms: turn_start.elapsed().as_millis() as u64,
            error: None,
            provider_error: None,
            turn_end: Some(TurnEndFacts::of_status(TurnEndStatus::Interrupted)),
        });
        return TurnOutcome::Continue;
    }
    if !tracker.turn_terminal_seen {
        for ev in tracker.close_open_tools() {
            let _ = events_tx.send(ev);
        }
        let reason = tracker
            .last_error
            .clone()
            .or_else(|| (!stderr_tail.is_empty()).then(|| stderr_tail.clone()))
            .unwrap_or_else(|| {
                format!(
                    "codex exited unexpectedly ({})",
                    format_exit_status(&wait_result)
                )
            });
        let _ = events_tx.send(AgentEvent::Result {
            text: tracker.turn_text(),
            duration_ms: turn_start.elapsed().as_millis() as u64,
            error: Some(reason),
            provider_error: tracker.last_provider_error.clone(),
            turn_end: Some(TurnEndFacts::of_status(TurnEndStatus::Synthesized)),
        });
    }
    TurnOutcome::Continue
}

#[cfg(test)]
#[path = "codex_tests/parsing.rs"]
mod parsing_tests;

#[cfg(test)]
#[path = "codex_tests/commands.rs"]
mod commands_tests;

#[cfg(test)]
#[path = "codex_tests/driver.rs"]
mod driver_tests;
