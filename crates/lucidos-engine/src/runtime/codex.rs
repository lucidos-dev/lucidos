//! `AgentRuntime` implementation backed by the OpenAI `codex` CLI.
//!
//! Two drivers behind one runtime, selected per spawn by
//! `LUCIDOS_CODEX_PROTOCOL` (ADR 0005):
//!
//! - **app-server** (default, `codex_app_server.rs`) — one persistent
//!   `codex app-server` JSON-RPC child per session: permission cards,
//!   per-token streaming, graceful `turn/interrupt`.
//! - **exec** (this file, the escape hatch) — `codex exec --json` runs
//!   exactly ONE turn per process and exits, with `codex exec resume
//!   <thread_id>` continuing the conversation in a fresh process. The driver
//!   presents a long-lived [`RunningAgent`] over a *sequence* of short-lived
//!   children: each `AgentInput` becomes one `codex exec` run; between turns
//!   the driver idles on its channels. Non-interactive: the OS sandbox is
//!   the only guard. Kept fully wired because app-server is
//!   upstream-experimental — one env var rolls a workspace back.
//!
//! Both drivers share [`CodexConfig`], the sandbox profile, the `lucidos`
//! MCP server wiring (`ask_user_question`), and the Codex thread id (same
//! on-disk rollout, so a thread survives a protocol flip).
//!
//! Exec protocol mapping lives in `codex_parse.rs`. Lifecycle contract
//! honored by both drivers (see `agent_runtime.rs`):
//! - `Init` once, from the first `thread.started` / the thread response (the
//!   Codex thread id is the engine's resume handle, persisted like CC's
//!   session id).
//! - One `Result` per accepted input — synthesized on interrupt and on a
//!   child that dies without a turn terminal, so the engine never waits on a
//!   Result that isn't coming.
//! - `Exited` exactly once, when the driver winds down (cancellation, input
//!   channel closed, or consumer gone).

use async_trait::async_trait;
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::agent_runtime::{
    AgentEvent, AgentInput, AgentRuntime, CodingAgent, ControlRequest, RunningAgent, SpawnArgs,
};
use super::claude_code::{format_exit_status, CcMenuOption};
use super::codex_parse::{parse_codex_line, TurnTracker};
use super::lucidos_cli::{ensure_workspace_bin_symlink, lucidos_cli_dir};
use super::spawn_env::{apply_lucidos_env, drain_stderr};

/// Prompt used when the engine asks for a continuation (resume with no new
/// user input). Mirrors the text `claude --print --resume` auto-injects in
/// the same situation, so both backends behave alike after a mid-turn
/// engine restart. Shared with the app-server driver.
pub(super) const CONTINUATION_PROMPT: &str = "Continue from where you left off.";

#[derive(Debug, serde::Deserialize)]
struct CodexMenuOptionsFile {
    models: Vec<CcMenuOption>,
    reasoning_efforts: Vec<CcMenuOption>,
}

const CODEX_MENU_OPTIONS_JSON: &str = include_str!("codex_menu_options.json");

static CODEX_MENU_OPTIONS: std::sync::LazyLock<CodexMenuOptionsFile> =
    std::sync::LazyLock::new(|| {
        serde_json::from_str(CODEX_MENU_OPTIONS_JSON)
            .expect("codex_menu_options.json is malformed — see runtime/codex_menu_options.json")
    });

pub fn codex_model_options() -> &'static [CcMenuOption] {
    &CODEX_MENU_OPTIONS.models
}

pub fn codex_reasoning_effort_options() -> &'static [CcMenuOption] {
    &CODEX_MENU_OPTIONS.reasoning_efforts
}

/// Reasoning-summary mode both Codex drivers request. Codex's default
/// (`auto`) resolves to NO summaries over the app-server/exec protocols —
/// verified live against codex-cli 0.142.5: zero `item/reasoning/*`
/// notifications and an empty `summary` on the completed reasoning item, even
/// at `high` effort with real reasoning tokens billed — which left the
/// `CodingAgentThoughtStreamed` capture dormant for Codex. `detailed` makes
/// codex stream `item/reasoning/summaryTextDelta`, so a long reasoning pass
/// renders as a live Thinking step instead of a silent gap.
pub(super) const CODEX_REASONING_SUMMARY: &str = "detailed";

