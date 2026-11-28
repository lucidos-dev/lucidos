pub mod agent_notes;
pub mod agent_profile;
pub mod agent_run_marker;
pub mod agent_runtime;
pub mod agent_sign_in;
pub use crate::core::agent_sandbox;
pub use crate::engine::thread_events::billing_provider;
pub mod browser;
mod browser_consent;
pub use crate::llm::cc_model_discovery;
pub mod chat_ladder;
mod chat_ladder_policy;
pub mod claude_code;
pub mod claude_code_notes;
pub mod codex;
mod codex_app_server;
mod codex_app_server_parse;
mod codex_parse;
pub mod codex_side_question;
pub(crate) mod compile_mode;
pub mod intelligence_ladder;
mod intelligence_ladder_policy;
pub mod lucidos_cli;
pub mod pty;
pub mod python;
pub mod spawn_env;
pub(crate) mod startup_instructions;
#[cfg(test)]
pub(crate) mod stub_executable;
pub(crate) use crate::core::temp_dirs;

#[cfg(test)]
use crate::llm::coding_agent_models::{
    curated_coding_agent_model_options, effort_within_row_tiers, offered_row,
    validate_model_against,
};
#[cfg(test)]
use crate::llm::intelligence_rungs::strip_version_pin;

pub use agent_runtime::{
    AccountPin, AgentEvent, AgentExitFacts, AgentInput, AgentPermissionRequest, AgentRuntime,
    CodingAgent, ControlRequest, InputWithdrawal, RunningAgent, SessionContextBlock, SpawnArgs,
    WithdrawRequest,
};
pub use browser::{BrowserLogins, BrowserRuntime, HeadlessBlocklist};
pub use claude_code::{
    ClaudeCodeRuntime, CC_MCP_ASK_USER_QUESTION_TOOL, CC_NATIVE_ASK_USER_QUESTION_TOOL,
    CC_PERMISSION_PROMPT_TOOL,
};
pub use codex::{CodexRuntime, CODEX_ASK_USER_QUESTION_TOOL};
pub use python::PythonRuntime;

pub use crate::llm::coding_agent_models::{
    coding_agent_context_window, coding_agent_model_options, coding_agent_reasoning_effort_options,
    validate_coding_agent_effort, validate_coding_agent_model,
};

/// INVARIANT: True for every wire tool name that raises a Lucidos QuestionCard.
///
/// ONE list, because two separate gates key on it and a name missing from
/// either produces a different broken flow:
///
/// - `api::internal::permission_prompt` auto-allows these, so the user is not
///   asked to approve the question before being shown it.
/// - `agent_session::run_session` suppresses their `CodingAgentToolCalled`, so
///   the `UserQuestionAsked` card is the only thing on the timeline instead of
///   a tool-call step stacked above it.
///
/// THREE names for two backends, which is the trap this function exists for.
/// Claude Code has its own built-in tool AND can reach the MCP one, because the
/// permission server it already mounts advertises `ask_user_question` alongside
/// `approve`; Codex mounts that same server under a different name. Both gates
/// knew only CC's native name and Codex's, so a CC session calling the MCP tool
/// got a permission card first and then a stray pending step above the question
/// for as long as the user took to answer (observed 2026-08-09).
///
/// Codex's name is accepted at the permission gate too. Nothing routes a Codex
/// call through `--permission-prompt-tool` (that flag is CC's), so it is inert
/// there; splitting the list per gate to express that would be two lists to
/// keep in step, which is the failure above.
///
/// SUPPRESSION IS THE EMIT ONLY. The `ToolUse` still increments
/// `tools_in_flight` at the run-loop call site, BEFORE this branch, because the
/// user may take ten minutes to answer and the watchdog must stay disarmed for
/// all of it. Folding that increment into the else arm on the reading that a
/// question tool_use is a no-op would euthanize the session mid-question.
pub fn is_user_question_tool(name: &str) -> bool {
    USER_QUESTION_TOOLS.contains(&name)
}

pub const USER_QUESTION_TOOLS: [&str; 3] = [
    CC_NATIVE_ASK_USER_QUESTION_TOOL,
    CC_MCP_ASK_USER_QUESTION_TOOL,
    CODEX_ASK_USER_QUESTION_TOOL,
];

/// INVARIANT: Whether the engine is running inside a packaged build (the macOS `.app` /
/// headless tarball / `install.sh` service all set `LUCIDOS_PACKAGED=1` — see
/// `desktop.rs::spawn_gateway` and `service.sh::service_runtime_env_pairs`).
/// Dev / e2e leave it unset. Used to fail fast on a missing bundled runtime
/// dependency in packaged while keeping dev's tolerant degrade-with-log path.
pub fn is_packaged() -> bool {
    std::env::var_os("LUCIDOS_PACKAGED").is_some_and(|v| v == "1")
}

