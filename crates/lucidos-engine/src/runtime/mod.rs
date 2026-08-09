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
    let mut status = detect_agent_binary(agent, override_path);
    if let Some(binary) = version_probe_target(&status).map(std::path::PathBuf::from) {
        status.version = probe_agent_version(agent, &binary, VERSION_PROBE_TIMEOUT).await;
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

    #[cfg(unix)]
    #[tokio::test]
    async fn probe_reports_the_version_a_healthy_binary_prints() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin = fake_binary(tmp.path(), "agent", "echo '4.5.6 (Fake Agent)'");
        assert_eq!(
            probe_agent_version(CodingAgent::Codex, &bin, std::time::Duration::from_secs(5))
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
        let timeout = std::time::Duration::from_secs(5);
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
            probe_agent_version(CodingAgent::Codex, &bin, std::time::Duration::from_secs(10))
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
        let enriched =
            detect_agent_binary_with_version(CodingAgent::ClaudeCode, Some(&configured)).await;

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
}