/// Project-doc fallback both Codex drivers configure. Codex natively reads
/// only `AGENTS.md`, which Lucidos deliberately does not ship (ADR 0004 — no
/// AGENTS.md injection); Claude Code auto-loads `CLAUDE.md` + `.claude/rules`.
/// Pointing codex's *fallback* filename list at `CLAUDE.md` gives a Codex
/// session the same working agreement without writing anything into the repo
/// (a repo that ships its own `AGENTS.md` still wins — fallback only). The
/// byte cap is raised above codex's 32 KiB default: Lucidos' CLAUDE.md is
/// ~29 KiB and growing, and silent truncation would drop the tail rules.
pub(super) const CODEX_PROJECT_DOC_FALLBACKS: &[&str] = &["CLAUDE.md"];
pub(super) const CODEX_PROJECT_DOC_MAX_BYTES: u64 = 65536;

/// Validate a reasoning-effort value against the Codex model/effort matrix
/// (`codex_menu_options.json` is the source of truth). An out-of-vocabulary or
/// model-incompatible value fails the whole turn with `invalid_request_error`
/// (two real turns died this way on 2026-06-21), so both drivers drop it with a
/// log line instead: Codex then applies its own default effort.
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
        log!(
            "[Codex] dropping unsupported reasoning effort '{}' for model '{}' — codex default applies",
            e,
            model.unwrap_or("default/unknown"),
        );
        None
    }
}

/// Render the control-command menu for the frontend's `/model` picker —
/// Codex counterpart of `claude_code::cc_command_definitions`. No
/// `set_permission_mode`: Codex has no CC-style permission-MODE protocol to
/// switch — approvals are per-request cards under the app-server driver and
/// absent entirely under the exec escape hatch (sandbox is the guard).
pub fn codex_command_definitions() -> serde_json::Value {
    fn options_to_json(opts: &[CcMenuOption]) -> Vec<serde_json::Value> {
        opts.iter()
            .map(|m| {
                let mut option = serde_json::json!({
                    "value": m.value,
                    "label": m.label,
                    "description": m.description,
                });
                if let Some(models) = &m.supported_models {
                    option["supported_models"] = serde_json::json!(models);
                }
                option
            })
            .collect()
    }
    serde_json::json!([
        {
            "subtype": "set_model",
            "label": "Model",
            "params": [{ "key": "model", "label": "Model", "options": options_to_json(codex_model_options()) }]
        },
        {
            "subtype": "set_reasoning_effort",
            "label": "Reasoning Effort",
            "params": [{ "key": "effort", "label": "Reasoning Effort", "options": options_to_json(codex_reasoning_effort_options()) }]
        }
    ])
}