/// INVARIANT: Effective resolution of a coding agent's CLI binary — the payload of
/// `GET /api/v1/coding-agents/binaries` (Settings → Coding Agents). Live
/// detection, recomputed per request: only an explicit user override is ever
/// persisted (the `coding_agent_*_path` preference), so a Homebrew upgrade or
/// installer move self-heals instead of leaving a stale stored path.
#[derive(Debug, serde::Serialize)]
pub struct AgentBinaryStatus {
    pub path: Option<String>,
    pub source: &'static str,
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

pub async fn installed_coding_agents(pool: &sqlx::PgPool) -> Vec<CodingAgent> {
    let claude = crate::core::prefs::CODING_AGENT_CLAUDE_PATH
        .read(pool)
        .await;
    let codex = crate::core::prefs::CODING_AGENT_CODEX_PATH.read(pool).await;
    [CodingAgent::ClaudeCode, CodingAgent::Codex]
        .into_iter()
        .filter(|agent| {
            let path = match agent {
                CodingAgent::ClaudeCode => claude.as_deref(),
                CodingAgent::Codex => codex.as_deref(),
            };
            detect_agent_binary(*agent, path).valid
        })
        .collect()
}

pub fn detect_agent_binary(agent: CodingAgent, override_path: Option<&str>) -> AgentBinaryStatus {
    if let Some(raw) = override_path {
        let (label, pref_key) = match agent {
            CodingAgent::ClaudeCode => (
                "Claude Code (`claude`)",
                crate::core::prefs::CODING_AGENT_CLAUDE_PATH.key(),
            ),
            CodingAgent::Codex => (
                "Codex (`codex`)",
                crate::core::prefs::CODING_AGENT_CODEX_PATH.key(),
            ),
        };
        return match spawn_env::resolve_binary_override(raw, label, pref_key) {
            Ok(p) => AgentBinaryStatus {
                path: Some(p.display().to_string()),
                source: "override",
                valid: true,
                error: None,
                version: None,
            },
            Err(e) => AgentBinaryStatus {
                path: Some(raw.to_string()),
                source: "override",
                valid: false,
                error: Some(e.to_string()),
                version: None,
            },
        };
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let resolved = match agent {
        CodingAgent::ClaudeCode => claude_code::resolve_claude_binary(home.as_deref(), None),
        CodingAgent::Codex => codex::resolve_codex_binary(home.as_deref(), None),
    };
    if std::path::Path::new(&resolved).is_absolute() {
        return AgentBinaryStatus {
            path: Some(resolved.to_string_lossy().into_owned()),
            source: "detected",
            valid: true,
            error: None,
            version: None,
        };
    }
    let path_hit = spawn_env::find_on_path(&resolved, std::env::var_os("PATH").as_deref());
    match path_hit {
        Some(found) => AgentBinaryStatus {
            path: Some(found.display().to_string()),
            source: "path",
            valid: true,
            error: None,
            version: None,
        },
        None => AgentBinaryStatus {
            path: None,
            source: "not-found",
            valid: false,
            error: None,
            version: None,
        },
    }
}

const VERSION_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub async fn detect_agent_binary_with_version(
    agent: CodingAgent,
    override_path: Option<&str>,
) -> AgentBinaryStatus {
    detect_agent_binary_within(agent, override_path, VERSION_PROBE_TIMEOUT).await
}

async fn detect_agent_binary_within(
    agent: CodingAgent,
    override_path: Option<&str>,
    timeout: std::time::Duration,
) -> AgentBinaryStatus {
    let mut status = detect_agent_binary(agent, override_path);
    if let Some(binary) = version_probe_target(&status).map(std::path::PathBuf::from) {
        status.version = probe_agent_version(agent, &binary, timeout).await;
    }
    status
}

fn version_probe_target(status: &AgentBinaryStatus) -> Option<&str> {
    if !status.valid {
        return None;
    }
    status.path.as_deref()
}

/// INVARIANT: The most bytes either pipe of a `--version` probe may contribute. The answer
/// is one short line, so anything past this is a binary that is not the CLI we
/// asked for.
///
/// The cap is what bounds the probe in MEMORY, and the timeout alone does not:
/// `Command::output` reads each pipe to EOF, so a binary that streams would
/// have the engine buffer everything it could produce in the whole 5 s, twice
/// over (both agents are probed concurrently).
const VERSION_PROBE_OUTPUT_LIMIT: u64 = 8 * 1024;

const VERSION_PROBE_LOG_EXCERPT: usize = 200;

/// INVARIANT: Ask a resolved coding-agent binary for its version.
///
/// Bounded in time, memory, and aftermath: `--version` on a healthy CLI answers
/// in milliseconds, but this runs on a user-facing Settings request, so a wedged
/// or wrong binary must not hold it open ([`VERSION_PROBE_TIMEOUT`]), must not
/// have the engine buffer whatever it feels like printing
/// ([`VERSION_PROBE_OUTPUT_LIMIT`]), and must not outlive the request
/// (`kill_on_drop` reaps the child along with the abandoned future).
///
/// Every failure branch degrades to `None` and logs the reason. That is an
/// absence, not a swallowed error: the Settings section's contract is WHICH
/// binary runs, which `AgentBinaryStatus` reports in full either way, and the
/// version is additive. Reporting "unknown" or a placeholder version instead
/// would be exactly the silent default the no-silent-defaults rule bans.
pub(crate) async fn probe_agent_version(
    agent: CodingAgent,
    binary: &std::path::Path,
    timeout: std::time::Duration,
) -> Option<String> {
    probe_agent_version_command(
        agent,
        binary,
        timeout,
        &mut tokio::process::Command::new(binary),
    )
    .await
}

async fn probe_agent_version_command(
    agent: CodingAgent,
    binary: &std::path::Path,
    timeout: std::time::Duration,
    cmd: &mut tokio::process::Command,
) -> Option<String> {
    let label = agent.as_str();
    let shown = binary.display();
    cmd.arg("--version")
        // INVARIANT: stdin is the one stdio tokio does NOT set for us, so closing it here
        // is load-bearing: a binary that isn't the CLI we think it is would
        // otherwise sit reading the engine's own stdin until the timeout.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            crate::log_legacy!("[AgentBinary] {label}: `{shown} --version` failed to run: {e}");
            return None;
        }
    };
    let out_pipe = child.stdout.take();
    let err_pipe = child.stderr.take();
    let out_ctx = format!("{label}: `{shown} --version` stdout");
    let err_ctx = format!("{label}: `{shown} --version` stderr");
    let collected = tokio::time::timeout(timeout, async {
        tokio::join!(
            read_capped(out_pipe, VERSION_PROBE_OUTPUT_LIMIT, &out_ctx),
            read_capped(err_pipe, VERSION_PROBE_OUTPUT_LIMIT, &err_ctx),
            child.wait(),
        )
    })
    .await;
    let (stdout, stderr, waited) = match collected {
        Ok(collected) => collected,
        Err(_) => {
            crate::log_legacy!(
                "[AgentBinary] {label}: `{shown} --version` timed out after {timeout:?}"
            );
            return None;
        }
    };
    let status = match waited {
        Ok(status) => status,
        Err(e) => {
            crate::log_legacy!(
                "[AgentBinary] {label}: waiting for `{shown} --version` failed: {e}"
            );
            return None;
        }
    };
    if !status.success() {
        crate::log_legacy!(
            "[AgentBinary] {label}: `{shown} --version` exited {status}: {}",
            log_excerpt(&stderr, VERSION_PROBE_LOG_EXCERPT)
        );
        return None;
    }
    let parsed = parse_agent_version(&stdout);
    if parsed.is_none() {
        crate::log_legacy!(
            "[AgentBinary] {label}: `{shown} --version` printed no recognizable version: '{}'",
            log_excerpt(&stdout, VERSION_PROBE_LOG_EXCERPT)
        );
    }
    parsed
}

