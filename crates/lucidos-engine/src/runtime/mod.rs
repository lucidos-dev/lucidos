pub mod agent_runtime;
pub mod browser;
mod browser_consent;
pub mod claude_code;
pub mod codex;
mod codex_app_server;
mod codex_app_server_parse;
mod codex_parse;
pub mod lucidos_cli;
pub mod python;
pub(crate) mod spawn_env;

pub use agent_runtime::{
    AgentEvent, AgentInput, AgentPermissionRequest, AgentRuntime, CodingAgent, ControlRequest,
    RunningAgent, SpawnArgs,
};
pub use browser::{BrowserLogins, BrowserRuntime, HeadlessBlocklist};
// The three CC wire names are re-exported beside Codex's so every consumer
// reads them off `runtime::`, whichever backend owns the name. They are only
// ever compared against each other (see `is_user_question_tool` below), and a
// call site that had to reach one through `claude_code::` and its neighbour
// through `runtime::` reads as if the two were different KINDS of thing.
pub use claude_code::{
    ClaudeCodeRuntime, CC_MCP_ASK_USER_QUESTION_TOOL, CC_NATIVE_ASK_USER_QUESTION_TOOL,
    CC_PERMISSION_PROMPT_TOOL,
};
pub use codex::{CodexRuntime, CODEX_ASK_USER_QUESTION_TOOL};
pub use python::PythonRuntime;

/// The `/model` picker entries for one backend. Same data the frontend picker
/// renders, reached without the caller having to know which module owns it.
pub fn coding_agent_model_options(agent: CodingAgent) -> &'static [claude_code::CcMenuOption] {
    match agent {
        CodingAgent::ClaudeCode => claude_code::cc_model_options(),
        CodingAgent::Codex => codex::codex_model_options(),
    }
}

/// The context window a coding-agent session runs `model` under, when the
/// backend's own window differs from what the model registry infers.
///
/// `None` means nothing is declared, and the caller falls back to
/// `context_window_for`. That fallback is right for the engine's own calls and
/// wrong for these: it answers 200k for a bare `claude-` id, because LUCIDOS
/// gates 1M mode on its own `[1m]` suffix. Claude Code does not. A Sonnet 5
/// session runs 1M whatever we spell, so a capture rendered a real 240k prompt
/// as "203k / 200k (100%)" before this existed.
///
/// The lookup ignores a `@default` version pin, which the picker writes on two
/// rows (`claude-opus-5@default`) and the agent strips when it echoes the model
/// back. It deliberately does NOT ignore `[1m]`: that suffix is what keeps a 1M
/// row distinct from its bare sibling, the same rule [`likely_intended_model`]
/// documents.
pub fn coding_agent_context_window(agent: CodingAgent, model: &str) -> Option<usize> {
    let target = strip_version_pin(model);
    coding_agent_model_options(agent)
        .iter()
        .find(|o| strip_version_pin(&o.value) == target)
        .and_then(|o| o.context_window)
}

/// The `/effort` picker entries for one backend.
pub fn coding_agent_reasoning_effort_options(
    agent: CodingAgent,
) -> &'static [claude_code::CcMenuOption] {
    match agent {
        CodingAgent::ClaudeCode => claude_code::cc_reasoning_effort_options(),
        CodingAgent::Codex => codex::codex_reasoning_effort_options(),
    }
}