/// Owned snapshot of the spawn parameters — the driver task outlives the
/// borrowed `SpawnArgs`. Shared by both Codex drivers (exec per-turn and
/// app-server persistent — see `codex_app_server.rs`).
pub(super) struct CodexConfig {
    /// Executable to spawn. Production: `resolve_codex_binary()`. Driver
    /// tests inject a stub script here, the same seam CC's driver tests get
    /// from `driver_task`'s pre-spawned child.
    pub(super) codex_bin: std::ffi::OsString,
    pub(super) worktree_path: PathBuf,
    pub(super) system_prompt: Option<String>,
    pub(super) model: Option<String>,
    pub(super) reasoning_effort: Option<String>,
    /// Extra writable roots for the OS sandbox, beyond the session worktree
    /// that `--sandbox workspace-write` already grants (`--add-dir` on exec;
    /// `sandbox_workspace_write.writable_roots` on app-server). Resolved once
    /// at spawn by [`sandbox_writable_roots`], which documents why each entry
    /// is there. Deliberately a closed, explicit list — every entry is a hole
    /// in the sandbox and needs a reason.
    pub(super) sandbox_writable_roots: Vec<PathBuf>,
    /// Pre-built env (Lucidos contract) applied to every spawned child.
    pub(super) env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

/// Resolve the `codex` executable: the user-configured override (already
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

/// `AgentRuntime` implementation backed by the `codex` CLI.
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
            // Packaged builds stage the CLI (LUCIDOS_CLI_BIN); without it the
            // `lucidos` permission/question MCP server can't load and the session
            // errors mid-stream. Fail fast with an actionable Result. Dev/e2e
            // keep the tolerant log path. Mirrors ClaudeCodeRuntime::spawn.
            if crate::runtime::is_packaged() {
                return Err("The Lucidos CLI is required for coding-agent \
                    permission/question tools but was not found alongside the engine \
                    (expected LUCIDOS_CLI_BIN or a sibling `lucidos` binary)"
                    .into());
            }
            crate::log!(
                "[Codex] lucidos CLI binary not found near current_exe — \
                 spawned Codex sessions won't have the `lucidos` command on PATH \
                 (build with `cargo build -p lucidos-cli`)"
            );
        }
        ensure_workspace_bin_symlink(args.worktree_path, cli_dir);

        let sandbox_writable_roots =
            sandbox_writable_roots(args.worktree_path, args.workspace_path).await;

        // Bake the Lucidos env contract once; each per-turn child re-applies it.
        let env = {
            let mut probe = tokio::process::Command::new("true");
            apply_lucidos_env(&mut probe, &args, cli_dir, "Codex");
            probe
                .as_std()
                .get_envs()
                .filter_map(|(k, v)| v.map(|v| (k.to_owned(), v.to_owned())))
                .collect()
        };

        // A user-configured `codex` path must point at a real executable —
        // fail the spawn naming the setting rather than silently probing past
        // a typo (see `spawn_env::resolve_binary_override`).
        if let Some(path) = args.binary_override {
            super::spawn_env::resolve_binary_override(
                path,
                "Codex (`codex`)",
                "coding_agent_codex_path",
            )?;
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let config = CodexConfig {
            codex_bin: resolve_codex_binary(home.as_deref(), args.binary_override.map(Path::new)),
            worktree_path: args.worktree_path.to_path_buf(),
            system_prompt: args.system_prompt.map(str::to_string),
            model: args.model.map(str::to_string),
            reasoning_effort: args.reasoning_effort.map(str::to_string),
            sandbox_writable_roots,
            env,
        };

        let (events_tx, events_rx) = mpsc::unbounded_channel::<AgentEvent>();
        let (input_tx, input_rx) = mpsc::unbounded_channel::<AgentInput>();
        let (control_tx, control_rx) = mpsc::unbounded_channel::<ControlRequest>();

        let resume_session_id = args.resume_session_id.map(str::to_string);
        let continuation = args.continuation;
        let protocol =
            codex_protocol_from_env(std::env::var("LUCIDOS_CODEX_PROTOCOL").ok().as_deref());
        let permission_rx = match protocol {
            CodexProtocol::AppServer => {
                let (permission_tx, permission_rx) = mpsc::unbounded_channel();
                tokio::spawn(super::codex_app_server::app_server_driver_task(
                    config,
                    resume_session_id,
                    continuation,
                    events_tx,
                    input_rx,
                    control_rx,
                    permission_tx,
                    cancel,
                ));
                Some(permission_rx)
            }
            CodexProtocol::Exec => {
                tokio::spawn(driver_task(
                    config,
                    resume_session_id,
                    continuation,
                    events_tx,
                    input_rx,
                    control_rx,
                    cancel,
                ));
                // The exec protocol has no approval channel — the OS sandbox
                // is the only guard (ADR 0004 §4; ADR 0005 keeps this as the
                // escape-hatch model).
                None
            }
        };

        Ok(RunningAgent {
            kind: CodingAgent::Codex,
            events_rx,
            input_tx,
            control_tx,
            permission_rx,
        })
    }
}