/// INVARIANT: KEEP at most `limit` bytes from one of a probe's pipes, as lossy UTF-8, then
/// drain the rest without keeping it.
///
/// Both halves matter. Keeping only `limit` is what bounds memory. Draining the
/// remainder anyway is what stops that bound from costing correctness: a child
/// whose pipe fills blocks on the write and never exits, so merely stopping at
/// the cap would turn a legitimate CLI with a chatty banner into a timeout.
/// The drain is bounded in time by the caller's timeout and in memory by the
/// scratch buffer.
///
/// `context` names the agent, command and stream for the one thing worth
/// logging here. A read error is reported but not propagated: whatever arrived
/// before it is still worth parsing, and the caller already logs the output it
/// could not recognize, which is the actionable half.
async fn read_capped(
    pipe: Option<impl tokio::io::AsyncRead + Unpin>,
    limit: u64,
    context: &str,
) -> String {
    use tokio::io::AsyncReadExt as _;
    let Some(mut pipe) = pipe else {
        return String::new();
    };
    let mut kept = Vec::new();
    if let Err(e) = (&mut pipe).take(limit).read_to_end(&mut kept).await {
        crate::log_legacy!("[AgentBinary] {context} read failed: {e}");
        return String::from_utf8_lossy(&kept).into_owned();
    }
    let mut sink = [0u8; 4096];
    loop {
        match pipe.read(&mut sink).await {
            Ok(0) => break,
            Ok(_) => continue,
            Err(e) => {
                crate::log_legacy!("[AgentBinary] {context} drain failed: {e}");
                break;
            }
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}

fn log_excerpt(text: &str, max: usize) -> &str {
    let text = text.trim();
    &text[..text.floor_char_boundary(max)]
}

fn parse_agent_version(output: &str) -> Option<String> {
    output
        .lines()
        .next()?
        .split_whitespace()
        .find_map(version_token)
        .map(str::to_string)
}

fn version_token(word: &str) -> Option<&str> {
    let trimmed = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let core = trimmed.strip_prefix('v').unwrap_or(trimmed);
    (core.starts_with(|c: char| c.is_ascii_digit()) && core.contains('.')).then_some(core)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_shapes_both_shipped_agents_print() {
        assert_eq!(
            parse_agent_version("2.1.224 (Claude Code)\n").as_deref(),
            Some("2.1.224")
        );
        assert_eq!(
            parse_agent_version("codex-cli 0.147.0\n").as_deref(),
            Some("0.147.0")
        );
    }

    #[test]
    fn keeps_a_prerelease_suffix_and_strips_a_leading_v() {
        assert_eq!(
            parse_agent_version("v1.0.0-beta.3").as_deref(),
            Some("1.0.0-beta.3")
        );
        assert_eq!(
            parse_agent_version("mytool version 2.39.5").as_deref(),
            Some("2.39.5")
        );
    }

    #[test]
    fn reads_only_the_first_line() {
        assert_eq!(
            parse_agent_version("3.2.1 (Some CLI)\nloaded /home/u/.config/some.json\n").as_deref(),
            Some("3.2.1")
        );
    }

    #[test]
    fn returns_none_rather_than_guessing() {
        assert_eq!(parse_agent_version("no version here"), None);
        assert_eq!(parse_agent_version("build 20260808"), None);
        assert_eq!(parse_agent_version(""), None);
        assert_eq!(parse_agent_version("\n"), None);
    }

    fn status(source: &'static str, path: Option<&str>, valid: bool) -> AgentBinaryStatus {
        AgentBinaryStatus {
            path: path.map(str::to_string),
            source,
            valid,
            error: None,
            version: None,
        }
    }

    #[test]
    fn probes_only_a_resolved_valid_binary() {
        assert_eq!(
            version_probe_target(&status("detected", Some("/y/claude"), true)),
            Some("/y/claude")
        );
        assert_eq!(
            version_probe_target(&status("path", Some("/z/codex"), true)),
            Some("/z/codex")
        );
        assert_eq!(
            version_probe_target(&status("override", Some("/x/claude"), true)),
            Some("/x/claude")
        );
    }

    #[test]
    fn never_probes_an_invalid_override_or_a_missing_binary() {
        assert_eq!(
            version_probe_target(&status("override", Some("/typo/claude"), false)),
            None,
            "a path that isn't an executable file must never be executed"
        );
        assert_eq!(
            version_probe_target(&status("not-found", None, false)),
            None
        );
    }

    fn fake_binary(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        stub_executable::write_stub_executable(&path, &format!("#!/bin/sh\n{body}\n"));
        path
    }

    const UNREACHABLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

    #[cfg(unix)]
    #[tokio::test]
    async fn probe_reports_the_version_a_healthy_binary_prints() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin = fake_binary(tmp.path(), "agent", "echo '4.5.6 (Fake Agent)'");
        assert_eq!(
            probe_agent_version(CodingAgent::Codex, &bin, UNREACHABLE_DEADLINE)
                .await
                .as_deref(),
            Some("4.5.6")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn probe_degrades_to_none_on_a_failing_or_silent_binary() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let exits_nonzero = fake_binary(tmp.path(), "broken", "echo boom >&2; exit 1");
        let says_nothing_useful = fake_binary(tmp.path(), "quiet", "echo 'usage: quiet [opts]'");
        let missing = tmp.path().join("does-not-exist");
        let timeout = UNREACHABLE_DEADLINE;
        for bin in [&exits_nonzero, &says_nothing_useful, &missing] {
            assert_eq!(
                probe_agent_version(CodingAgent::ClaudeCode, bin, timeout).await,
                None,
                "a misbehaving binary must yield no version, never an error or a placeholder"
            );
        }
    }

    #[tokio::test]
    async fn read_capped_keeps_only_the_cap_and_drains_the_rest() {
        let payload = vec![b'x'; 64 * 1024];
        let kept = read_capped(Some(payload.as_slice()), 16, "test").await;
        assert_eq!(
            kept.len(),
            16,
            "output past the cap must be dropped, not buffered"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn probe_keeps_the_version_of_a_binary_that_floods_afterwards() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin = fake_binary(
            tmp.path(),
            "chatty",
            "echo '5.6.7 (Chatty)'\nhead -c 262144 /dev/zero | tr '\\0' 'x'",
        );
        assert_eq!(
            probe_agent_version(CodingAgent::Codex, &bin, UNREACHABLE_DEADLINE)
                .await
                .as_deref(),
            Some("5.6.7"),
            "far more output than the cap must not lose the answer or stall the probe"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn probe_gives_up_at_the_timeout() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin = fake_binary(tmp.path(), "wedged", "sleep 30");
        let started = tokio::time::Instant::now();
        let version = probe_agent_version(
            CodingAgent::Codex,
            &bin,
            std::time::Duration::from_millis(200),
        )
        .await;
        assert_eq!(version, None);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the probe must return at its deadline, not wait out the child"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn enrichment_adds_the_version_and_leaves_the_resolution_alone() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin = fake_binary(tmp.path(), "claude", "echo '7.8.9 (Claude Code)'");
        let configured = bin.to_string_lossy().into_owned();

        let sync = detect_agent_binary(CodingAgent::ClaudeCode, Some(&configured));
        let enriched = detect_agent_binary_within(
            CodingAgent::ClaudeCode,
            Some(&configured),
            std::time::Duration::from_secs(120),
        )
        .await;

        assert_eq!(enriched.path, sync.path);
        assert_eq!(enriched.source, sync.source);
        assert_eq!(enriched.valid, sync.valid);
        assert_eq!(enriched.error, sync.error);
        assert_eq!(sync.version, None, "the sync resolution executes nothing");
        assert_eq!(enriched.version.as_deref(), Some("7.8.9"));
    }

    #[tokio::test]
    async fn enrichment_keeps_an_invalid_overrides_error_and_adds_no_version() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let typo = tmp.path().join("claude").to_string_lossy().into_owned();
        let status = detect_agent_binary_with_version(CodingAgent::ClaudeCode, Some(&typo)).await;
        assert_eq!(status.source, "override");
        assert!(!status.valid);
        assert!(
            status
                .error
                .as_deref()
                .is_some_and(|e| e.contains(crate::core::prefs::CODING_AGENT_CLAUDE_PATH.key())),
            "the spawn-failure message naming the preference must survive"
        );
        assert_eq!(status.version, None);
    }

    #[test]
    fn a_model_the_backend_offers_is_accepted_unchanged() {
        assert_eq!(
            validate_coding_agent_model(CodingAgent::ClaudeCode, Some("claude-sonnet-5")),
            Ok(Some("claude-sonnet-5".to_string()))
        );
        assert_eq!(
            validate_coding_agent_model(CodingAgent::Codex, Some("gpt-5.6-luna")),
            Ok(Some("gpt-5.6-luna".to_string()))
        );
    }

    #[test]
    fn an_absent_or_blank_model_inherits_the_backend_default() {
        for input in [None, Some(""), Some("   ")] {
            assert_eq!(
                validate_coding_agent_model(CodingAgent::ClaudeCode, input),
                Ok(None),
                "{input:?} means 'unpinned', which is not an error"
            );
        }
    }

    #[test]
    fn an_unknown_model_is_refused_and_never_swapped_for_the_default() {
        let err = validate_coding_agent_model(CodingAgent::ClaudeCode, Some("claude-sonnet-9"))
            .expect_err("an id the backend does not offer must fail the spawn");
        assert!(
            err.contains("claude-sonnet-9"),
            "name the rejected id: {err}"
        );
        assert!(
            err.contains("claude-opus-5-5[1m]"),
            "list what IS offered, so the caller can fix it in one step: {err}"
        );
    }

    #[test]
    fn a_near_miss_id_names_the_likely_intended_one_before_the_list() {
        let err = validate_coding_agent_model(
            CodingAgent::ClaudeCode,
            Some("claude-opus-5-5@default[1m]"),
        )
        .expect_err("an id no backend offers must fail the spawn");
        assert!(
            err.contains("which spells that model 'claude-opus-5-5[1m]'"),
            "name the near miss before the full list: {err}"
        );
        assert!(
            err.contains("Choose one of:"),
            "the full list must still follow the hint: {err}"
        );
    }

    #[test]
    fn an_id_with_no_near_match_gets_the_plain_list() {
        let err = validate_coding_agent_model(CodingAgent::ClaudeCode, Some("claude-sonnet-9"))
            .expect_err("an id the backend does not offer must fail the spawn");
        assert!(
            !err.contains("which spells that model"),
            "no candidate is close, so no hint must be guessed: {err}"
        );
    }

    #[test]
    fn a_model_from_the_other_backend_is_refused() {
        assert!(validate_coding_agent_model(CodingAgent::Codex, Some("claude-sonnet-5")).is_err());
        assert!(
            validate_coding_agent_model(CodingAgent::ClaudeCode, Some("gpt-5.6-luna")).is_err()
        );
    }

    fn discovered_picker() -> Vec<claude_code::CcMenuOption> {
        let row = |value: &str, runs: &str| {
            serde_json::json!({
                "value": value,
                "resolvedModel": runs,
                "displayName": value,
                "supportsEffort": true,
                "supportedEffortLevels": ["low", "high"],
            })
        };
        let reply = serde_json::json!({ "models": [
            row("default", "claude-opus-5-5[1m]"),
            row("sonnet", "claude-sonnet-5[1m]"),
            row("claude-fable-5-1", "claude-fable-5-1"),
            row("opus", "claude-opus-5-5[1m]"),
            { "value": "haiku", "resolvedModel": "claude-haiku-4-5", "displayName": "Haiku 4.5" },
            row("claude-opus-5-5[1m]", "claude-opus-5-5[1m]"),
        ]});
        cc_model_discovery::menu_options(
            &cc_model_discovery::parse_initialize_models(&reply).expect("recorded reply parses"),
        )
    }

    #[test]
    fn the_model_a_picker_row_runs_is_accepted() {
        let picker = discovered_picker();
        assert_eq!(
            validate_model_against(
                CodingAgent::ClaudeCode,
                Some("claude-sonnet-5[1m]"),
                &picker
            ),
            Ok(Some("claude-sonnet-5[1m]".to_string()))
        );
    }

    #[test]
    fn every_discovered_picker_row_is_accepted_both_ways() {
        let picker = discovered_picker();
        let (served, _) = claude_code::model_and_effort_options(
            &picker,
            claude_code::cc_reasoning_effort_options(),
        );
        assert_eq!(served.len(), picker.len());
        for (wire, row) in served.iter().zip(&picker) {
            let value = wire["value"].as_str().expect("a served row has a value");
            let runs = row
                .resolved_model
                .as_deref()
                .expect("a discovered row runs a model");
            for id in [value, runs] {
                assert_eq!(
                    validate_model_against(CodingAgent::ClaudeCode, Some(id), &picker),
                    Ok(Some(id.to_string())),
                    "the picker offers {value}, which runs {runs}"
                );
            }
        }
    }

    #[test]
    fn a_row_the_picker_folds_is_still_accepted() {
        let picker = discovered_picker();
        let shown = claude_code::cc_picker_rows(&picker);
        assert!(!shown.iter().any(|r| r.value == "claude-opus-5-5[1m]"));
        for id in ["opus", "claude-opus-5-5[1m]"] {
            assert_eq!(
                validate_model_against(CodingAgent::ClaudeCode, Some(id), &picker),
                Ok(Some(id.to_string()))
            );
        }
    }

    #[test]
    fn every_model_the_picker_endpoint_serves_is_accepted() {
        let definitions = claude_code::cc_command_definitions();
        let set_model = definitions
            .as_array()
            .and_then(|all| all.iter().find(|c| c["subtype"] == "set_model"))
            .expect("the picker serves set_model");
        let served = set_model["params"][0]["options"]
            .as_array()
            .expect("set_model has options");
        assert!(!served.is_empty());
        for row in served {
            let value = row["value"].as_str().expect("a served row has a value");
            assert_eq!(
                validate_coding_agent_model(CodingAgent::ClaudeCode, Some(value)),
                Ok(Some(value.to_string()))
            );
        }
    }

    #[test]
    fn a_refusal_names_the_model_each_row_runs() {
        let err = validate_model_against(
            CodingAgent::ClaudeCode,
            Some("claude-sonnet-9"),
            &discovered_picker(),
        )
        .expect_err("no row offers or runs it");
        assert!(
            err.ends_with(
                "Choose one of: default (runs claude-opus-5-5[1m]), \
                 sonnet (runs claude-sonnet-5[1m]), claude-fable-5-1, \
                 opus (runs claude-opus-5-5[1m]), haiku (runs claude-haiku-4-5), \
                 claude-opus-5-5[1m]"
            ),
            "got {err}"
        );
    }

    #[test]
    fn the_model_a_row_runs_finds_that_row() {
        let picker = discovered_picker();
        let haiku = offered_row(&picker, "claude-haiku-4-5").expect("haiku runs it");
        assert_eq!(haiku.value, "haiku");
        assert_eq!(haiku.reasoning_efforts.as_deref(), Some(&[][..]));
        let sonnet = offered_row(&picker, "claude-sonnet-5[1m]").expect("sonnet runs it");
        assert_eq!(sonnet.value, "sonnet");
    }

    #[test]
    fn effort_validation_mirrors_the_model_rules() {
        assert_eq!(
            validate_coding_agent_effort(CodingAgent::ClaudeCode, None, Some("low")),
            Ok(Some("low".to_string()))
        );
        assert_eq!(
            validate_coding_agent_effort(CodingAgent::Codex, None, None),
            Ok(None)
        );
        let err = validate_coding_agent_effort(CodingAgent::ClaudeCode, None, Some("xxhigh"))
            .expect_err("an unknown tier must fail rather than silently drop");
        assert!(err.contains("xxhigh"), "{err}");
    }

    #[test]
    fn a_chat_only_effort_tier_is_not_accepted_for_a_coding_agent() {
        for agent in [CodingAgent::ClaudeCode, CodingAgent::Codex] {
            assert!(
                validate_coding_agent_effort(agent, None, Some("none")).is_err(),
                "{} does not offer 'none'",
                agent.as_str()
            );
        }
    }

    #[test]
    fn a_model_restricted_tier_is_refused_for_a_model_that_lacks_it() {
        let err = validate_coding_agent_effort(CodingAgent::Codex, Some("gpt-5.5"), Some("max"))
            .expect_err("gpt-5.5 does not offer max");
        assert!(
            err.contains("gpt-5.6-sol"),
            "names a model that fits: {err}"
        );
        assert_eq!(
            validate_coding_agent_effort(CodingAgent::Codex, Some("gpt-5.6-sol"), Some("max")),
            Ok(Some("max".to_string()))
        );
    }

    #[test]
    fn a_model_restricted_tier_needs_the_model_named() {
        for model in [None, Some(""), Some("default")] {
            let err = validate_coding_agent_effort(CodingAgent::Codex, model, Some("max"))
                .expect_err("codex max always drops without a named model");
            assert!(err.contains("model"), "must say what to pass: {model:?}");
        }
        assert_eq!(
            validate_coding_agent_effort(CodingAgent::Codex, None, Some("xhigh")),
            Ok(Some("xhigh".to_string()))
        );
        assert_eq!(
            validate_coding_agent_effort(CodingAgent::ClaudeCode, None, Some("max")),
            Ok(Some("max".to_string()))
        );
    }

    fn incident_picker() -> Vec<claude_code::CcMenuOption> {
        let row = |value: &str, runs: &str| serde_json::json!({ "value": value, "resolvedModel": runs, "displayName": value });
        let reply = serde_json::json!({ "models": [
            row("default", "claude-opus-5-5[1m]"),
            row("opus", "claude-opus-5-5[1m]"),
            row("claude-sonnet-5-5", "claude-sonnet-5-5"),
            row("claude-sonnet-5", "claude-sonnet-5"),
            row("claude-sonnet-4-6", "claude-sonnet-4-6"),
            row("claude-sonnet-4-6[1m]", "claude-sonnet-4-6[1m]"),
            row("haiku", "claude-haiku-4-5"),
        ]});
        cc_model_discovery::menu_options(
            &cc_model_discovery::parse_initialize_models(&reply).expect("recorded reply parses"),
        )
    }

    fn picker_listing_withdrawn_models() -> Vec<claude_code::CcMenuOption> {
        let mut rows = incident_picker();
        let template = rows[0].clone();
        for value in ["claude-fable-5-1", "claude-opus-5[1m]"] {
            rows.push(claude_code::CcMenuOption {
                value: value.to_string(),
                resolved_model: Some(value.to_string()),
                label: value.to_string(),
                ..template.clone()
            });
        }
        rows
    }

    fn both_list_states() -> [std::sync::Arc<[claude_code::CcMenuOption]>; 3] {
        [
            claude_code::offered_cc_model_options(None),
            claude_code::offered_cc_model_options(Some(&incident_picker())),
            claude_code::offered_cc_model_options(Some(&picker_listing_withdrawn_models())),
        ]
    }

    #[test]
    fn a_model_claude_code_accepts_but_did_not_list_is_still_offered() {
        for options in both_list_states() {
            for id in [
                "claude-opus-5-5",
                "claude-opus-5-5[1m]",
                "claude-sonnet-5-5",
                "claude-fable-5-2",
            ] {
                assert_eq!(
                    validate_model_against(CodingAgent::ClaudeCode, Some(id), &options),
                    Ok(Some(id.to_string())),
                    "{id} must pass whatever Claude Code listed"
                );
            }
        }
    }

    #[test]
    fn an_overlay_only_model_is_never_listed_or_accepted() {
        let banned = [
            "claude-opus-5",
            "claude-opus-5@default",
            "claude-opus-5[1m]",
            "claude-fable-5",
            "claude-fable-5[1m]",
            "claude-fable-5-1",
            "claude-fable-5-1[1m]",
        ];
        for options in both_list_states() {
            for id in banned {
                assert!(
                    offered_row(&options, id).is_none(),
                    "{id} is overlay only and must not be offered"
                );
                assert!(
                    validate_model_against(CodingAgent::ClaudeCode, Some(id), &options).is_err(),
                    "{id} must be refused"
                );
            }
            let listed: Vec<&str> = options.iter().map(|o| o.value.as_str()).collect();
            for id in banned {
                assert!(!listed.contains(&id), "{id} is in the list: {listed:?}");
            }
        }
    }

    #[test]
    fn a_discovered_row_wins_over_a_curated_row_with_its_value() {
        let options = claude_code::offered_cc_model_options(Some(&incident_picker()));
        let rows: Vec<_> = options
            .iter()
            .filter(|o| o.value == "claude-sonnet-5-5")
            .collect();
        assert_eq!(rows.len(), 1, "the value is listed once");
        assert_eq!(rows[0].label, "claude-sonnet-5-5");
        assert_eq!(rows[0].resolved_model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(options[0].value, "default", "discovered rows lead");
    }

    #[test]
    fn a_curated_row_names_no_model_it_runs() {
        for agent in [CodingAgent::ClaudeCode, CodingAgent::Codex] {
            for option in curated_coding_agent_model_options(agent) {
                assert_eq!(
                    option.resolved_model, None,
                    "{} hardcodes what it runs",
                    option.value
                );
            }
        }
    }

    #[test]
    fn an_overlay_only_row_still_declares_its_window() {
        for id in ["claude-fable-5-1", "claude-fable-5", "claude-opus-5"] {
            assert_eq!(
                coding_agent_context_window(CodingAgent::ClaudeCode, id),
                Some(1_000_000),
                "{id}"
            );
        }
    }

    #[test]
    fn a_declared_window_answers_for_the_backend() {
        assert_eq!(
            coding_agent_context_window(CodingAgent::ClaudeCode, "claude-sonnet-5"),
            Some(1_000_000)
        );
        assert_eq!(
            coding_agent_context_window(CodingAgent::ClaudeCode, "claude-fable-5"),
            Some(1_000_000)
        );
        assert_eq!(
            coding_agent_context_window(CodingAgent::ClaudeCode, "claude-fable-5-1"),
            Some(1_000_000)
        );
        assert_eq!(
            coding_agent_context_window(CodingAgent::ClaudeCode, "claude-opus-5-5"),
            Some(1_000_000)
        );
        assert_eq!(
            coding_agent_context_window(CodingAgent::ClaudeCode, "claude-sonnet-5-5"),
            Some(1_000_000)
        );
    }

    #[test]
    fn the_echoed_id_matches_a_row_spelled_with_a_version_pin() {
        for echoed in ["claude-opus-5", "claude-opus-4-8"] {
            assert_eq!(
                coding_agent_context_window(CodingAgent::ClaudeCode, echoed),
                Some(1_000_000),
                "{echoed} must match its '@default' row"
            );
        }
    }

    #[test]
    fn a_1m_row_does_not_inherit_from_its_bare_sibling() {
        assert_eq!(
            coding_agent_context_window(CodingAgent::ClaudeCode, "claude-opus-4-8[1m]"),
            None
        );
        assert_eq!(
            coding_agent_context_window(CodingAgent::ClaudeCode, "claude-fable-5[1m]"),
            None
        );
        assert_eq!(
            coding_agent_context_window(CodingAgent::ClaudeCode, "claude-fable-5-1[1m]"),
            None
        );
        assert_eq!(
            coding_agent_context_window(CodingAgent::ClaudeCode, "claude-opus-5-5[1m]"),
            None
        );
        assert_eq!(
            coding_agent_context_window(CodingAgent::ClaudeCode, "claude-sonnet-5-5[1m]"),
            None
        );
    }

    #[test]
    fn a_row_with_its_own_tiers_refuses_the_rest() {
        let tiers = ["low".to_string(), "high".to_string()];
        assert_eq!(
            effort_within_row_tiers(CodingAgent::ClaudeCode, "opus", "high", &tiers),
            Ok(Some("high".to_string()))
        );
        let err = effort_within_row_tiers(CodingAgent::ClaudeCode, "opus", "max", &tiers)
            .expect_err("max is not offered");
        assert!(err.contains("It offers: low, high"), "got {err}");
        let err = effort_within_row_tiers(CodingAgent::ClaudeCode, "haiku", "high", &[])
            .expect_err("no tiers");
        assert!(err.contains("It offers: none"), "got {err}");
    }

    #[test]
    fn an_undeclared_model_leaves_the_registry_to_answer() {
        for model in ["haiku", "claude-opus-4-1", "default", "not-a-model"] {
            assert_eq!(
                coding_agent_context_window(CodingAgent::ClaudeCode, model),
                None,
                "{model} must not declare a window"
            );
        }
        assert_eq!(
            coding_agent_context_window(CodingAgent::Codex, "gpt-5.6-sol"),
            None
        );
    }

    #[test]
    fn no_alias_row_declares_a_window() {
        for agent in [CodingAgent::ClaudeCode, CodingAgent::Codex] {
            for alias in ["default", "sonnet", "opus", "opus[1m]", "haiku"] {
                assert_eq!(
                    coding_agent_context_window(agent, alias),
                    None,
                    "{alias} moves between models and must stay undeclared"
                );
            }
        }
    }

    #[test]
    fn no_two_rows_collapse_to_the_same_lookup_key() {
        for agent in [CodingAgent::ClaudeCode, CodingAgent::Codex] {
            let mut seen = std::collections::HashSet::new();
            for option in curated_coding_agent_model_options(agent) {
                let key = strip_version_pin(&option.value);
                assert!(
                    seen.insert(key.clone()),
                    "{} strips to '{key}', which another row already claims",
                    option.value
                );
            }
        }
    }

    #[test]
    fn a_declared_window_is_a_plausible_token_count() {
        for agent in [CodingAgent::ClaudeCode, CodingAgent::Codex] {
            for option in curated_coding_agent_model_options(agent) {
                if let Some(window) = option.context_window {
                    assert!(
                        window >= 200_000,
                        "{} declares an implausible {window}-token window",
                        option.value
                    );
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
mod agent_sandbox_boundary_tests;

#[cfg(test)]
mod plan_ladder_tests;