/// Validate a caller-supplied model id against the backend that will run it.
///
/// REFUSES an unknown id rather than falling back to the default, and that is
/// the whole point of the function. A silent fallback is what shipped before:
/// `run_coding_agent` advertised a `model` argument, no layer read it, and every
/// session ran on the settings default while the tool result said success. The
/// caller then reported a model choice it had not made. An id the backend does
/// not offer is a caller mistake, and a mistake the caller can SEE is worth more
/// than a session that quietly runs on the wrong model.
///
/// The vocabulary is the backend's own picker list, so this cannot drift from
/// what the user can pick in the UI.
///
/// When the rejected id is not offered but is a close spelling of one that
/// is, `likely_intended_model` names it before the full list. A caller can
/// paste the LUCIDOS CHAT id `claude-opus-5@default[1m]` by mistake. Claude
/// Code spells that same model `claude-opus-5[1m]`.
pub fn validate_coding_agent_model(
    agent: CodingAgent,
    model: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(model) = model.map(str::trim).filter(|m| !m.is_empty()) else {
        return Ok(None);
    };
    let options = coding_agent_model_options(agent);
    if options.iter().any(|o| o.value == model) {
        return Ok(Some(model.to_string()));
    }
    let offered = options
        .iter()
        .map(|o| o.value.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let hint = likely_intended_model(model, options)
        .map(|likely| format!(", which spells that model '{likely}'"))
        .unwrap_or_default();
    Err(format!(
        "model '{model}' is not offered by {}{hint}. Choose one of: {offered}",
        agent.as_str(),
    ))
}

/// The single offered id that is a close misspelling of a rejected `model`,
/// or `None` when no exactly-one candidate is close enough to name.
///
/// Checked two ways, one at a time. First with a `@default` version pin
/// removed, the LUCIDOS CHAT picker's own decoration. Then with a trailing
/// `[1m]` context-window suffix removed.
///
/// Never both at once. Claude Code's own list pins `@default` on one row,
/// `claude-opus-5@default`. It pins `[1m]` on a different row for the same
/// model, `claude-opus-5[1m]`. Stripping both would collapse those two ids
/// into one stem and turn a clean match into a guess.
fn likely_intended_model<'a>(
    model: &str,
    options: &'a [claude_code::CcMenuOption],
) -> Option<&'a str> {
    unique_match_after(model, options, strip_version_pin).or_else(|| {
        unique_match_after(model, options, |id| {
            id.strip_suffix("[1m]").unwrap_or(id).to_string()
        })
    })
}

/// A model id without the `@default` version pin the Claude Code picker writes
/// on some rows. Shared by the two callers that must see through it, so they
/// cannot disagree about what the decoration means.
fn strip_version_pin(id: &str) -> String {
    id.replace("@default", "")
}

/// The one offered id whose value equals `strip(model)` once `strip` is
/// applied to it too. `None` when zero or more than one candidate ties.
fn unique_match_after<'a>(
    model: &str,
    options: &'a [claude_code::CcMenuOption],
    strip: impl Fn(&str) -> String,
) -> Option<&'a str> {
    let target = strip(model);
    let mut matches = options.iter().filter(|o| strip(&o.value) == target);
    let first = matches.next()?;
    matches.next().is_none().then_some(first.value.as_str())
}

/// Validate a caller-supplied reasoning effort against the backend that will
/// run it. Refuses an out-of-vocabulary tier for the same reason
/// [`validate_coding_agent_model`] refuses an unknown model.
///
/// Codex adds a second constraint its own driver already enforces
/// (`validate_codex_effort`): some tiers are model-specific. That check stays
/// where it is, because it needs the RESOLVED model; this one only rejects a
/// tier the backend does not know at all.
pub fn validate_coding_agent_effort(
    agent: CodingAgent,
    effort: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(effort) = effort.map(str::trim).filter(|e| !e.is_empty()) else {
        return Ok(None);
    };
    let options = coding_agent_reasoning_effort_options(agent);
    if options.iter().any(|o| o.value == effort) {
        return Ok(Some(effort.to_string()));
    }
    let offered = options
        .iter()
        .map(|o| o.value.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "reasoning_effort '{}' is not offered by {}. Choose one of: {}",
        effort,
        agent.as_str(),
        offered
    ))
}

/// True for every wire tool name that raises a Lucidos QuestionCard.
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
    name == CC_NATIVE_ASK_USER_QUESTION_TOOL
        || name == CC_MCP_ASK_USER_QUESTION_TOOL
        || name == CODEX_ASK_USER_QUESTION_TOOL
}

/// Whether the engine is running inside a packaged build (the macOS `.app` /
/// headless tarball / `install.sh` service all set `LUCIDOS_PACKAGED=1` — see
/// `desktop.rs::spawn_gateway` and `service.sh::service_runtime_env_pairs`).
/// Dev / e2e leave it unset. Used to fail fast on a missing bundled runtime
/// dependency in packaged while keeping dev's tolerant degrade-with-log path.
pub(crate) fn is_packaged() -> bool {
    std::env::var_os("LUCIDOS_PACKAGED").is_some_and(|v| v == "1")
}