/// The extra writable roots a Codex session needs beyond its own worktree,
/// which `--sandbox workspace-write` already grants. Every entry is a
/// deliberate hole in the sandbox:
///
/// 1. **The worktree's shared git dir.** A linked worktree's `.git` is a file
///    pointing at `<main>/.git/worktrees/<x>`, so an in-agent `git commit`
///    writes outside the worktree. Omitted when git can't tell us (degrade).
/// 2. **The workspace's `data/` tree.** Writing there is the documented
///    contract for a coding-agent thread: `lucidos data write` /
///    `lucidos data path` resolve under `<workspace>/data/`, and the workspace
///    knowhow instructs agents to log follow-ups to
///    `artifacts/work-tracker/data.json`. That path is OUTSIDE the worktree, so
///    the macOS seatbelt refused it with `EPERM (os error 1)` — which is how
///    the 2026-07-26 nightly's Codex security pass silently failed to persist
///    two high-severity findings. Claude Code runs unsandboxed and never hit
///    it, so the contract looked like it worked.
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
        // Canonicalized for the same reason the data dir below is: the seatbelt
        // matches REAL paths, so a workspace reached through a symlink (a
        // `/tmp/...` one resolves to `/private/tmp/...` on macOS) would grant a
        // root the kernel never matches — and in-agent `git commit`, the only
        // reason this entry exists, would stay blocked. Falls back to the
        // as-resolved path if it has since vanished.
        roots.push(std::fs::canonicalize(&git_dir).unwrap_or(git_dir));
    }
    let data_dir = workspace.join(crate::core::DATA_DIR);
    // `canonicalize` does three jobs here, all load-bearing:
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
        Some(dir) if widens_past_the_workspace(&dir, workspace) => crate::log!(
            "[Codex] {} resolves to {}, which contains the workspace — refusing \
             to grant it; the sandbox will block `lucidos data write`",
            data_dir.display(),
            dir.display()
        ),
        Some(dir) => roots.push(dir),
        None => crate::log!(
            "[Codex] {} is not a reachable directory — the sandbox will block \
             `lucidos data write` to the parent workspace",
            data_dir.display()
        ),
    }
    roots
}

/// Does this resolved `data/` path grant MORE than the data tree — the
/// workspace root itself, or an ancestor of it (`/` included)?
///
/// Resolving symlinks is what the sandbox needs (see `sandbox_writable_roots`),
/// but it also means a `data` symlink decides the hole's width. Relocating the
/// tree (`data -> /Volumes/ext/lucidos-data`) is legitimate and stays allowed;
/// WIDENING it (`data -> .`, `data -> /`) is not, because it silently grants
/// the `.lucidos/` runtime and every sibling worktree — the exact scoping this
/// function's doc comment promises it withholds. Cheaper to make impossible
/// than to document as a footgun.
fn widens_past_the_workspace(resolved_data: &Path, workspace: &Path) -> bool {
    let workspace = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    workspace.starts_with(resolved_data)
}

/// `git rev-parse --git-common-dir` for the worktree. `None` (log + degrade)
/// when git fails — Codex still runs, but its own `git commit` would be
/// blocked by the sandbox; the engine's auto-commit (which runs unsandboxed)
/// still captures the work.
async fn resolve_git_common_dir(worktree: &Path) -> Option<PathBuf> {
    let out = crate::engine::git_ops::git_cmd(&["rev-parse", "--git-common-dir"], worktree)
        .await
        .ok()?;
    if !out.status.success() {
        crate::log!(
            "[Codex] git rev-parse --git-common-dir failed in {} — sandbox will block in-agent git commits",
            worktree.display()
        );
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(&raw);
    let abs = if path.is_absolute() {
        path
    } else {
        worktree.join(path)
    };
    Some(abs)
}

/// Build one per-turn `codex exec` command. Pure over its inputs so the unit
/// tests can pin flag layout without spawning.
fn build_codex_turn_command(
    config: &CodexConfig,
    model: Option<&str>,
    effort: Option<&str>,
    resume_session_id: Option<&str>,
    prompt: &str,
    image_paths: &[PathBuf],
) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(&config.codex_bin);
    cmd.arg("exec").arg("--json");
    // Worktree-scoped writes, like CC's acceptEdits-in-cwd. Network stays on:
    // coding tasks need cargo/npm fetches, same as a CC session would run.
    cmd.arg("--sandbox").arg("workspace-write");
    cmd.arg("-c")
        .arg("sandbox_workspace_write.network_access=true");
    // Reasoning summaries + CLAUDE.md project-doc fallback — see the
    // CODEX_REASONING_SUMMARY / CODEX_PROJECT_DOC_FALLBACKS docs. TOML value
    // syntax: `-c` values parse as TOML, so the list renders as `["..."]`.
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
    for flag in lucidos_mcp_server_config_overrides(&config.env) {
        cmd.arg("-c").arg(flag);
    }
    for root in &config.sandbox_writable_roots {
        cmd.arg("--add-dir").arg(root);
    }
    // "default" = let the user's Codex config pick (mirrors CC's sentinel).
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
    // Global exec flags must precede the `resume` subcommand; the prompt is
    // positional in both forms.
    if let Some(sid) = resume_session_id {
        cmd.arg("resume").arg(sid);
    }
    cmd.arg(prompt);
    cmd.current_dir(&config.worktree_path)
        // /dev/null stdin — codex falls back to reading the prompt from
        // stdin when it considers the arg incomplete; an inherited fd would
        // make it block on "Reading additional input from stdin...".
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (k, v) in &config.env {
        cmd.env(k, v);
    }
    // Own process group so a group-wide signal to the engine can't truncate
    // the turn — same isolation CC gets. (Codex's env is baked from a probe,
    // so this Command attribute must be set on the real command directly.)
    super::spawn_env::isolate_in_process_group(&mut cmd);
    cmd
}

/// `-c key=value` overrides wiring the lucidos MCP server into the exec
/// driver's per-turn CLI invocation. Derived mechanically from
/// [`lucidos_mcp_server_config_json`] — one source of truth rendered as JSON
/// for app-server and as TOML-compatible `-c` values for exec.
///
/// Keys verified against codex-cli 0.141.0 (`codex mcp list -c …` accepts
/// this shape; unknown keys would fail config parse and kill the session).
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

/// Wire name of the question tool as it appears in `CodingAgentToolCalled`
/// events — codex prefixes MCP tools with `mcp__<server>__`. The engine's
/// run loop suppresses the tool-call event for this name (the
/// `UserQuestionAsked` emitted by the internal endpoint renders the card; a
/// tool-call step on top would double-surface the same question).
pub const CODEX_ASK_USER_QUESTION_TOOL: &str = "mcp__lucidos__ask_user_question";

/// The lucidos MCP server config (`lucidos mcp-permission-server`, same
/// binary CC spawns) — the SINGLE source for both Codex drivers: the
/// app-server driver embeds it in `thread/start`'s `config` object, the
/// exec driver derives `-c` flags from it via
/// [`lucidos_mcp_server_config_overrides`]. It gives the model a
/// QuestionCard path via the `ask_user_question` tool:
///
/// - `enabled_tools` hides the CC-only `approve` permission tool — without it
///   Codex would advertise `mcp__lucidos__approve` to the model as an
///   ordinary callable tool.
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
/// - `command` is the bare binary name: `apply_lucidos_env` prepends the
///   bundled CLI dir to `PATH`, which codex itself uses to find the command.
///   The dir resolves via `LUCIDOS_CLI_BIN` in a packaged build (the staged
///   `<resources>/lucidos`) and via the exe sibling-walk in dev — see
///   `lucidos_cli::resolve_cli_dir`. A packaged build with the CLI unstaged
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
    obj.insert("tool_timeout_sec".to_string(), 86400.into());
    serde_json::Value::Object(obj)
}