/// Effective resolution of a coding agent's CLI binary — the payload of
/// `GET /api/v1/coding-agents/binaries` (Settings → Coding Agents). Live
/// detection, recomputed per request: only an explicit user override is ever
/// persisted (the `coding_agent_*_path` preference), so a Homebrew upgrade or
/// installer move self-heals instead of leaving a stale stored path.
#[derive(Debug, serde::Serialize)]
pub struct AgentBinaryStatus {
    /// Resolved absolute path when one was found (the override, a probe hit,
    /// or a PATH hit); `None` when nothing resolves.
    pub path: Option<String>,
    /// Where the resolution came from: `override` (user preference),
    /// `detected` (probe list), `path` (bare PATH lookup), `not-found`.
    pub source: &'static str,
    /// `false` when the override is set but doesn't point at an executable —
    /// `error` then carries the message a spawn would fail with.
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The binary's self-reported version, parsed down to the bare token
    /// (`2.1.224`, `0.147.0`). Only [`detect_agent_binary_with_version`] fills
    /// it, because a version can only come from EXECUTING the binary; the sync
    /// [`detect_agent_binary`] runs nothing and always leaves it `None`. Also
    /// `None` when the probe failed or printed nothing recognizable, so the
    /// field is present exactly when we actually know a version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Compute the effective CLI binary resolution for `agent`, mirroring exactly
/// what a spawn would do: a set override wins (validated the same way the
/// spawn validates it), else the probe list, else a bare-name PATH walk.
pub fn detect_agent_binary(agent: CodingAgent, override_path: Option<&str>) -> AgentBinaryStatus {
    if let Some(raw) = override_path {
        let (label, pref_key) = match agent {
            CodingAgent::ClaudeCode => (
                "Claude Code (`claude`)",
                crate::core::PREF_CODING_AGENT_CLAUDE_PATH,
            ),
            CodingAgent::Codex => ("Codex (`codex`)", crate::core::PREF_CODING_AGENT_CODEX_PATH),
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
    // Bare name — emulate `Command::spawn`'s PATH walk so the UI shows what
    // would actually run (or that nothing would).
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

/// How long a `--version` probe may run before it is abandoned. Both shipped
/// agents answer in well under 100 ms; the ceiling exists so a wedged binary
/// can't hold the Settings request open.
const VERSION_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// [`detect_agent_binary`] plus the resolved binary's self-reported version.
///
/// Split from the sync resolution because the resolution is a pure filesystem
/// question while a version can only come from EXECUTING something: keeping
/// them apart means every existing caller of `detect_agent_binary` stays free
/// of subprocess spawns, and the enrichment can only ever add `version` (never
/// change `path` / `source` / `valid` / `error`).
///
/// Probed live per request, like the path itself: a Homebrew upgrade or a
/// coding agent's self-update must self-heal rather than leave a stale value
/// on screen. The two agents are probed concurrently by the caller, so the
/// endpoint pays one probe's latency, not two.
pub async fn detect_agent_binary_with_version(
    agent: CodingAgent,
    override_path: Option<&str>,
) -> AgentBinaryStatus {
    detect_agent_binary_within(agent, override_path, VERSION_PROBE_TIMEOUT).await
}

/// The enrichment above, with the probe's ceiling supplied by the caller.
///
/// Only the test names its own ceiling, and it needs to. The production five
/// seconds is a user-facing request budget, not a claim about how long a fork
/// takes. The full suite runs thousands of tests at once, and a spawn there
/// really can miss that budget. This failed as a flake rather than a defect.
/// The sibling probe tests already take a timeout for the same reason.
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

/// The binary a `--version` probe is allowed to execute for this resolution,
/// or `None` when nothing may be run.
///
/// `not-found` has no path at all, and an invalid override carries the path the
/// user typed at something that isn't an executable file. Executing either is
/// impossible or wrong, so eligibility is `valid` AND a path, and it lives in
/// its own function so that stays testable without spawning anything.
fn version_probe_target(status: &AgentBinaryStatus) -> Option<&str> {
    if !status.valid {
        return None;
    }
    status.path.as_deref()
}

/// Most either pipe of a `--version` probe may contribute. The answer is one
/// short line, so anything past this is a binary that is not the CLI we asked.
///
/// The cap is what bounds the probe in MEMORY, and the timeout alone does not:
/// `Command::output` reads each pipe to EOF, so a binary that streams would
/// have the engine buffer everything it could produce in the whole 5 s, twice
/// over (both agents are probed concurrently).
const VERSION_PROBE_OUTPUT_LIMIT: u64 = 8 * 1024;

/// How much of a probe's output a single log line may carry.
const VERSION_PROBE_LOG_EXCERPT: usize = 200;

/// Ask a resolved coding-agent binary for its version.
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
async fn probe_agent_version(
    agent: CodingAgent,
    binary: &std::path::Path,
    timeout: std::time::Duration,
) -> Option<String> {
    let label = agent.as_str();
    let shown = binary.display();
    let mut cmd = tokio::process::Command::new(binary);
    cmd.arg("--version")
        // stdin is the one stdio tokio does NOT set for us, so closing it here
        // is load-bearing: a binary that isn't the CLI we think it is would
        // otherwise sit reading the engine's own stdin until the timeout.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            crate::log!("[AgentBinary] {label}: `{shown} --version` failed to run: {e}");
            return None;
        }
    };
    // Read both pipes while waiting, never one after the other: a child whose
    // other pipe filled blocks on the write and never reaches the exit we are
    // waiting for.
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
            crate::log!("[AgentBinary] {label}: `{shown} --version` timed out after {timeout:?}");
            return None;
        }
    };
    let status = match waited {
        Ok(status) => status,
        Err(e) => {
            crate::log!("[AgentBinary] {label}: waiting for `{shown} --version` failed: {e}");
            return None;
        }
    };
    if !status.success() {
        crate::log!(
            "[AgentBinary] {label}: `{shown} --version` exited {status}: {}",
            log_excerpt(&stderr, VERSION_PROBE_LOG_EXCERPT)
        );
        return None;
    }
    let parsed = parse_agent_version(&stdout);
    if parsed.is_none() {
        crate::log!(
            "[AgentBinary] {label}: `{shown} --version` printed no recognizable version: '{}'",
            log_excerpt(&stdout, VERSION_PROBE_LOG_EXCERPT)
        );
    }
    parsed
}