const LUCIDOS_MCP_CHILD_ENV_KEYS: &[&str] = &[
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

/// Which protocol drives a Codex session. Selected per spawn from
/// `LUCIDOS_CODEX_PROTOCOL`; the session id (Codex thread id) is shared
/// on-disk state, so an existing thread resumes fine after a flip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CodexProtocol {
    /// Persistent `codex app-server` child speaking JSON-RPC (default) —
    /// permission cards, per-token streaming, graceful interrupt. See
    /// ADR 0005.
    AppServer,
    /// Per-turn `codex exec --json` children — the escape hatch for when an
    /// upstream codex release breaks the (experimental) app-server contract.
    Exec,
}

/// Parse `LUCIDOS_CODEX_PROTOCOL`. Default app-server (the reason ADR 0005
/// exists); only an explicit `exec` opts out. Unknown values log and fall
/// back to the default rather than failing the spawn.
pub(super) fn codex_protocol_from_env(value: Option<&str>) -> CodexProtocol {
    match value {
        Some("exec") => CodexProtocol::Exec,
        Some("app-server") | None => CodexProtocol::AppServer,
        Some(other) => {
            crate::log!(
                "[Codex] Unknown LUCIDOS_CODEX_PROTOCOL={:?} — defaulting to app-server",
                other
            );
            CodexProtocol::AppServer
        }
    }
}

/// First fresh turn carries the engine's session instructions inline —
/// `codex exec` has no `--append-system-prompt` equivalent, and resumed turns
/// already have them in the Codex-side conversation history.
fn compose_first_turn_prompt(system_prompt: Option<&str>, user_text: &str) -> String {
    match system_prompt {
        Some(sp) if !sp.is_empty() => format!(
            "# Session instructions (from the Lucidos engine)\n\n{}\n\n---\n\n{}",
            sp, user_text
        ),
        _ => user_text.to_string(),
    }
}

/// Materialize pasted images as temp files (`-i` flags on exec; `localImage`
/// inputs on app-server). Returns the paths plus guards that delete the
/// files on drop (kept alive until the turn ends).
pub(super) fn write_image_files(
    images: &[crate::api::ChatImage],
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
                crate::log!("[Codex] dropping undecodable pasted image: {}", e);
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
                crate::log!("[Codex] failed to create temp image file: {}", e);
                continue;
            }
        };
        if let Err(e) = std::fs::write(file.path(), &bytes) {
            crate::log!("[Codex] failed to write temp image file: {}", e);
            continue;
        }
        let path = file.path().to_path_buf();
        paths.push(path);
        guards.push(file.into_temp_path());
    }
    (paths, guards)
}

/// Outcome of one turn — tells the driver loop what to do next.
enum TurnOutcome {
    /// Turn ended (completed, failed, interrupted, or synthesized) — keep
    /// serving inputs. Interrupt deliberately does NOT drop queued inputs:
    /// the engine counts every forwarded follow-up in `pending_followups`
    /// and waits for one Result each — dropping them would wedge the thread
    /// at idle. Matches CC, whose stdin-queued messages also run after Esc.
    Continue,
    /// Cancellation token fired or the consumer vanished — wind down.
    Shutdown,
}

/// Drive a session: one `codex exec` child per accepted input.
async fn driver_task(
    config: CodexConfig,
    resume_session_id: Option<String>,
    continuation: bool,
    events_tx: mpsc::UnboundedSender<AgentEvent>,
    mut input_rx: mpsc::UnboundedReceiver<AgentInput>,
    mut control_rx: mpsc::UnboundedReceiver<ControlRequest>,
    cancel: CancellationToken,
) {
    let mut tracker = TurnTracker::new(resume_session_id);
    let mut model = config.model.clone();
    let mut effort = config.reasoning_effort.clone();
    let mut queue: VecDeque<AgentInput> = VecDeque::new();
    if continuation {
        // Engine resumes a mid-turn-interrupted session with no new input —
        // see `SpawnArgs::continuation`. CC auto-injects this; Codex needs it
        // as an explicit prompt.
        queue.push_back(AgentInput {
            text: CONTINUATION_PROMPT.to_string(),
            images: Vec::new(),
        });
    }

    'session: loop {
        // Idle between turns: wait for the next input or a reason to stop.
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
                        Some(req) => apply_idle_control(req, &mut model, &mut effort),
                        None => break 'session,
                    }
                }
                _ = cancel.cancelled() => {
                    log!("[Codex] cancellation signalled while idle — ending session");
                    break 'session;
                }
            }
        }
        let input = queue.pop_front().expect("queue non-empty");

        let outcome = run_turn(
            &config,
            &mut tracker,
            &mut model,
            &mut effort,
            input,
            &events_tx,
            &mut input_rx,
            &mut control_rx,
            &mut queue,
            &cancel,
        )
        .await;
        match outcome {
            TurnOutcome::Continue => {}
            TurnOutcome::Shutdown => break 'session,
        }
    }

    // Codex (exec mode) spawns a fresh child per turn — there is no single
    // long-lived process whose stray-signal death the safety net could resume.
    // Auto-resume on signal-kill stays CC-only.
    let _ = events_tx.send(AgentEvent::Exited {
        killed_by_signal: false,
    });
    // events_tx drops here — channel closes, consumer sees None.
}

/// Apply a control request while no turn is running. Interrupt at idle is a
/// no-op (nothing to interrupt); model/effort updates take effect next turn.
fn apply_idle_control(
    req: ControlRequest,
    model: &mut Option<String>,
    effort: &mut Option<String>,
) {
    match req {
        ControlRequest::Interrupt => {}
        ControlRequest::SetModel { model: m } => *model = Some(m),
        ControlRequest::SetReasoningEffort { effort: e } => *effort = Some(e),
        // No permission protocol — Codex runs under the OS sandbox.
        ControlRequest::SetPermissionMode { .. } => {
            log!("[Codex] SetPermissionMode is a no-op for the Codex backend");
        }
    }
}

/// Run one `codex exec` child to completion, forwarding events.
/// `model` / `effort` are `&mut` because a mid-turn `SetModel` /
/// `SetReasoningEffort` control request lands here and must take effect on
/// the NEXT per-turn child.
#[allow(clippy::too_many_arguments)]
async fn run_turn(
    config: &CodexConfig,
    tracker: &mut TurnTracker,
    model: &mut Option<String>,
    effort: &mut Option<String>,
    input: AgentInput,
    events_tx: &mpsc::UnboundedSender<AgentEvent>,
    input_rx: &mut mpsc::UnboundedReceiver<AgentInput>,
    control_rx: &mut mpsc::UnboundedReceiver<ControlRequest>,
    queue: &mut VecDeque<AgentInput>,
    cancel: &CancellationToken,
) -> TurnOutcome {
    tracker.begin_turn();
    let turn_start = std::time::Instant::now();

    let (image_paths, _image_guards) = write_image_files(&input.images);
    let text = if input.text.is_empty() && !image_paths.is_empty() {
        // codex exec requires a prompt argument; an empty positional makes it
        // fall back to stdin (nulled) and produce an empty turn.
        "See the attached image(s).".to_string()
    } else {
        input.text
    };
    let resume_sid = tracker.session_id.clone();
    let prompt = if resume_sid.is_none() {
        compose_first_turn_prompt(config.system_prompt.as_deref(), &text)
    } else {
        text
    };

    let turn_model = model.clone();
    let turn_effort = effort.clone();
    let mut cmd = build_codex_turn_command(
        config,
        turn_model.as_deref(),
        turn_effort.as_deref(),
        resume_sid.as_deref(),
        &prompt,
        &image_paths,
    );
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log!("[Codex] failed to spawn codex CLI: {}", e);
            let _ = events_tx.send(AgentEvent::Result {
                text: String::new(),
                duration_ms: 0,
                error: Some(format!("Failed to start Codex: {e}")),
            });
            return TurnOutcome::Continue;
        }
    };
    let child_pid = child.id();
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let mut stdout_reader = BufReader::new(stdout);
    let mut stderr_reader = BufReader::new(stderr);

    let mut line_buf = String::new();
    let mut interrupted = false;
    let mut shutdown = false;
    'turn: loop {
        tokio::select! {
            read_result = stdout_reader.read_line(&mut line_buf) => {
                match read_result {
                    Ok(0) => break 'turn,
                    Ok(_) => {
                        let events = tracker.map_line(
                            parse_codex_line(&line_buf),
                            turn_start.elapsed().as_millis() as u64,
                        );
                        line_buf.clear();
                        for ev in events {
                            if events_tx.send(ev).is_err() {
                                // Consumer dropped — abandon the process.
                                shutdown = true;
                                break 'turn;
                            }
                        }
                    }
                    Err(e) => {
                        log!("[Codex] stdout read error: {}", e);
                        break 'turn;
                    }
                }
            }
            input = input_rx.recv() => {
                match input {
                    // Codex can't take mid-turn injections — queue for the
                    // next per-turn child so every accepted input still gets
                    // its own turn (and its own Result).
                    Some(i) => queue.push_back(i),
                    None => {
                        shutdown = true;
                        break 'turn;
                    }
                }
            }
            req = control_rx.recv() => {
                match req {
                    Some(ControlRequest::Interrupt) => {
                        log!("[Codex] interrupt — killing in-flight codex turn (pid={:?})", child_pid);
                        interrupted = true;
                        break 'turn;
                    }
                    Some(ControlRequest::SetModel { model: m }) => *model = Some(m),
                    Some(ControlRequest::SetReasoningEffort { effort: e }) => *effort = Some(e),
                    Some(ControlRequest::SetPermissionMode { .. }) => {
                        log!("[Codex] SetPermissionMode is a no-op for the Codex backend");
                    }
                    None => {
                        shutdown = true;
                        break 'turn;
                    }
                }
            }
            _ = cancel.cancelled() => {
                log!("[Codex] cancellation signalled — killing codex turn");
                shutdown = true;
                break 'turn;
            }
        }
    }

    // Reap the child: clean exit path lets it finish; interrupt/shutdown kill it.
    let wait_result = if interrupted || shutdown {
        let _ = child.start_kill();
        child.wait().await
    } else {
        // tokio's `read_line` is not cancel-safe: when another select arm
        // wins a poll cycle, a line already copied into `line_buf` would be
        // silently dropped — and the next read would append fresh bytes to
        // it, corrupting the parse (same defect the CC driver documents).
        // Forward any pre-captured line before draining.
        if !line_buf.is_empty() {
            for ev in tracker.map_line(
                parse_codex_line(&line_buf),
                turn_start.elapsed().as_millis() as u64,
            ) {
                let _ = events_tx.send(ev);
            }
            line_buf.clear();
        }
        // stdout EOF — drain whatever the OS still buffers, then wait.
        let drain_deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            match tokio::time::timeout_at(drain_deadline, stdout_reader.read_line(&mut line_buf))
                .await
            {
                Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
                Ok(Ok(_)) => {
                    let events = tracker.map_line(
                        parse_codex_line(&line_buf),
                        turn_start.elapsed().as_millis() as u64,
                    );
                    line_buf.clear();
                    for ev in events {
                        let _ = events_tx.send(ev);
                    }
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

    let stderr_tail = drain_stderr(&mut stderr_reader).await;
    if !stderr_tail.is_empty() {
        log!("[Codex] codex stderr: {}", stderr_tail.trim());
    }
    log!(
        "[Codex] turn child exited (pid={} status={})",
        child_pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "?".to_string()),
        format_exit_status(&wait_result),
    );

    if shutdown {
        return TurnOutcome::Shutdown;
    }
    if interrupted {
        // The engine's interrupt arm waits for a Result to close the turn as
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
        });
        return TurnOutcome::Continue;
    }
    if !tracker.turn_terminal_seen {
        // Child died without `turn.completed` / `turn.failed` (auth failure,
        // panic, OOM-kill, …). Synthesize the failed Result so the engine
        // shows a red dot instead of waiting forever.
        for ev in tracker.close_open_tools() {
            let _ = events_tx.send(ev);
        }
        let reason = tracker
            .last_error
            .clone()
            .or_else(|| {
                let t = stderr_tail.trim();
                (!t.is_empty()).then(|| t.to_string())
            })
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