/// KEEP at most `limit` bytes from one of a probe's pipes, as lossy UTF-8, then
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
        crate::log!("[AgentBinary] {context} read failed: {e}");
        return String::from_utf8_lossy(&kept).into_owned();
    }
    let mut sink = [0u8; 4096];
    loop {
        match pipe.read(&mut sink).await {
            Ok(0) => break,
            Ok(_) => continue,
            Err(e) => {
                crate::log!("[AgentBinary] {context} drain failed: {e}");
                break;
            }
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}

/// The leading `max` characters of `text`, trimmed, for a log line that must
/// not carry a misbehaving binary's whole output.
fn log_excerpt(text: &str, max: usize) -> &str {
    let text = text.trim();
    &text[..text.floor_char_boundary(max)]
}

/// Extract the bare version token from a `--version` first line.
///
/// Deliberately loose, because the format belongs to somebody else's CLI:
/// `2.1.224 (Claude Code)` and `codex-cli 0.147.0` both yield their number
/// without either shape being hardcoded. A token qualifies when, stripped of
/// surrounding punctuation and one leading `v`, it starts with a digit and
/// contains a `.`; the dot is what stops a stray count or year from reading as
/// a version. Only the FIRST line is considered, so a later line that happens
/// to carry a dotted number (a config path, a `1.5s` timing) can't be mistaken
/// for the answer.
///
/// Returns `None` rather than guessing when nothing qualifies. The caller logs
/// the raw line, so an unrecognized format stays recoverable without a
/// half-understood string reaching the UI.
fn parse_agent_version(output: &str) -> Option<String> {
    output
        .lines()
        .next()?
        .split_whitespace()
        .find_map(version_token)
        .map(str::to_string)
}

/// The version token inside one whitespace-separated word, or `None`.
fn version_token(word: &str) -> Option<&str> {
    let trimmed = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let core = trimmed.strip_prefix('v').unwrap_or(trimmed);
    (core.starts_with(|c: char| c.is_ascii_digit()) && core.contains('.')).then_some(core)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Version parsing (parse_agent_version) ──────────────────────────────
    // The two shipped agents print different shapes and neither is a contract
    // we control, so the parser is checked against their real output.

    #[test]
    fn parses_the_shapes_both_shipped_agents_print() {
        // Measured 2026-08-08: `claude --version` and `codex --version`.
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
        // Truncating to the numeric core would report a prerelease as if it
        // were the release.
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
        // A later line's dotted number (a config path, a timing) must not be
        // mistaken for the version.
        assert_eq!(
            parse_agent_version("3.2.1 (Some CLI)\nloaded /home/u/.config/some.json\n").as_deref(),
            Some("3.2.1")
        );
    }

    #[test]
    fn returns_none_rather_than_guessing() {
        // No token qualifies -> the caller logs the raw line and shows nothing,
        // instead of putting a half-understood string next to the path.
        assert_eq!(parse_agent_version("no version here"), None);
        assert_eq!(parse_agent_version("build 20260808"), None);
        assert_eq!(parse_agent_version(""), None);
        assert_eq!(parse_agent_version("\n"), None);
    }

    // ── Probe eligibility (version_probe_target) ───────────────────────────
    // Nothing may be executed unless the resolution produced a path AND is
    // valid: `not-found` has nothing to run, and an invalid override is the
    // path the user typo'd.

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

    // ── The probe itself (probe_agent_version) ─────────────────────────────
    // Every failure branch must degrade to `None` without failing the request
    // and without leaving a child behind.

    fn fake_binary(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod fake binary");
        }
        path
    }

    /// Headroom for the tests below, where the deadline is scaffolding rather
    /// than the thing under test. Each spawns a shell script that returns in
    /// milliseconds. Any deadline they reach means the host is loaded, not
    /// that the probe is wrong. The production `VERSION_PROBE_TIMEOUT` of 5s
    /// was reachable: the full suite saturated one and `--version` timed out.
    ///
    /// `probe_gives_up_at_the_timeout` owns deadline behaviour and keeps its
    /// own short value, so widening here costs no coverage.
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
        // Generous on purpose: a deadline hit here would make the test pass for
        // the wrong reason, since a timeout also yields `None`.
        let timeout = UNREACHABLE_DEADLINE;
        for bin in [&exits_nonzero, &says_nothing_useful, &missing] {
            assert_eq!(
                probe_agent_version(CodingAgent::ClaudeCode, bin, timeout).await,
                None,
                "a misbehaving binary must yield no version, never an error or a placeholder"
            );
        }
    }

    /// The probe is bounded in MEMORY, not only in time: `Command::output`
    /// reads each pipe to EOF, so a binary that streams would have the engine
    /// buffer everything it could produce before the deadline.
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

    /// Draining past the cap is what stops the bound from costing correctness:
    /// a child whose pipe fills blocks on the write and never exits, so a probe
    /// that merely stopped reading would time out on a CLI that prints a banner
    /// after its version.
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

    /// A wedged binary must not hold the Settings request open: the probe
    /// returns `None` at the deadline, and `kill_on_drop` reaps the child with
    /// the abandoned future rather than leaving it running.
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

    // ── The enrichment (detect_agent_binary_with_version) ──────────────────

    #[cfg(unix)]
    #[tokio::test]
    async fn enrichment_adds_the_version_and_leaves_the_resolution_alone() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin = fake_binary(tmp.path(), "claude", "echo '7.8.9 (Claude Code)'");
        let configured = bin.to_string_lossy().into_owned();

        let sync = detect_agent_binary(CodingAgent::ClaudeCode, Some(&configured));
        // A ceiling of its own, so a loaded host forking a shell cannot fail
        // this. The production budget is asserted by the probe tests above.
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
                .is_some_and(|e| e.contains("coding_agent_claude_path")),
            "the spawn-failure message naming the preference must survive"
        );
        assert_eq!(status.version, None);
    }

    // ── Spawn pin validation ───────────────────────────────────────────────
    // Both validators REFUSE rather than fall back. That is the whole contract:
    // `run_coding_agent` advertised a `model` argument that no layer read, so
    // every session silently ran on the `cc-settings.json` default while the
    // spawn reported success. Falling back to the default on a bad id would
    // rebuild exactly that failure, one layer higher up.

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
            err.contains("claude-opus-5@default"),
            "list what IS offered, so the caller can fix it in one step: {err}"
        );
    }

    /// The real mistake this guards against: a caller pastes the LUCIDOS CHAT
    /// id for Opus 5's 1M variant, which carries CHAT's own `@default`
    /// version pin. Claude Code spells the same model without that pin.
    #[test]
    fn a_near_miss_id_names_the_likely_intended_one_before_the_list() {
        let err =
            validate_coding_agent_model(CodingAgent::ClaudeCode, Some("claude-opus-5@default[1m]"))
                .expect_err("an id no backend offers must fail the spawn");
        assert!(
            err.contains("which spells that model 'claude-opus-5[1m]'"),
            "name the near miss before the full list: {err}"
        );
        assert!(
            err.contains("Choose one of:"),
            "the full list must still follow the hint: {err}"
        );
    }

    /// `claude-sonnet-9` collapses to no other offered id under either strip,
    /// so the refusal carries no guess, only the plain list.
    #[test]
    fn an_id_with_no_near_match_gets_the_plain_list() {
        let err = validate_coding_agent_model(CodingAgent::ClaudeCode, Some("claude-sonnet-9"))
            .expect_err("an id the backend does not offer must fail the spawn");
        assert!(
            !err.contains("which spells that model"),
            "no candidate is close, so no hint must be guessed: {err}"
        );
    }

    /// The two backends have disjoint model vocabularies, and picking the wrong
    /// one is the likeliest caller mistake: a Codex spawn carrying a Claude id
    /// looks entirely reasonable at the call site.
    #[test]
    fn a_model_from_the_other_backend_is_refused() {
        assert!(validate_coding_agent_model(CodingAgent::Codex, Some("claude-sonnet-5")).is_err());
        assert!(
            validate_coding_agent_model(CodingAgent::ClaudeCode, Some("gpt-5.6-luna")).is_err()
        );
    }

    #[test]
    fn effort_validation_mirrors_the_model_rules() {
        assert_eq!(
            validate_coding_agent_effort(CodingAgent::ClaudeCode, Some("low")),
            Ok(Some("low".to_string()))
        );
        assert_eq!(
            validate_coding_agent_effort(CodingAgent::Codex, None),
            Ok(None)
        );
        let err = validate_coding_agent_effort(CodingAgent::ClaudeCode, Some("xxhigh"))
            .expect_err("an unknown tier must fail rather than silently drop");
        assert!(err.contains("xxhigh"), "{err}");
    }

    /// `none` is on the CHAT effort ladder and on neither coding-agent picker.
    /// A caller reading the chat vocabulary would reach for it, and it must not
    /// resolve to the backend default in silence.
    #[test]
    fn a_chat_only_effort_tier_is_not_accepted_for_a_coding_agent() {
        for agent in [CodingAgent::ClaudeCode, CodingAgent::Codex] {
            assert!(
                validate_coding_agent_effort(agent, Some("none")).is_err(),
                "{} does not offer 'none'",
                agent.as_str()
            );
        }
    }

    // ── The backend's own context window ───────────────────────────────────

    /// The bug this resolver exists for. A Sonnet 5 session runs 1M under
    /// Claude Code, while the registry answers 200k for the same id. So the
    /// viewer rendered a real 240k prompt as "203k / 200k (100%)".
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
    }

    /// The picker pins a version on two rows and the agent echoes the model
    /// without it, so an exact-value lookup would miss both.
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

    /// `[1m]` is NOT stripped when matching. It is what tells a 1M row from its
    /// bare sibling, so collapsing the two would make one inherit the other's
    /// window. Those rows declare nothing: the id-shape rule answers 1M.
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
    }

    /// Undeclared means "the registry answers", which is the behaviour every
    /// one of these had before the field existed.
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

    /// The trap that makes this table subtle. `normalize_cc_model_id` folds an
    /// old dated id onto an alias, so `sonnet` is what a Sonnet 4.6 session
    /// records. A window declared there would describe the wrong model.
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

    /// The lookup takes the FIRST row matching the stripped id, where
    /// `unique_match_after` refuses an ambiguous one. That is safe only while
    /// no two rows collapse to the same key, so pin it: adding a bare
    /// `claude-opus-5` beside `claude-opus-5@default` would otherwise let
    /// whichever comes first answer.
    #[test]
    fn no_two_rows_collapse_to_the_same_lookup_key() {
        for agent in [CodingAgent::ClaudeCode, CodingAgent::Codex] {
            let mut seen = std::collections::HashSet::new();
            for option in coding_agent_model_options(agent) {
                let key = strip_version_pin(&option.value);
                assert!(
                    seen.insert(key.clone()),
                    "{} strips to '{key}', which another row already claims",
                    option.value
                );
            }
        }
    }

    /// A declared window is a real token count. Guards a slipped digit in the
    /// JSON, which would otherwise read as a tiny window and pin every step at
    /// hundreds of percent.
    #[test]
    fn a_declared_window_is_a_plausible_token_count() {
        for agent in [CodingAgent::ClaudeCode, CodingAgent::Codex] {
            for option in coding_agent_model_options(agent) {
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
