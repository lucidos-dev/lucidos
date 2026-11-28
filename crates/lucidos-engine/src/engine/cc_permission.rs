use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;

use crate::core::grants::GrantFile;
use crate::engine::coding_command_grants::{CLAUDE_CODE_COMMAND_TOOL, CODEX_COMMAND_TOOL};
use crate::engine::command_guard::{
    self, unwrap_shell_command, Catastrophe, CommandSite, JudgeInput, RiskLane, StaticVerdict,
    TrustedHeads,
};
use crate::engine::command_judge::JudgeVerdict;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::permission::standing_deny;
use crate::engine::permission::tool_grants::{self, AnsweredBy};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::thread_events::{GrantOffer, OfferWithheld};
use crate::engine::trusted_repo_scripts::trusted_repo_scripts;
use crate::engine::types::SharedEngine;
use crate::engine::LucidosEngine;
use crate::llm::tool_names as tn;

pub const DENIAL_REASON: &str = "User denied";

/// INVARIANT: Reason returned to the coding agent when a prompt resolves because the
/// broadcast channel CLOSED. The engine is tearing down, and the user did not
/// click Deny. Distinct from `DENIAL_REASON`: a restart is not a user decision,
/// so a resumed session must not read it as "the user rejected my approach".
/// The companion half is `RESTART_NOT_REJECTION_RULE` in
/// `agent_session::prompts`.
pub const RESTART_INTERRUPT_REASON: &str =
    "Interrupted by an engine restart — not a user decision. Re-attempt the action; \
     the restart did not reject your approach.";

pub const RESTART_CLOSED_REASON: &str = "Interrupted by an engine restart before anyone answered";

pub const SUPERSEDED_REASON: &str = "Superseded by a new message";

/// INVARIANT: Reason stamped on a `CodingAgentPermissionResolved` that the engine emits
/// when a coding-agent session IDLES with a permission card still unresolved.
/// Cleared at the turn boundary in `emit_coding_agent_idled`, so the stale card
/// cannot sit clickable and a later click cannot resurrect the finished thread.
/// Distinct from `SUPERSEDED_REASON` and from the boot orphan-recovery reason.
pub const SESSION_ENDED_REASON: &str =
    "Coding agent session ended before answering — request expired";

pub const SESSION_ALLOW_REASON: &str = "Allowed for this thread";

pub const PERSISTED_ALLOW_REASON: &str = "Allowed by this workspace's coding-agent allowlist";

pub const WORKTREE_WRITE_ALLOW_REASON: &str =
    "Auto-allowed: file write inside this session's own worktree";

pub const COMMAND_GUARD_ALLOW_REASON: &str = "Auto-allowed: the command guard allowed this command";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionAnswer {
    Allowed,
    Denied,
    Refused(String),
    Withdrawn(&'static str),
}

impl PermissionAnswer {
    pub fn from_decision(allowed: bool) -> Self {
        if allowed {
            Self::Allowed
        } else {
            Self::Denied
        }
    }
}

pub type DedupKey = (Uuid, String, String);

pub struct PermissionEntry {
    pub thread_id: Uuid,
    pub request_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    pub grant_offer: GrantOffer,
    pub tx: tokio::sync::broadcast::Sender<PermissionAnswer>,
}

#[derive(Default)]
pub struct PermissionState {
    pub by_dedup_key: HashMap<DedupKey, PermissionEntry>,
    pub by_request_id: HashMap<String, DedupKey>,
}

impl PermissionState {
    pub fn take(&mut self, request_id: &str) -> Option<PermissionEntry> {
        let key = self.by_request_id.remove(request_id)?;
        self.by_dedup_key.remove(&key)
    }

    pub fn thread_of(&self, request_id: &str) -> Option<Uuid> {
        let key = self.by_request_id.get(request_id)?;
        self.by_dedup_key.get(key).map(|entry| entry.thread_id)
    }

    pub fn register_or_attach(
        &mut self,
        dedup_key: DedupKey,
        thread_id: Uuid,
        tool_name: String,
        input: serde_json::Value,
        grant_offer: GrantOffer,
    ) -> (
        String,
        tokio::sync::broadcast::Receiver<PermissionAnswer>,
        bool,
    ) {
        self.gc_dead_entries();
        if let Some(entry) = self.by_dedup_key.get(&dedup_key) {
            return (entry.request_id.clone(), entry.tx.subscribe(), false);
        }
        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::broadcast::channel(1);
        self.by_dedup_key.insert(
            dedup_key.clone(),
            PermissionEntry {
                thread_id,
                request_id: request_id.clone(),
                tool_name,
                input,
                grant_offer,
                tx,
            },
        );
        self.by_request_id.insert(request_id.clone(), dedup_key);
        (request_id, rx, true)
    }

    pub fn gc_dead_entries(&mut self) {
        let dead: Vec<DedupKey> = self
            .by_dedup_key
            .iter()
            .filter(|(_, entry)| entry.tx.receiver_count() == 0)
            .map(|(k, _)| k.clone())
            .collect();
        for key in dead {
            if let Some(entry) = self.by_dedup_key.remove(&key) {
                self.by_request_id.remove(&entry.request_id);
            }
        }
    }
}

pub struct PermissionPromptOutcome {
    pub allowed: bool,
    pub reason: Option<String>,
}

const SUMMARY_MAX_PATHS: usize = 3;

/// INVARIANT: What the engine could learn about a file-write request's targets.
///
/// Three states rather than a `Vec`, because both decisions below are made over
/// the WHOLE set. A plain list cannot tell "no targets, this is not a file
/// write" from "a target we could not read". Dropping an unreadable entry is
/// the dangerous shape: its siblings would vouch for it, and one in-worktree
/// path would auto-approve a patch whose other half writes somewhere unseen.
enum FileTargets {
    NotAFileWrite,
    /// INVARIANT: Every target the request names, all of them resolved. Never empty, so
    /// the `all()` below can never vacuously approve nothing.
    Known(Vec<String>),
    Unresolved,
}

pub fn build_permission_summary(tool_name: &str, input: &serde_json::Value) -> String {
    let display_name = match tool_name {
        "Skill" => "skill",
        _ => tool_name,
    };
    if tool_name == "file_change" && input.get("changes").is_some() {
        if let FileTargets::Known(paths) = coding_agent_file_targets(tool_name, input) {
            let shown = paths
                .iter()
                .take(SUMMARY_MAX_PATHS)
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            let rest = paths.len().saturating_sub(SUMMARY_MAX_PATHS);
            return if rest > 0 {
                format!("{} {} +{} more", display_name, shown, rest)
            } else {
                format!("{} {}", display_name, shown)
            };
        }
    }
    let arg = [
        "file_path",
        "path",
        "command",
        "notebook_path",
        "skill",
        "url",
        "pattern",
        "reason",
        "grant_root",
    ]
    .iter()
    .find_map(|k| input.get(k).and_then(|v| v.as_str()))
    .unwrap_or("");
    if arg.is_empty() {
        display_name.to_string()
    } else {
        format!("{} {}", display_name, arg)
    }
}

/// INVARIANT: Recover the originating user actor for a coding-agent thread from its most
/// recent user-message event. Narrowing to those two event types keeps a later
/// engine-stamped event from overwriting the human actor. Returns `None` when
/// no such event carries an actor, and the caller falls back to
/// `EventMeta::NONE`.
pub async fn lookup_thread_actor(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<MessageOrigin> {
    let row: Result<Option<(serde_json::Value,)>, sqlx::Error> = sqlx::query_as(
        "SELECT payload->'actor' FROM events \
         WHERE thread_id = $1 \
           AND event_type IN ('MessageReceived', 'CodingAgentUserMessageSent') \
           AND payload ? 'actor' \
           AND payload->'actor' != 'null'::jsonb \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await;
    match row {
        Ok(Some((v,))) => serde_json::from_value::<MessageOrigin>(v).ok(),
        Ok(None) => None,
        Err(e) => {
            crate::log_legacy!(
                "[CCPermission] lookup_thread_actor failed for thread {}: {}",
                thread_id,
                e
            );
            None
        }
    }
}

/// INVARIANT: What a coding-agent *command* request carries. Codex raises these as
/// `command_execution`; Claude Code as `Bash`.
///
/// The three cases are not two. A command whose payload could not be read must
/// never take the fall-through of a request that is no command at all.
enum CommandPayload<'a> {
    NotACommand,
    Known(&'a str),
    Unresolved,
}

fn coding_agent_command<'a>(tool_name: &str, input: &'a serde_json::Value) -> CommandPayload<'a> {
    if !matches!(tool_name, "command_execution" | "Bash") {
        return CommandPayload::NotACommand;
    }
    match input.get("command").and_then(|v| v.as_str()) {
        Some(cmd) if !cmd.trim().is_empty() => CommandPayload::Known(cmd),
        _ => CommandPayload::Unresolved,
    }
}

/// INVARIANT: Extract the target paths of a coding-agent *file-write* request. Claude Code
/// raises one of several tools, each naming a single path. Codex raises
/// `file_change`, whose `changes` list can name several files at once.
///
/// **One unreadable entry makes the whole set [`FileTargets::Unresolved`]**,
/// rather than being filtered out. A partially-understood patch really does
/// reach here, and excusing it by the entries that DID parse is exactly the
/// fail-open the callers must not make.
fn coding_agent_file_targets(tool_name: &str, input: &serde_json::Value) -> FileTargets {
    if !matches!(
        tool_name,
        "file_change" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit"
    ) {
        return FileTargets::NotAFileWrite;
    }
    // INVARIANT: `changes` is Codex vocabulary, so only a `file_change` may be read from
    // it. Without the tool gate, a stray `changes` key on a Claude Code `Write`
    // would REPLACE its real `file_path`. One in-worktree entry would then skip
    // the card for a write landing anywhere. CC's input is model-authored JSON
    // the permission server forwards verbatim, so that shape is not
    // hypothetical.
    if tool_name == "file_change" {
        if let Some(changes) = input.get("changes") {
            let Some(changes) = changes.as_array() else {
                return FileTargets::Unresolved;
            };
            let mut paths = Vec::with_capacity(changes.len());
            for change in changes {
                match change.get("path").and_then(|p| p.as_str()) {
                    // INVARIANT: Absolute only. A relative path cannot be placed without
                    // the agent's cwd, and `path_outside_workspace` reads an
                    // unplaceable path as in-workspace, which is right for CC.
                    // For a `file_change` the request's own existence inverts
                    // that: codex raises it BECAUSE the patch escaped its
                    // sandbox, so "assume it lands in the worktree" is the one
                    // conclusion the evidence rules out.
                    Some(path) if !path.is_empty() && Path::new(path).is_absolute() => {
                        paths.push(path.to_string())
                    }
                    _ => return FileTargets::Unresolved,
                }
            }
            return if paths.is_empty() {
                FileTargets::Unresolved
            } else {
                FileTargets::Known(paths)
            };
        }
    }
    for key in ["file_path", "path", "notebook_path", "grant_root"] {
        if let Some(s) = input.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return FileTargets::Known(vec![s.to_string()]);
            }
        }
    }
    FileTargets::Unresolved
}

/// INVARIANT: True when the command runs in a request `cwd` outside the workspace and
/// destroys something there. Codex states that directory. The command text
/// alone reads a relative path as in-workspace, so neither the static verdict
/// nor the judge can see this.
fn destroys_from_request_cwd(
    tool_name: &str,
    input: &serde_json::Value,
    workspace_path: &Path,
) -> bool {
    let cwd_outside = input
        .get("cwd")
        .and_then(|v| v.as_str())
        .is_some_and(|cwd| path_outside_workspace(cwd, workspace_path));
    cwd_outside
        && matches!(
            coding_agent_command(tool_name, input),
            CommandPayload::Known(cmd)
                if command_guard::destroys_when_started_outside(&unwrap_shell_command(cmd))
        )
}

fn path_outside_workspace(path: &str, workspace_path: &Path) -> bool {
    let p = Path::new(path);
    // INVARIANT: Checked FIRST, before the relative-is-inside shortcut: a relative
    // `../../etc/...` escapes the worktree just as an absolute one does.
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return true;
    }
    if !p.is_absolute() {
        return false;
    }
    if !p.starts_with(workspace_path) {
        return true;
    }
    // INVARIANT: A lexical prefix check is escapable, which is why the sibling
    // `path_inside_worktree` resolves both sides. A symlink inside the
    // workspace pointing outside makes the path read as contained while the
    // write lands elsewhere. Resolve
    // here too, but only to OVERRIDE a lexical "inside". A dangling link on the
    // chain reads as outside, since the write follows it somewhere unknown.
    // Otherwise, when either side fails to resolve we keep the lexical answer.
    // The ordinary case is a `Write` naming a file that does not exist yet, and
    // calling that outside would card every new file.
    match (
        std::fs::canonicalize(workspace_path),
        canonical_existing_prefix(p),
    ) {
        (_, ExistingPrefix::DanglingLink) => true,
        (Ok(root), ExistingPrefix::Resolved(resolved)) => !resolved.starts_with(&root),
        _ => false,
    }
}

fn has_git_component(p: &Path) -> bool {
    p.components()
        .any(|c| matches!(c, std::path::Component::Normal(name) if name == ".git"))
}

fn has_rejected_component(p: &Path) -> bool {
    has_git_component(p)
        || p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
}

#[derive(Debug, PartialEq, Eq)]
enum ExistingPrefix {
    Resolved(std::path::PathBuf),
    DanglingLink,
    Unresolved,
}

/// INVARIANT: Canonicalize the longest existing prefix of `p`. A `Write` names a file that
/// does not exist yet, so canonicalizing the target itself fails on exactly the
/// case we most need to classify. Walk up to the nearest ancestor that does
/// resolve. Never walk past an entry that exists but fails to resolve: its
/// parent says nothing about where a write through that link lands.
fn canonical_existing_prefix(p: &Path) -> ExistingPrefix {
    let mut current = Some(p);
    while let Some(candidate) = current {
        if let Ok(real) = std::fs::canonicalize(candidate) {
            return ExistingPrefix::Resolved(real);
        }
        if std::fs::symlink_metadata(candidate).is_ok() {
            return ExistingPrefix::DanglingLink;
        }
        current = candidate.parent();
    }
    ExistingPrefix::Unresolved
}

/// INVARIANT: True when `path` provably resolves INSIDE `worktree_root`, the session's own
/// disposable worktree, and is therefore covered by the reviewed-before-Apply
/// guarantee that makes [`worktree_write_auto_allowed`] safe.
///
/// "Provably" is load-bearing: a false positive here skips a security card, so
/// every branch that cannot *prove* containment returns false.
///
///   * **Symlinks are resolved, not trusted lexically.** A symlink inside the
///     worktree pointing outside makes a path look contained while the write
///     lands elsewhere, invisible to the reviewed diff. So both sides are
///     canonicalized, and an unresolvable side leaves containment unproven.
///   * **A `..` component** is checked lexically before resolution.
///   * **A `.git` component** is checked BOTH on the input path and on the
///     resolved one, so a symlink into git metadata is caught too. A hook
///     written there never appears in the diff reviewed before Apply.
///   * **A relative path** cannot be resolved without the agent's cwd, which is
///     the worktree for a repo-rooted spawn but a subdirectory for an app
///     thread. It costs nothing, since both backends pass an absolute path.
fn path_inside_worktree(path: &str, worktree_root: &Path) -> bool {
    let p = Path::new(path);
    if !p.is_absolute() || has_rejected_component(p) {
        return false;
    }
    let Ok(root) = std::fs::canonicalize(worktree_root) else {
        return false;
    };
    let ExistingPrefix::Resolved(resolved) = canonical_existing_prefix(p) else {
        return false;
    };
    let Ok(relative) = resolved.strip_prefix(&root) else {
        return false;
    };
    !has_rejected_component(relative)
}

/// INVARIANT: True when this coding-agent permission request is a file write landing
/// inside the session's own worktree, and can therefore be auto-allowed without
/// rendering a card.
///
/// Why this exists: Claude Code auto-approves in-cwd writes under
/// `acceptEdits` **except** under `.claude/` and `.git/`, which it routes
/// through the permission prompt in every mode. Lucidos keeps its own agent
/// configuration in `.claude/`, so editing a rule cost a card every time, and
/// no persisted scope suppresses that. The engine's policy is the simpler
/// invariant, *an in-worktree file write needs no card*.
///
/// It is safe because the worktree is disposable and every change in it is
/// reviewed in the Diff before Apply. `.git` is carved out of
/// [`path_inside_worktree`] because it is the one in-worktree location that
/// ISN'T in that diff.
///
/// Scope is the file-write vocabulary of [`coding_agent_file_targets`]. A
/// command is NOT covered, since it can do anything. A `None` `worktree_root`
/// fails closed. **Every** target must be contained, and an empty target list
/// is never auto-allowed.
pub fn worktree_write_auto_allowed(
    tool_name: &str,
    input: &serde_json::Value,
    worktree_root: Option<&Path>,
) -> bool {
    let Some(root) = worktree_root else {
        return false;
    };
    match coding_agent_file_targets(tool_name, input) {
        FileTargets::Known(targets) => {
            !targets.is_empty()
                && targets
                    .iter()
                    .all(|target| path_inside_worktree(target, root))
        }
        FileTargets::NotAFileWrite | FileTargets::Unresolved => false,
    }
}

enum CommandStatic {
    NotACommand,
    Unreadable,
    Verdict(StaticVerdict),
}

#[cfg(test)]
fn static_command_verdict(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
) -> CommandStatic {
    static_command_verdict_at(tool_name, input, trusted, None)
}

fn static_command_verdict_at_with_paths(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
    root: Option<&Path>,
    paths: &command_guard::SecretPaths,
) -> CommandStatic {
    match coding_agent_command(tool_name, input) {
        CommandPayload::NotACommand => CommandStatic::NotACommand,
        CommandPayload::Unresolved => CommandStatic::Unreadable,
        CommandPayload::Known(cmd) => {
            let synthetic = serde_json::json!({ "command": cmd });
            CommandStatic::Verdict(command_guard::static_classify_with_paths(
                tn::RUN_BASH,
                &synthetic,
                CommandSite::TextOnly { root },
                trusted,
                paths,
            ))
        }
    }
}

const FILE_READ_TOOLS: [&str; 3] = ["Read", "Grep", "Glob"];

/// INVARIANT: Whether a `Read`, `Grep` or `Glob` request reaches a secret path
/// ([`command_guard::SecretPaths::mentions`]). The path is read alone and
/// joined with its glob, so a search rooted at `~/.lucidos` for
/// `secret-keys/*` counts. A bash command is the static pass's business.
fn file_tool_reads_secret_with_paths(
    tool_name: &str,
    input: &serde_json::Value,
    paths: &command_guard::SecretPaths,
) -> bool {
    if !FILE_READ_TOOLS.contains(&tool_name) {
        return false;
    }
    let field = |key: &str| input.get(key).and_then(|v| v.as_str());
    let root = field("file_path").or_else(|| field("path")).unwrap_or("");
    let globs = [field("pattern"), field("glob")];
    paths.mentions(root)
        || globs
            .into_iter()
            .flatten()
            .any(|glob| paths.mentions(&format!("{root}/{glob}")))
}

fn lane_allows_escalation(lane: Option<RiskLane>, reversible_allowed: bool) -> bool {
    match lane {
        Some(RiskLane::Safe) => true,
        Some(RiskLane::ReversibleDanger) => reversible_allowed,
        Some(RiskLane::IrreversibleDanger | RiskLane::Catastrophic) | None => false,
    }
}

async fn coding_agent_auto_answer(pool: &sqlx::PgPool) -> bool {
    crate::core::prefs::CODING_AGENT_AUTO_ANSWER
        .try_read(pool)
        .await
        .unwrap_or(false)
}

/// INVARIANT: Ask the command judge to place one ambiguous command, for the escalation
/// gate.
///
/// `Err` names why no verdict came back, and the caller cards. Deliberately NOT
/// `fallback_classify`, which reads an unrecognised head as benign, where an
/// auto-allow here must rest on a positive verdict.
///
/// Auto-answer on is consent to judge every coding-agent command, and nothing
/// else is: the guard's own switches never let a judge answer a coding agent,
/// so turning safety on never means asking less.
async fn judge_escalation_lane(
    engine: &LucidosEngine,
    ji: &JudgeInput,
    thread_id: Uuid,
    auto_answer: bool,
) -> Result<JudgeVerdict, String> {
    if !auto_answer {
        return Err("auto-answer is off".to_string());
    }
    engine
        .judge_command(ji, thread_id)
        .await
        .map_err(|e| format!("the judge failed ({e})"))
}

#[allow(clippy::too_many_arguments)]
async fn guard_allows_command_with_paths(
    engine: Option<&LucidosEngine>,
    tool_name: &str,
    input: &serde_json::Value,
    workspace_path: &Path,
    root: Option<&Path>,
    thread_id: Uuid,
    auto_answer: bool,
    trusted: &TrustedHeads,
    paths: &command_guard::SecretPaths,
) -> bool {
    let in_scope = match tool_name {
        CODEX_COMMAND_TOOL => true,
        CLAUDE_CODE_COMMAND_TOOL => auto_answer,
        _ => false,
    };
    if !in_scope {
        return false;
    }
    let (lane, why) =
        match static_command_verdict_at_with_paths(tool_name, input, trusted, root, paths) {
            CommandStatic::Verdict(StaticVerdict::Settled(lane)) => {
                (Some(lane), format!("static verdict {lane:?}"))
            }
            CommandStatic::Verdict(StaticVerdict::SettledIrreversible(category)) => (
                Some(RiskLane::IrreversibleDanger),
                format!("static verdict IrreversibleDanger ({category:?})"),
            ),
            CommandStatic::Verdict(StaticVerdict::NeedsJudge(ji)) if ji.fast_path_refused => (
                None,
                "a refused shape, so the judge was skipped".to_string(),
            ),
            CommandStatic::Verdict(StaticVerdict::NeedsJudge(ji)) => match engine {
                Some(engine) => {
                    match judge_escalation_lane(engine, &ji, thread_id, auto_answer).await {
                        Ok(v) => (
                            Some(v.lane),
                            format!("judge verdict {:?}: {}", v.lane, v.summary),
                        ),
                        Err(why) => (None, why),
                    }
                }
                None => (None, "no judge handle".to_string()),
            },
            CommandStatic::NotACommand | CommandStatic::Unreadable => {
                (None, "an unreadable command".to_string())
            }
        };
    let why = if !lane_allows_escalation(lane, auto_answer) {
        why
    } else if command_privilege_escalation(tool_name, input) {
        "it runs as another user".to_string()
    } else if destroys_from_request_cwd(tool_name, input, workspace_path) {
        "it destroys from a cwd outside the workspace".to_string()
    } else {
        return true;
    };
    crate::log_legacy!(
        "[CCPermission] {} cards for thread {}: {}",
        tool_name,
        thread_id,
        why
    );
    false
}

/// INVARIANT: Whether this command request reaches for another user's rights.
///
/// Reads the request's own command text rather than taking it from
/// [`static_command_verdict`], which answers about the classification and not
/// about the text. An unreadable command is already a card by then, so `false`
/// here cannot widen anything.
fn command_privilege_escalation(tool_name: &str, input: &serde_json::Value) -> bool {
    match coding_agent_command(tool_name, input) {
        CommandPayload::Known(cmd) => command_guard::command_escalates_privilege(cmd),
        CommandPayload::NotACommand | CommandPayload::Unresolved => false,
    }
}

/// INVARIANT: Whether this thread's session-allow set already covers the request.
///
/// A command tool takes the chat lane's rule through the one shared predicate,
/// [`command_guard::grant_covers_command`]: EVERY segment head must be
/// covered, and a code-injecting `VAR=value` preamble is refused outright.
/// Matching a single derived pattern instead let `git status && rm -rf /` ride
/// a grant naming only `git`. Every other tool matches its one derived pattern
/// exactly, as before. No grant covers a secret read, by command or file tool,
/// and no session grant covers a command that may write under `.claude/` or
/// `.git/`.
#[cfg(test)]
fn session_allow_covers(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
    allowed: impl Fn(&str) -> bool,
) -> bool {
    session_allow_covers_at(tool_name, input, trusted, None, allowed)
}

fn session_allow_covers_at_with_paths(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
    root: Option<&Path>,
    allowed: impl Fn(&str) -> bool,
    paths: &command_guard::SecretPaths,
) -> bool {
    use crate::engine::coding_command_grants::derive_allow_pattern;
    use crate::engine::thread_events::AllowScope;
    if file_tool_reads_secret_with_paths(tool_name, input, paths) {
        return false;
    }
    if matches!(tool_name, CLAUDE_CODE_COMMAND_TOOL | CODEX_COMMAND_TOOL) {
        let Some(command) = input.get("command").and_then(|v| v.as_str()) else {
            return false;
        };
        return command_guard::grant_covers_command_with_paths(
            tool_name,
            command,
            CommandSite::TextOnly { root },
            trusted,
            paths,
            bare_label_only_for_protected_write(tool_name, input, allowed),
        );
    }
    derive_allow_pattern(tool_name, input, AllowScope::Session, trusted)
        .is_some_and(|p| allowed(&p))
}

#[cfg(test)]
fn persisted_allow_covers(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
    allowed: impl Fn(&str) -> bool,
) -> bool {
    persisted_allow_covers_at(tool_name, input, trusted, None, allowed)
}

fn persisted_allow_covers_at_with_paths(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
    root: Option<&Path>,
    allowed: impl Fn(&str) -> bool,
    paths: &command_guard::SecretPaths,
) -> bool {
    use crate::engine::coding_command_grants::derive_allow_pattern;
    use crate::engine::thread_events::AllowScope;
    if file_tool_reads_secret_with_paths(tool_name, input, paths) {
        return false;
    }
    if !matches!(tool_name, CLAUDE_CODE_COMMAND_TOOL | CODEX_COMMAND_TOOL) {
        return [AllowScope::Broad, AllowScope::Narrow]
            .into_iter()
            .filter_map(|scope| derive_allow_pattern(tool_name, input, scope, trusted))
            .any(|pattern| allowed(&pattern));
    }
    let Some(command) = input.get("command").and_then(|v| v.as_str()) else {
        return false;
    };
    command_guard::grant_covers_command_with_paths(
        CLAUDE_CODE_COMMAND_TOOL,
        command,
        CommandSite::TextOnly { root },
        trusted,
        paths,
        bare_label_only_for_protected_write(CLAUDE_CODE_COMMAND_TOOL, input, allowed),
    )
}

fn bare_label_only_for_protected_write<'a>(
    label: &'a str,
    input: &serde_json::Value,
    allowed: impl Fn(&str) -> bool + 'a,
) -> impl Fn(&str) -> bool + 'a {
    let protected =
        crate::engine::coding_command_grants::input_touches_protected_path(label, input);
    move |pattern| (!protected || pattern == label) && allowed(pattern)
}

#[cfg(test)]
fn grant_offer(tool_name: &str, input: &serde_json::Value, trusted: &TrustedHeads) -> GrantOffer {
    grant_offer_at(tool_name, input, trusted, None)
}

fn grant_offer_at_with_paths(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
    root: Option<&Path>,
    paths: &command_guard::SecretPaths,
) -> GrantOffer {
    use crate::engine::coding_command_grants::derive_allow_pattern;
    use crate::engine::thread_events::AllowScope;
    let offered = |scope| {
        let pattern = derive_allow_pattern(tool_name, input, scope, trusted)?;
        let only = |p: &str| p == pattern;
        let covers = match scope {
            AllowScope::Session => {
                session_allow_covers_at_with_paths(tool_name, input, trusted, root, only, paths)
            }
            AllowScope::Broad | AllowScope::Narrow => {
                persisted_allow_covers_at_with_paths(tool_name, input, trusted, root, only, paths)
            }
        };
        covers.then_some(pattern)
    };
    let deny = match coding_agent_command(tool_name, input) {
        CommandPayload::Known(command) => {
            command_guard::grant_head(command, CommandSite::TextOnly { root }, trusted)
                .map(|head| format!("Bash({head}:*)"))
                .and_then(|line| {
                    standing_deny::card_rule(line, standing_deny::CommandKind::Shell, command)
                })
        }
        _ => None,
    };
    if request_reads_secret_with_paths(tool_name, input, paths) {
        return GrantOffer {
            deny,
            ..GrantOffer::withheld(OfferWithheld::SecretRead)
        };
    }
    GrantOffer {
        deny,
        broad: offered(AllowScope::Broad),
        narrow: offered(AllowScope::Narrow),
        session: offered(AllowScope::Session),
        withheld: None,
    }
}

fn permission_event_input(input: &serde_json::Value) -> serde_json::Value {
    let secrets = crate::core::subprocess_secret_values(&[], crate::core::inherited_secret_env());
    let mut masked = input.clone();
    crate::core::redact_subprocess_output_in_json(&mut masked, &secrets);
    masked
}

fn without_secret_patterns(offer: GrantOffer) -> GrantOffer {
    let clean = |pattern: Option<String>| {
        pattern.filter(|p| {
            let value = serde_json::Value::String(p.clone());
            permission_event_input(&value) == value
        })
    };
    GrantOffer {
        deny: clean(offer.deny),
        broad: clean(offer.broad),
        narrow: clean(offer.narrow),
        session: clean(offer.session),
        withheld: offer.withheld,
    }
}

fn request_reads_secret_with_paths(
    tool_name: &str,
    input: &serde_json::Value,
    paths: &command_guard::SecretPaths,
) -> bool {
    file_tool_reads_secret_with_paths(tool_name, input, paths)
        || matches!(
            coding_agent_command(tool_name, input),
            CommandPayload::Known(command) if paths.command(command)
        )
}

pub struct CodingAgentPermissionInput {
    pub thread_id: Uuid,
    pub tool_use_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
}

pub async fn lookup_session_worktree(
    agent_sessions: &tokio::sync::Mutex<HashMap<Uuid, crate::engine::types::AgentSession>>,
    thread_id: Uuid,
) -> Option<std::path::PathBuf> {
    let sessions = agent_sessions.lock().await;
    sessions.get(&thread_id)?.worktree_path.clone()
}

async fn request_trusted_scripts(
    tool_name: &str,
    input: &serde_json::Value,
    worktree_path: Option<&Path>,
) -> TrustedHeads {
    let CommandPayload::Known(command) = coding_agent_command(tool_name, input) else {
        return TrustedHeads::NONE;
    };
    let cwd = input.get("cwd").and_then(|v| v.as_str()).map(Path::new);
    trusted_repo_scripts(command, cwd, worktree_path).await
}

/// INVARIANT: One blocking permission round-trip: the shared core both raise paths drive,
/// CC's MCP HTTP path and the Codex app-server bridge. Every session gets the
/// same gates, whoever started it: the spawn gate already refused a run its
/// cause did not consent to (`agent_session::spawn_gate`). The gates, in order:
///
///   1. **Catastrophic floor and standing denies.** No grant or preference
///      lifts a refusal. An unavailable standing deny check goes to the card.
///   2. **In-worktree write fast path.** A file write inside the session's own
///      worktree needs no card. Checked first because it is pure and DB-free.
///   3. **Thread grant pre-check.** An earlier "Allow for this thread" click
///      whose pattern matches skips the prompt. The grant is a durable row,
///      so it survives an engine restart.
///   4. **Standing grant pre-check.** An earlier "Always allow" click whose
///      pattern is a standing grant of this workspace skips the prompt.
///   5. **Command gate.** A Codex `command_execution` escape the
///      command guard places as `Safe` skips the card. Under
///      `coding_agent_auto_answer` a `ReversibleDanger` one does too, and a
///      Claude Code `Bash` request enters the gate. See
///      [`guard_allows_command`].
///   6. **Card.** [`park_card`] dedups and emits the card, and the wait on the
///      broadcast is **indefinite**.
///
/// The paired `CodingAgentPermissionResolved` is emitted by the consent
/// endpoint, so it fires once per click rather than once per deduped listener.
///
/// `engine` serves step 5's LLM half. `None` leaves its static half.
pub async fn prompt_coding_agent_permission(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    pending: &Mutex<PermissionState>,
    workspace_path: &Path,
    worktree_path: Option<&Path>,
    engine: Option<&SharedEngine>,
    request: CodingAgentPermissionInput,
) -> PermissionPromptOutcome {
    let CodingAgentPermissionInput {
        thread_id,
        tool_use_id,
        tool_name,
        input,
    } = request;

    if let Some(reason) = open_question_refusal(pool, thread_id).await {
        record_permission_denial(
            pool,
            event_bus,
            thread_id,
            &tool_use_id,
            &tool_name,
            &input,
            reason,
        )
        .await;
        return PermissionPromptOutcome {
            allowed: false,
            reason: Some(reason.to_string()),
        };
    }

    let captured = match engine {
        Some(engine) => engine
            .agent_sessions
            .lock()
            .await
            .get(&thread_id)
            .and_then(|session| session.execution_sandbox.clone()),
        None => None,
    };
    let paths = captured
        .as_ref()
        .map(|sandbox| command_guard::SecretPaths::for_sandbox(sandbox, workspace_path))
        .unwrap_or_else(command_guard::SecretPaths::conservative);
    let paths = &paths;
    let secret_read = request_reads_secret_with_paths(&tool_name, &input, paths);

    // INVARIANT: Use the session's trusted worktree for the floor's exemption, while
    // retaining the request's cwd.
    let mut unproven_floor = false;
    if let CommandPayload::Known(command) = coding_agent_command(&tool_name, &input) {
        let command = if let Some(cwd) = input
            .get("cwd")
            .or_else(|| input.get("workdir"))
            .and_then(|v| v.as_str())
        {
            format!("cd {:?} && {}", cwd, command)
        } else {
            command.to_string()
        };
        match command_guard::catastrophic_reason_for_command_at(
            &command,
            worktree_path.or(Some(workspace_path)),
        ) {
            Some(Catastrophe::Proven(reason)) => {
                let reason = format!("Refused by the command guard: {reason}");
                record_permission_denial(
                    pool,
                    event_bus,
                    thread_id,
                    &tool_use_id,
                    &tool_name,
                    &input,
                    &reason,
                )
                .await;
                return PermissionPromptOutcome {
                    allowed: false,
                    reason: Some(reason),
                };
            }
            Some(Catastrophe::Unproven(_)) => unproven_floor = true,
            None => {}
        }
    }

    // INVARIANT: The owner's standing deny beats every allow below, and the judge.
    let rules_unavailable = if let CommandPayload::Known(command) =
        coding_agent_command(&tool_name, &input)
    {
        match standing_deny::check_refusal(pool, standing_deny::CommandKind::Shell, command).await {
            Ok(Some(reason)) => {
                record_permission_denial(
                    pool,
                    event_bus,
                    thread_id,
                    &tool_use_id,
                    &tool_name,
                    &input,
                    &reason,
                )
                .await;
                return PermissionPromptOutcome {
                    allowed: false,
                    reason: Some(reason),
                };
            }
            Ok(None) => false,
            Err(_) => true,
        }
    } else {
        false
    };

    let offer = if unproven_floor {
        GrantOffer::withheld(OfferWithheld::UnprovenWorkspaceDestruction)
    } else if rules_unavailable {
        GrantOffer::withheld(if secret_read {
            OfferWithheld::StandingDenyUnavailableSecretRead
        } else {
            OfferWithheld::StandingDenyUnavailable
        })
    } else {
        if worktree_write_auto_allowed(&tool_name, &input, worktree_path) {
            return PermissionPromptOutcome {
                allowed: true,
                reason: Some(WORKTREE_WRITE_ALLOW_REASON.to_string()),
            };
        }

        let trusted = request_trusted_scripts(&tool_name, &input, worktree_path).await;

        let granted =
            tool_grants::granted_lines(event_bus, GrantFile::CodingAgentTools, Some(thread_id))
                .await;
        if session_allow_covers_at_with_paths(
            &tool_name,
            &input,
            &trusted,
            worktree_path.or(Some(workspace_path)),
            |p| granted.thread.iter().any(|g| g == p),
            paths,
        ) {
            return PermissionPromptOutcome {
                allowed: true,
                reason: Some(SESSION_ALLOW_REASON.to_string()),
            };
        }
        if persisted_allow_covers_at_with_paths(
            &tool_name,
            &input,
            &trusted,
            worktree_path.or(Some(workspace_path)),
            |p| granted.standing.iter().any(|g| g == p),
            paths,
        ) {
            return PermissionPromptOutcome {
                allowed: true,
                reason: Some(PERSISTED_ALLOW_REASON.to_string()),
            };
        }

        let is_command = matches!(
            tool_name.as_str(),
            CODEX_COMMAND_TOOL | CLAUDE_CODE_COMMAND_TOOL
        );
        let auto_answer = is_command && coding_agent_auto_answer(pool).await;
        if guard_allows_command_with_paths(
            engine.map(|e| &**e),
            &tool_name,
            &input,
            workspace_path,
            worktree_path.or(Some(workspace_path)),
            thread_id,
            auto_answer,
            &trusted,
            paths,
        )
        .await
        {
            crate::log_legacy!(
                "[CCPermission] command auto-allowed for thread {}",
                thread_id
            );
            return PermissionPromptOutcome {
                allowed: true,
                reason: Some(if secret_read {
                    format!(
                        "{COMMAND_GUARD_ALLOW_REASON}. {}",
                        command_guard::SECRET_READ_STEER
                    )
                } else {
                    COMMAND_GUARD_ALLOW_REASON.to_string()
                }),
            };
        }

        without_secret_patterns(grant_offer_at_with_paths(
            &tool_name,
            &input,
            &trusted,
            worktree_path.or(Some(workspace_path)),
            paths,
        ))
    };
    let mut answer = park_card(
        pool,
        event_bus,
        pending,
        thread_id,
        tool_use_id,
        tool_name,
        &input,
        offer,
    )
    .await;
    let mut outcome = outcome_from_permission_recv(answer.recv().await);
    if secret_read && outcome.reason.as_deref() == Some(DENIAL_REASON) {
        outcome.reason = Some(format!(
            "{DENIAL_REASON}. {}",
            command_guard::SECRET_READ_STEER
        ));
    }
    outcome
}

async fn card_event_meta(pool: &sqlx::PgPool, thread_id: Uuid) -> EventMeta {
    match lookup_thread_actor(pool, thread_id).await {
        Some(actor) => EventMeta::with_actor(Some(actor)),
        None => EventMeta::NONE,
    }
}

#[allow(clippy::too_many_arguments)]
async fn park_card(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    pending: &Mutex<PermissionState>,
    thread_id: Uuid,
    tool_use_id: String,
    tool_name: String,
    input: &serde_json::Value,
    offer: GrantOffer,
) -> tokio::sync::broadcast::Receiver<PermissionAnswer> {
    // INVARIANT: A request with a fallback notice cannot share a verdict with an
    // ordinary request for the same command.
    let canonical_input =
        serde_json::to_string(&(input, offer.withheld)).expect("JSON value and enum serialize");
    let dedup_key: DedupKey = (thread_id, tool_name.clone(), canonical_input);
    // INVARIANT: The dedup key and the pending entry keep the verbatim input, which is what
    // the agent asked to run and what the approval must match. The event is
    // persisted and fanned out over SSE, so it carries a redacted COPY.
    let event_input = permission_event_input(input);
    let summary = build_permission_summary(&tool_name, &event_input);

    let (request_id, rx, is_canonical) = {
        let mut pending = pending.lock().unwrap();
        pending.register_or_attach(
            dedup_key,
            thread_id,
            tool_name.clone(),
            input.clone(),
            offer.clone(),
        )
    };

    if is_canonical {
        let request = ThreadEvent::CodingAgentPermissionRequest {
            request_id: request_id.clone(),
            tool_use_id,
            tool_name,
            input: event_input,
            summary,
            grant_offer: Some(offer),
        };
        let mut unposted = UnpostedCard {
            pending,
            request_id: Some(request_id),
        };
        let meta = card_event_meta(pool, thread_id).await;
        let posted =
            super::card_queue::post_permission_prompt(event_bus, thread_id, request, meta, None)
                .await;
        match posted {
            Ok(_) => unposted.request_id = None,
            Err(error) => {
                crate::log_legacy!("[CCPermission] Could not publish the permission card: {error}")
            }
        }
    }
    rx
}

struct UnpostedCard<'a> {
    pending: &'a Mutex<PermissionState>,
    request_id: Option<String>,
}

impl Drop for UnpostedCard<'_> {
    fn drop(&mut self) {
        let Some(request_id) = self.request_id.take() else {
            return;
        };
        if let Some(entry) = self.pending.lock().unwrap().take(&request_id) {
            let _ = entry.tx.send(PermissionAnswer::Refused(
                "Could not show the permission card. Try again later.".into(),
            ));
        }
    }
}

/// INVARIANT: Record an automatic denial in the thread's timeline, as the ordinary
/// request/resolved pair rather than a new event type.
///
/// An automatic ALLOW emits nothing, deliberately: there is no decision to
/// show. A deny is different. The agent reports a failed step, and without
/// this the user cannot see what the engine refused or why. The two events land
/// in one atomic emit, so the card renders already answered and the thread is
/// never left needing attention.
///
/// The command text is redacted for the event exactly as a card redacts it:
/// it is persisted AND fanned out over SSE.
async fn record_permission_denial(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    thread_id: Uuid,
    tool_use_id: &str,
    tool_name: &str,
    input: &serde_json::Value,
    reason: &str,
) {
    let request_id = Uuid::new_v4().to_string();
    let event_input = permission_event_input(input);
    let summary = build_permission_summary(tool_name, &event_input);
    let meta = card_event_meta(pool, thread_id).await;
    let request = ThreadEvent::CodingAgentPermissionRequest {
        request_id: request_id.clone(),
        tool_use_id: tool_use_id.to_string(),
        tool_name: tool_name.to_string(),
        input: event_input,
        summary,
        grant_offer: None,
    };
    let resolved =
        coding_agent_permission_resolved(request_id, false, Some(reason.to_string()), None, None);
    if let Err(e) = event_bus
        .emit_atomically(thread_id, vec![(request, meta.clone()), (resolved, meta)])
        .await
    {
        crate::log_legacy!("[CCPermission] Could not record the auto-deny: {e}");
    }
}

pub async fn open_question_refusal(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<&'static str> {
    use crate::engine::question_park_sql::{agent_question_open, OPEN_QUESTION_TOOL_REFUSAL};
    match agent_question_open(pool, thread_id).await {
        Ok(true) => Some(OPEN_QUESTION_TOOL_REFUSAL),
        Ok(false) => None,
        Err(e) => {
            crate::log_legacy!("[CCPermission] Open question on {thread_id} unreadable: {e}");
            None
        }
    }
}

/// INVARIANT: Map the broadcast `recv` result for a pending permission into the outcome
/// relayed to the agent. Only a human's Deny reads as `DENIAL_REASON`:
///   * `Withdrawn` names the engine's reason and says nobody decided.
///   * `Err(_)` means the channel CLOSED, which can only be the engine tearing
///     down. Every live resolution path sends before dropping the sender, and
///     `gc_dead_entries` never reaps an entry whose receiver is still awaiting.
///     It carries the neutral `RESTART_INTERRUPT_REASON`.
///
/// Otherwise the agent reads a rejection of an approach nobody rejected.
fn outcome_from_permission_recv(
    recv: Result<PermissionAnswer, tokio::sync::broadcast::error::RecvError>,
) -> PermissionPromptOutcome {
    match recv {
        Ok(PermissionAnswer::Allowed) => PermissionPromptOutcome {
            allowed: true,
            reason: None,
        },
        Ok(PermissionAnswer::Denied) => PermissionPromptOutcome {
            allowed: false,
            reason: Some(DENIAL_REASON.to_string()),
        },
        Ok(PermissionAnswer::Refused(reason)) => PermissionPromptOutcome {
            allowed: false,
            reason: Some(reason),
        },
        Ok(PermissionAnswer::Withdrawn(reason)) => PermissionPromptOutcome {
            allowed: false,
            reason: Some(format!(
                "{reason}, not a user decision. Nobody denied this request, so do not read it \
                 as a rejection of your approach."
            )),
        },
        Err(_) => PermissionPromptOutcome {
            allowed: false,
            reason: Some(RESTART_INTERRUPT_REASON.to_string()),
        },
    }
}

pub async fn record_coding_agent_allow_grant(
    engine: &LucidosEngine,
    entry: &PermissionEntry,
    scope: crate::engine::thread_events::AllowScope,
    answered_by: AnsweredBy,
) {
    let Some(pattern) = entry.grant_offer.pattern(scope).map(str::to_string) else {
        return;
    };
    if let Err(e) = tool_grants::record_card_allow(
        &engine.event_bus,
        GrantFile::CodingAgentTools,
        entry.thread_id,
        &entry.request_id,
        pattern.clone(),
        scope,
        answered_by,
    )
    .await
    {
        crate::log_legacy!("[CCPermission] Failed to record allow pattern {pattern:?}: {e}");
    }
}

fn coding_agent_permission_resolved(
    request_id: String,
    allowed: bool,
    reason: Option<String>,
    persist_scope: Option<crate::engine::thread_events::AllowScope>,
    standing_deny: Option<String>,
) -> ThreadEvent {
    ThreadEvent::CodingAgentPermissionResolved {
        request_id,
        allowed,
        reason,
        persist_scope,
        standing_deny,
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn emit_coding_agent_permission_resolved(
    event_bus: &EventBus,
    thread_id: Uuid,
    request_id: String,
    allowed: bool,
    reason: Option<String>,
    persist_scope: Option<crate::engine::thread_events::AllowScope>,
    standing_deny: Option<String>,
    meta: EventMeta,
    log_context: &str,
) {
    event_bus
        .emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: coding_agent_permission_resolved(
                    request_id,
                    allowed,
                    reason,
                    persist_scope,
                    standing_deny,
                ),
                meta,
            },
            log_context,
        )
        .await;
}

/// INVARIANT: Answer one coding-agent permission card, whoever answered it.
///
/// This lane's half of the contract `command_permission::resolve_command_permission`
/// documents: the single in-process path, three effects in order, and `false`
/// for a request id nothing is waiting on. The blocked waiter here is the
/// agent's own subprocess handler rather than an in-process loop.
pub async fn resolve_coding_agent_permission(
    engine: &LucidosEngine,
    request_id: String,
    allowed: bool,
    persist_scope: Option<crate::engine::thread_events::AllowScope>,
    always_deny: bool,
    answered_by: AnsweredBy,
    log_context: &str,
) -> Result<bool, String> {
    let entry = {
        let mut pending = engine.pending_cc_permission.lock().unwrap();
        pending.take(&request_id)
    };
    let Some(entry) = entry else {
        return Ok(false);
    };
    let standing_deny = if always_deny {
        Some(standing_deny::record_card_rule(engine, &entry, &answered_by).await)
    } else {
        None
    };
    let blocked = match (
        allowed,
        coding_agent_command(&entry.tool_name, &entry.input),
    ) {
        (true, CommandPayload::Known(command)) => {
            let check = standing_deny::check_refusal(
                &engine.pool,
                standing_deny::CommandKind::Shell,
                command,
            )
            .await;
            let fallback_allow_once = entry
                .grant_offer
                .withheld
                .is_some_and(OfferWithheld::standing_deny_unavailable)
                && persist_scope.is_none()
                && !always_deny;
            match check {
                Ok(refusal) => refusal,
                Err(_) if fallback_allow_once => None,
                Err(error) => Some(standing_deny::unreadable_refusal(&error)),
            }
        }
        _ => None,
    };
    let allowed = allowed && blocked.is_none();
    let refusal = blocked.clone().or_else(|| {
        standing_deny
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .map(|line| standing_deny::refusal(line))
    });
    let answer = refusal.map_or_else(
        || PermissionAnswer::from_decision(allowed),
        PermissionAnswer::Refused,
    );
    let reason = blocked.or_else(|| (!allowed).then(|| DENIAL_REASON.to_string()));
    let persist_scope =
        persist_scope.filter(|scope| allowed && entry.grant_offer.pattern(*scope).is_some());
    if let Some(scope) = persist_scope {
        record_coding_agent_allow_grant(engine, &entry, scope, answered_by.clone()).await;
    }
    let _ = entry.tx.send(answer);
    emit_coding_agent_permission_resolved(
        &engine.event_bus,
        entry.thread_id,
        request_id,
        allowed,
        reason,
        persist_scope,
        standing_deny
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .cloned(),
        EventMeta::with_actor(answered_by.actor),
        log_context,
    )
    .await;
    if let Some(engine) = engine.try_clone_arc() {
        engine
            .agent_host()
            .spawn_held_message_release(&engine, entry.thread_id);
    }
    if let Some(Err(error)) = standing_deny {
        return Err(format!(
            "The command was denied. Could not save the standing deny: {error}"
        ));
    }
    Ok(true)
}

pub async fn resolve_pending_permissions_as_superseded(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    pending: &Mutex<PermissionState>,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) {
    resolve_pending_permissions_with_reason(
        pool,
        event_bus,
        pending,
        thread_id,
        actor,
        SUPERSEDED_REASON,
        "[CCPermission] CodingAgentPermissionResolved (superseded)",
    )
    .await;
}

/// INVARIANT: Resolve every unresolved `CodingAgentPermissionRequest` on `thread_id` as
/// denied, because the session IDLED with a card still dangling. Thin wrapper
/// over [`resolve_pending_permissions_with_reason`], called from
/// `emit_coding_agent_idled` at the turn boundary. The turn is done by then, so
/// a still-pending card is orphaned and clearing it is safe: a live card blocks
/// the turn, so idle cannot fire during a genuine wait.
pub async fn resolve_pending_permissions_as_session_ended(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    pending: &Mutex<PermissionState>,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) {
    resolve_pending_permissions_with_reason(
        pool,
        event_bus,
        pending,
        thread_id,
        actor,
        SESSION_ENDED_REASON,
        "[CCPermission] CodingAgentPermissionResolved (session ended)",
    )
    .await;
}

const UNRESOLVED_PERMISSION_REQUESTS_SQL: &str = "SELECT e.payload->>'request_id' \
     FROM events e \
     WHERE e.event_type = 'CodingAgentPermissionRequest' \
       AND e.thread_id = $1 \
       AND NOT EXISTS ( \
         SELECT 1 FROM events r \
         WHERE r.event_type = 'CodingAgentPermissionResolved' \
           AND r.payload->>'request_id' = e.payload->>'request_id' \
       )";

pub async fn has_pending_permission_card(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(&format!(
        "SELECT EXISTS ({UNRESOLVED_PERMISSION_REQUESTS_SQL})"
    ))
    .bind(thread_id)
    .fetch_one(pool)
    .await
}

/// INVARIANT: Shared core of the two sweeps that clear every unresolved
/// `CodingAgentPermissionRequest` on this thread as denied: the superseded path
/// and the session-ended path. Mirrors `recover_orphan_cc_permission_requests`
/// but scoped to one thread. Two effects per unresolved request:
///
///   1. Fan a `Withdrawn` out to any still-blocked handler through the
///      in-memory broadcast entry. The subprocess's pending call then returns
///      immediately, rather than dangling until the next sweep.
///   2. Emit a denied `CodingAgentPermissionResolved`, so the card's buttons
///      stop dangling. Without it the card sits clickable forever and the
///      thread reads as stuck.
///
/// A resolution flips the thread to `running` ONLY from
/// `waiting_for_user_answer`, so emitting these on an already-idle thread
/// clears the stale card WITHOUT resurrecting it. `reason` and `log_label`
/// distinguish the two callers. No-op when nothing is pending.
async fn resolve_pending_permissions_with_reason(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    pending: &Mutex<PermissionState>,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
    reason: &'static str,
    log_label: &str,
) {
    let rows: Vec<(Option<String>,)> = match sqlx::query_as(UNRESOLVED_PERMISSION_REQUESTS_SQL)
        .bind(thread_id)
        .fetch_all(pool)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            crate::log_legacy!(
                "[CCPermission] unresolved-permission query failed for {}: {}",
                thread_id,
                e
            );
            return;
        }
    };

    for (request_id,) in rows {
        let Some(request_id) = request_id.filter(|s| !s.is_empty()) else {
            continue;
        };
        {
            let mut state = pending.lock().unwrap();
            if let Some(entry) = state.take(&request_id) {
                let _ = entry.tx.send(PermissionAnswer::Withdrawn(reason));
            }
        }
        emit_coding_agent_permission_resolved(
            event_bus,
            thread_id,
            request_id,
            false,
            Some(reason.to_string()),
            None,
            None,
            EventMeta::with_actor(actor.clone()),
            log_label,
        )
        .await;
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn static_command_verdict_at(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
    root: Option<&Path>,
) -> CommandStatic {
    static_command_verdict_at_with_paths(
        tool_name,
        input,
        trusted,
        root,
        &command_guard::SecretPaths::conservative(),
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn file_tool_reads_secret(tool_name: &str, input: &serde_json::Value) -> bool {
    file_tool_reads_secret_with_paths(
        tool_name,
        input,
        &command_guard::SecretPaths::conservative(),
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn session_allow_covers_at(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
    root: Option<&Path>,
    allowed: impl Fn(&str) -> bool,
) -> bool {
    session_allow_covers_at_with_paths(
        tool_name,
        input,
        trusted,
        root,
        allowed,
        &command_guard::SecretPaths::conservative(),
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn persisted_allow_covers_at(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
    root: Option<&Path>,
    allowed: impl Fn(&str) -> bool,
) -> bool {
    persisted_allow_covers_at_with_paths(
        tool_name,
        input,
        trusted,
        root,
        allowed,
        &command_guard::SecretPaths::conservative(),
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn grant_offer_at(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
    root: Option<&Path>,
) -> GrantOffer {
    grant_offer_at_with_paths(
        tool_name,
        input,
        trusted,
        root,
        &command_guard::SecretPaths::conservative(),
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
async fn guard_allows_command(
    engine: Option<&LucidosEngine>,
    tool_name: &str,
    input: &serde_json::Value,
    workspace_path: &Path,
    root: Option<&Path>,
    thread_id: Uuid,
    auto_answer: bool,
    trusted: &TrustedHeads,
) -> bool {
    guard_allows_command_with_paths(
        engine,
        tool_name,
        input,
        workspace_path,
        root,
        thread_id,
        auto_answer,
        trusted,
        &command_guard::SecretPaths::conservative(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::thread_events::EngineReason;
    use std::sync::Arc;

    #[tokio::test]
    #[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
    async fn live_session_uses_spawn_policy_after_preference_changes() {
        use crate::engine::session_claim::{claim_thread_for, ClaimFor};
        use crate::runtime::agent_sandbox::{policy_from, PolicyInputs, Sandbox};
        use crate::test_support::{
            boot_mock_engine_with_host, in_engine_child, setup_test_db, teardown_test_db,
        };
        if !in_engine_child(
            "engine::cc_permission::tests::live_session_uses_spawn_policy_after_preference_changes",
        )
        .await
        {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let thread_id = Uuid::new_v4();
        let host = Arc::new(crate::engine::agent_host::NoAgentHost {
            held_message_release_thread: Some(thread_id),
        });
        let (engine, workspace) = boot_mock_engine_with_host(&db, host).await;
        seed_cc_thread(&engine.event_bus, thread_id).await;
        let _claim = claim_thread_for(
            &mut *engine.agent_sessions.lock().await,
            thread_id,
            crate::runtime::CodingAgent::ClaudeCode,
            ClaimFor::Run,
        )
        .unwrap();
        tool_grants::record_card_allow(
            &engine.event_bus,
            GrantFile::CodingAgentTools,
            thread_id,
            "prior-grep-card",
            "Bash(grep:*)".into(),
            crate::engine::thread_events::AllowScope::Narrow,
            AnsweredBy::tap(None),
        )
        .await
        .unwrap();
        crate::core::preferences::PreferenceStore::set_row_for_test(
            &pool,
            crate::core::prefs::AGENTS_UNSANDBOXED.spec.key,
            "true",
        )
        .await
        .unwrap();
        let bare = Sandbox::resolve(&pool, workspace.path()).await.unwrap();
        let gateway = workspace.path().join(".lucidos/gateway");
        let policy = policy_from(&PolicyInputs {
            lucidos_home: None,
            data_dirs: vec![gateway.clone()],
            workspaces: vec![],
        });
        for (index, sandbox) in [
            bare,
            Sandbox::inherited(),
            Sandbox::enforced_for_test(policy),
        ]
        .into_iter()
        .enumerate()
        {
            engine
                .agent_sessions
                .lock()
                .await
                .get_mut(&thread_id)
                .unwrap()
                .execution_sandbox = Some(sandbox);
            crate::core::preferences::PreferenceStore::set_row_for_test(
                &pool,
                crate::core::prefs::AGENTS_UNSANDBOXED.spec.key,
                "false",
            )
            .await
            .unwrap();
            let engine_for_call = engine.clone();
            let workspace_path = workspace.path().to_path_buf();
            let input = serde_json::json!({"command": format!("grep warning {}/workspaces/demo/log", gateway.display())});
            let waiter = tokio::spawn(async move {
                prompt_coding_agent_permission(
                    engine_for_call.pool(),
                    &engine_for_call.event_bus,
                    &engine_for_call.pending_cc_permission,
                    &workspace_path,
                    None,
                    Some(&engine_for_call),
                    CodingAgentPermissionInput {
                        thread_id,
                        tool_use_id: format!("spawn-policy-{index}"),
                        tool_name: "Bash".into(),
                        input,
                    },
                )
                .await
            });
            if index < 2 {
                let request_id = await_canonical_request(&engine.pending_cc_permission).await;
                let withheld = {
                    let state = engine.pending_cc_permission.lock().unwrap();
                    let key = &state.by_request_id[&request_id];
                    state.by_dedup_key[key].grant_offer.withheld
                };
                assert_eq!(withheld, Some(OfferWithheld::SecretRead));
                assert!(resolve_coding_agent_permission(
                    &engine,
                    request_id,
                    false,
                    None,
                    false,
                    AnsweredBy::tap(Some(device("owner"))),
                    "[test] spawn policy deny",
                )
                .await
                .unwrap());
            }
            let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), waiter)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(outcome.allowed, index == 2);
            if !outcome.allowed {
                let reason = outcome.reason.as_deref().unwrap();
                assert!(reason.starts_with(DENIAL_REASON));
                assert!(reason.contains(command_guard::SECRET_READ_STEER));
            }
        }
        drop(engine);
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[test]
    fn a_shell_wrapper_keeps_outer_secret_reads_in_the_static_floor() {
        for tool in ["Bash", "command_execution"] {
            let input = serde_json::json!({"command": "bash -c 'echo done' $(cat ~/.lucidos/private/local-token)"});
            assert!(matches!(
                static_command_verdict(tool, &input, &TrustedHeads::NONE),
                CommandStatic::Verdict(StaticVerdict::SettledIrreversible(
                    command_guard::SideEffectCategory::SecretRead
                ))
            ));
        }
    }

    #[test]
    fn executing_policy_covers_both_backends_and_file_tools() {
        use crate::runtime::agent_sandbox::{policy_from, PolicyInputs, SandboxMode};
        let policy = policy_from(&PolicyInputs {
            lucidos_home: None,
            data_dirs: vec!["/srv/gateway".into()],
            workspaces: vec![],
        });
        for mode in [
            SandboxMode::Enforced,
            SandboxMode::Inherited,
            SandboxMode::OwnerOptedOut,
            SandboxMode::PermittedOutside,
        ] {
            let paths = command_guard::SecretPaths::for_policy(&policy, Some(mode));
            for (path, open) in [
                ("/srv/gateway/workspaces/demo/log", true),
                ("/srv/gateway//workspaces/./demo/log", true),
                ("/srv/gateway/workspaces/demo/../../config/key", false),
                ("/srv/gateway/config/key", false),
                (".lucidos/private/local-token", false),
            ] {
                let expected = open && mode == SandboxMode::Enforced;
                for tool in ["Bash", "command_execution", "Read", "Grep", "Glob"] {
                    let input = if matches!(tool, "Bash" | "command_execution") {
                        serde_json::json!({"command": format!("grep warning {path}")})
                    } else {
                        serde_json::json!({"file_path": path, "path": path})
                    };
                    let offer =
                        grant_offer_at_with_paths(tool, &input, &TrustedHeads::NONE, None, &paths);
                    assert_eq!(
                        offer.session.is_some(),
                        expected,
                        "{mode:?}: {tool}: {path}"
                    );
                    assert_eq!(
                        session_allow_covers_at_with_paths(
                            tool,
                            &input,
                            &TrustedHeads::NONE,
                            None,
                            |_| true,
                            &paths
                        ),
                        expected
                    );
                    assert_eq!(
                        persisted_allow_covers_at_with_paths(
                            tool,
                            &input,
                            &TrustedHeads::NONE,
                            None,
                            |_| true,
                            &paths
                        ),
                        expected
                    );
                }
            }
        }
    }

    #[test]
    fn build_permission_summary_uses_file_path() {
        let s = build_permission_summary(
            "Edit",
            &serde_json::json!({ "file_path": "/tmp/foo.md", "old_string": "x" }),
        );
        assert_eq!(s, "Edit /tmp/foo.md");
    }

    #[test]
    fn build_permission_summary_falls_back_to_command() {
        let s = build_permission_summary("Bash", &serde_json::json!({ "command": "ls -la" }));
        assert_eq!(s, "Bash ls -la");
        let s = build_permission_summary(
            "command_execution",
            &serde_json::json!({ "command": "sudo rm -rf /tmp/x", "cwd": "/wt" }),
        );
        assert_eq!(s, "command_execution sudo rm -rf /tmp/x");
    }

    #[test]
    fn build_permission_summary_returns_tool_name_when_no_arg_field() {
        let s = build_permission_summary("WeirdTool", &serde_json::json!({ "foo": 1 }));
        assert_eq!(s, "WeirdTool");
    }

    #[test]
    fn build_permission_summary_uses_skill_for_skill_tool() {
        let s = build_permission_summary("Skill", &serde_json::json!({ "skill": "update-config" }));
        assert_eq!(s, "skill update-config");
    }

    #[test]
    fn build_permission_summary_names_the_codex_changed_files() {
        let one = build_permission_summary(
            "file_change",
            &serde_json::json!({
                "item_id": "exec-1",
                "changes": [{"path": "/Users/me/notes.txt", "kind": {"type": "add"}}],
            }),
        );
        assert_eq!(one, "file_change /Users/me/notes.txt");

        let three = build_permission_summary(
            "file_change",
            &serde_json::json!({"changes": [
                {"path": "/a.rs"}, {"path": "/b.rs"}, {"path": "/c.rs"},
            ]}),
        );
        assert_eq!(three, "file_change /a.rs, /b.rs, /c.rs");

        let many = build_permission_summary(
            "file_change",
            &serde_json::json!({"changes": [
                {"path": "/a.rs"}, {"path": "/b.rs"}, {"path": "/c.rs"},
                {"path": "/d.rs"}, {"path": "/e.rs"},
            ]}),
        );
        assert_eq!(many, "file_change /a.rs, /b.rs, /c.rs +2 more");
    }

    #[test]
    fn build_permission_summary_prefers_paths_over_the_codex_reason() {
        let s = build_permission_summary(
            "file_change",
            &serde_json::json!({
                "reason": "writes outside worktree",
                "grant_root": "/etc",
                "changes": [{"path": "/etc/hosts", "kind": {"type": "update"}}],
            }),
        );
        assert_eq!(s, "file_change /etc/hosts");
    }

    #[test]
    fn build_permission_summary_falls_back_when_the_change_set_is_unknown() {
        let s = build_permission_summary(
            "file_change",
            &serde_json::json!({"item_id": "exec-1", "reason": "needs write access"}),
        );
        assert_eq!(s, "file_change needs write access");
        let bare = build_permission_summary("file_change", &serde_json::json!({"changes": []}));
        assert_eq!(bare, "file_change");
    }

    #[test]
    fn build_permission_summary_uses_url_for_webfetch() {
        let s = build_permission_summary(
            "WebFetch",
            &serde_json::json!({ "url": "https://example.com", "prompt": "x" }),
        );
        assert_eq!(s, "WebFetch https://example.com");
    }

    #[test]
    fn outcome_allow_has_no_reason() {
        let o = outcome_from_permission_recv(Ok(PermissionAnswer::Allowed));
        assert!(o.allowed);
        assert_eq!(o.reason, None);
    }

    #[test]
    fn outcome_explicit_deny_is_user_denied() {
        let o = outcome_from_permission_recv(Ok(PermissionAnswer::Denied));
        assert!(!o.allowed);
        assert_eq!(o.reason.as_deref(), Some(DENIAL_REASON));
    }

    #[test]
    fn outcome_withdrawn_names_the_reason_and_is_not_user_denied() {
        let o = outcome_from_permission_recv(Ok(PermissionAnswer::Withdrawn(SUPERSEDED_REASON)));
        assert!(!o.allowed);
        let reason = o.reason.unwrap();
        assert!(reason.starts_with(SUPERSEDED_REASON), "{reason}");
        assert!(reason.contains("not a user decision"), "{reason}");
    }

    #[test]
    fn outcome_closed_channel_is_restart_not_user_denied() {
        use tokio::sync::broadcast::error::RecvError;
        let o = outcome_from_permission_recv(Err(RecvError::Closed));
        assert!(!o.allowed);
        assert_eq!(o.reason.as_deref(), Some(RESTART_INTERRUPT_REASON));
        assert_ne!(o.reason.as_deref(), Some(DENIAL_REASON));
    }

    fn register(
        state: &mut PermissionState,
        key: DedupKey,
    ) -> (
        String,
        tokio::sync::broadcast::Receiver<PermissionAnswer>,
        bool,
    ) {
        let tool_name = key.1.clone();
        state.register_or_attach(
            key,
            Uuid::nil(),
            tool_name,
            serde_json::json!({}),
            GrantOffer::default(),
        )
    }

    #[test]
    fn register_or_attach_creates_canonical_entry_first_time() {
        let mut state = PermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".into(), "{}".into());
        let (request_id, _rx, is_canonical) = register(&mut state, key.clone());
        assert!(is_canonical, "first request must be canonical");
        assert!(state.by_dedup_key.contains_key(&key));
        assert!(state.by_request_id.contains_key(&request_id));
    }

    #[test]
    fn register_or_attach_returns_existing_request_id_for_duplicate() {
        let mut state = PermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".into(), "{}".into());
        let (first_id, _rx1, first_canonical) = register(&mut state, key.clone());
        let (second_id, _rx2, second_canonical) = register(&mut state, key.clone());
        assert!(first_canonical);
        assert!(!second_canonical, "duplicate must not be canonical");
        assert_eq!(
            first_id, second_id,
            "duplicate must reuse the canonical request_id"
        );
    }

    #[test]
    fn register_or_attach_stores_tool_name_and_input_on_canonical_entry() {
        let mut state = PermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Skill".into(), "{\"skill\":\"x:y\"}".into());
        let (request_id, _rx, _) = state.register_or_attach(
            key,
            Uuid::nil(),
            "Skill".into(),
            serde_json::json!({"skill": "x:y"}),
            GrantOffer::default(),
        );
        let entry = state.take(&request_id).unwrap();
        assert_eq!(entry.tool_name, "Skill");
        assert_eq!(entry.input, serde_json::json!({"skill": "x:y"}));
    }

    #[tokio::test]
    async fn duplicate_subscribers_both_receive_the_answer() {
        let mut state = PermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".into(), "{}".into());
        let (id, mut rx1, _) = register(&mut state, key.clone());
        let (_, mut rx2, _) = register(&mut state, key.clone());

        let entry = state.take(&id).expect("entry must be present");
        let _ = entry.tx.send(PermissionAnswer::Allowed);

        assert_eq!(rx1.recv().await.unwrap(), PermissionAnswer::Allowed);
        assert_eq!(rx2.recv().await.unwrap(), PermissionAnswer::Allowed);
    }

    fn entry(req_id: &str) -> PermissionEntry {
        let (tx, _rx) = tokio::sync::broadcast::channel(1);
        PermissionEntry {
            grant_offer: Default::default(),
            thread_id: Uuid::nil(),
            request_id: req_id.to_string(),
            tool_name: "Edit".to_string(),
            input: serde_json::json!({}),
            tx,
        }
    }

    #[test]
    fn take_returns_none_for_unknown_request_id() {
        let mut state = PermissionState::default();
        assert!(state.take("missing").is_none());
    }

    #[test]
    fn take_removes_from_both_indexes() {
        let mut state = PermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".to_string(), "{}".to_string());
        state.by_dedup_key.insert(key.clone(), entry("req-1"));
        state.by_request_id.insert("req-1".to_string(), key.clone());

        let taken = state.take("req-1").expect("entry must be returned");
        assert_eq!(taken.request_id, "req-1");
        assert!(state.by_dedup_key.is_empty());
        assert!(state.by_request_id.is_empty());
    }

    async fn seed_cc_thread(bus: &crate::engine::event_bus::EventBus, thread_id: Uuid) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionStarted {
                sandbox: None,
                coding_agent: crate::runtime::CodingAgent::Codex,
                session_id: "sid-test".into(),
                branch: "claude-code/test".into(),
                repo_id: None,
                coding_agent_kind: Default::default(),
                coding_agent_folder: String::new(),
                app_id: None,
                integrated: false,
                auto_compact_window: None,
                auto_compact_arm: None,
                continuity: None,
            },
            meta: EventMeta {
                channel: Some(crate::engine::thread_events::EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .expect("SessionStarted emit")
        .expect("SessionStarted persisted");
    }

    async fn insert_origin_event(
        pool: &sqlx::PgPool,
        thread_id: Uuid,
        event_type: &str,
        origin: &MessageOrigin,
    ) {
        let payload = serde_json::json!({ "origin": origin });
        sqlx::query(
            "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
             VALUES ($1, 'thread', $2, $3, $4, NOW(), $2::uuid)",
        )
        .bind(Uuid::new_v4())
        .bind(thread_id.to_string())
        .bind(event_type)
        .bind(payload)
        .execute(pool)
        .await
        .expect("insert origin event");
    }

    fn device(device_id: &str) -> MessageOrigin {
        MessageOrigin::Device {
            device_id: device_id.into(),
        }
    }

    fn scheduler_origin(trigger_id: &str) -> MessageOrigin {
        MessageOrigin::engine(EngineReason::Scheduler {
            trigger_id: trigger_id.to_string(),
            trigger_name: None,
            actor_email: None,
        })
    }

    #[test]
    fn a_card_stamps_its_grant_offer_and_an_old_card_still_loads() {
        let offer =
            |tool: &str, input: serde_json::Value| grant_offer(tool, &input, &TrustedHeads::NONE);
        let codex =
            serde_json::json!({ "command": "/bin/zsh -lc 'cat x; git push'", "cwd": "/ws" });
        assert_eq!(
            offer(CODEX_COMMAND_TOOL, codex),
            GrantOffer {
                deny: Some("Bash(git:*)".into()),
                broad: Some("Bash".into()),
                narrow: Some("Bash(git:*)".into()),
                session: Some("command_execution(git:*)".into()),
                withheld: None,
            }
        );
        assert_eq!(
            offer(
                CLAUDE_CODE_COMMAND_TOOL,
                serde_json::json!({ "command": "git push" })
            ),
            GrantOffer {
                deny: Some("Bash(git:*)".into()),
                broad: Some("Bash".into()),
                narrow: Some("Bash(git:*)".into()),
                session: Some("Bash(git:*)".into()),
                withheld: None,
            }
        );
        let two = serde_json::json!({ "command": "git push && curl -X POST https://example.com" });
        assert_eq!(
            offer(CLAUDE_CODE_COMMAND_TOOL, two),
            GrantOffer {
                deny: Some("Bash(git:*)".into()),
                broad: Some("Bash".into()),
                ..GrantOffer::default()
            }
        );
        let script = serde_json::json!({ "command": "/bin/zsh -lc ./scripts/x.sh", "cwd": "/ws" });
        assert_eq!(
            offer(CODEX_COMMAND_TOOL, script),
            GrantOffer {
                broad: Some("Bash".into()),
                ..GrantOffer::default()
            }
        );
        let edit = serde_json::json!({ "file_path": "/tmp/x.md" });
        assert_eq!(
            offer("Edit", edit),
            GrantOffer {
                session: Some("Edit(/tmp/x.md)".into()),
                ..GrantOffer::default()
            }
        );

        let old = serde_json::json!({
            "type": "CodingAgentPermissionRequest",
            "request_id": "r", "tool_use_id": "t", "tool_name": "Bash",
            "input": { "command": "ls" }, "summary": "s", "grant_head": "ls",
        });
        let event: ThreadEvent = serde_json::from_value(old).unwrap();
        assert!(matches!(
            event,
            ThreadEvent::CodingAgentPermissionRequest {
                grant_offer: None,
                ..
            }
        ));
        assert!(serde_json::to_value(&event)
            .unwrap()
            .get("grant_offer")
            .is_none());
    }

    #[test]
    fn an_offer_pattern_carrying_a_secret_is_not_stamped() {
        let token = format!("ghp_{}", "a".repeat(36));
        let edit = serde_json::json!({ "file_path": format!("/tmp/{token}/x.md") });
        let offer = without_secret_patterns(grant_offer("Edit", &edit, &TrustedHeads::NONE));
        assert_eq!(offer, GrantOffer::default());
        let plain = serde_json::json!({ "file_path": "/tmp/x.md" });
        let offer = without_secret_patterns(grant_offer("Edit", &plain, &TrustedHeads::NONE));
        assert_eq!(offer.session.as_deref(), Some("Edit(/tmp/x.md)"));
    }

    #[test]
    fn every_stamped_pattern_covers_its_own_request() {
        use crate::engine::thread_events::AllowScope;
        let trusted =
            TrustedHeads::new([("./scripts/x.sh".to_string(), "scripts/x.sh".to_string())]);
        let codex =
            |c: &str| serde_json::json!({ "command": format!("/bin/zsh -lc '{c}'"), "cwd": "/ws" });
        let claude = |c: &str| serde_json::json!({ "command": c });
        let corpus = [
            (CODEX_COMMAND_TOOL, codex("git status")),
            (
                CODEX_COMMAND_TOOL,
                codex("cat data/pid; ps -p 1; gh run rerun 4"),
            ),
            (CODEX_COMMAND_TOOL, codex("./scripts/x.sh --check")),
            (CODEX_COMMAND_TOOL, codex("./scripts/harden-gates.sh")),
            (CODEX_COMMAND_TOOL, codex("git status && ./scripts/x.sh")),
            (CODEX_COMMAND_TOOL, codex("echo x > .git/hooks/pre-commit")),
            (
                CODEX_COMMAND_TOOL,
                codex("timeout 5 curl https://example.com"),
            ),
            (CODEX_COMMAND_TOOL, codex("/usr/bin/git push")),
            (CLAUDE_CODE_COMMAND_TOOL, claude("git push origin main")),
            (
                CLAUDE_CODE_COMMAND_TOOL,
                claude("cat .claude/settings.json"),
            ),
            (CLAUDE_CODE_COMMAND_TOOL, claude("./scripts/x.sh")),
            (CLAUDE_CODE_COMMAND_TOOL, claude("cd sub && ls")),
            (
                "Skill",
                serde_json::json!({ "skill": "code-review:code-review" }),
            ),
            ("Edit", serde_json::json!({ "file_path": "/tmp/x.md" })),
            ("Read", serde_json::json!({ "file_path": "/tmp/x.md" })),
            (
                "file_change",
                serde_json::json!({ "changes": [{ "path": "/tmp/x" }] }),
            ),
        ];
        let mut stamped = 0;
        for (tool, input) in &corpus {
            let offer = grant_offer(tool, input, &trusted);
            for scope in [AllowScope::Broad, AllowScope::Narrow, AllowScope::Session] {
                let Some(pattern) = offer.pattern(scope) else {
                    continue;
                };
                stamped += 1;
                let stored = |p: &str| p == pattern;
                let covered = match scope {
                    AllowScope::Session => session_allow_covers(tool, input, &trusted, stored),
                    _ => persisted_allow_covers(tool, input, &trusted, stored),
                };
                assert!(covered, "{tool} {input} {scope:?} stamped dead {pattern}");
            }
        }
        assert!(stamped > corpus.len(), "the corpus stamps buttons at all");
        let offer = grant_offer(CODEX_COMMAND_TOOL, &corpus[2].1, &trusted);
        assert_eq!(offer.narrow.as_deref(), Some("Bash(./scripts/x.sh:*)"));
    }

    #[test]
    fn unwrap_shell_command_cases() {
        assert_eq!(
            unwrap_shell_command("/bin/zsh -lc 'curl -X POST https://x -d @y'"),
            "curl -X POST https://x -d @y"
        );
        assert_eq!(unwrap_shell_command("bash -c \"rm -rf /\""), "rm -rf /");
        assert_eq!(
            unwrap_shell_command("bash -c 'rm -rf /' ignored"),
            "rm -rf /"
        );
        assert_eq!(
            unwrap_shell_command("/bin/zsh -lc 'curl -X POST https://x' zsh arg1"),
            "curl -X POST https://x"
        );
        assert_eq!(unwrap_shell_command("bash -c 'rm -rf"), "rm -rf");
        assert_eq!(
            unwrap_shell_command("curl -X POST https://x"),
            "curl -X POST https://x"
        );
        assert_eq!(unwrap_shell_command("zsh script.sh"), "zsh script.sh");
    }

    async fn escalation(tool_name: &str, input: serde_json::Value) -> bool {
        command_gate(tool_name, input, false).await
    }

    async fn command_gate(tool_name: &str, input: serde_json::Value, auto_answer: bool) -> bool {
        guard_allows_command(
            None,
            tool_name,
            &input,
            Path::new("/ws"),
            None,
            Uuid::new_v4(),
            auto_answer,
            &TrustedHeads::NONE,
        )
        .await
    }

    #[tokio::test]
    async fn an_overwrite_from_a_cwd_outside_the_worktree_still_cards() {
        for cmd in ["echo x > .zshrc", "rm -rf old"] {
            let outside = serde_json::json!({ "command": cmd, "cwd": "/Users/me" });
            assert!(!escalation("command_execution", outside).await, "{cmd}");
        }
        let read = serde_json::json!({ "command": "cat .zshrc", "cwd": "/Users/me" });
        assert!(escalation("command_execution", read).await);
    }

    fn zsh(script: &str) -> serde_json::Value {
        serde_json::json!({ "command": format!("/bin/zsh -lc '{script}'") })
    }

    #[test]
    fn only_the_safe_lane_skips_the_card_with_the_flag_off() {
        assert!(lane_allows_escalation(Some(RiskLane::Safe), false));
        assert!(
            !lane_allows_escalation(Some(RiskLane::ReversibleDanger), false),
            "this gate has no safety ref, so in-workspace destruction still asks"
        );
        assert!(!lane_allows_escalation(
            Some(RiskLane::IrreversibleDanger),
            false
        ));
        assert!(!lane_allows_escalation(Some(RiskLane::Catastrophic), false));
        assert!(
            !lane_allows_escalation(None, false),
            "nothing classified it, so the user does"
        );
    }

    #[test]
    fn the_flag_adds_the_reversible_lane_and_nothing_else() {
        assert!(lane_allows_escalation(Some(RiskLane::Safe), true));
        assert!(lane_allows_escalation(
            Some(RiskLane::ReversibleDanger),
            true
        ));
        assert!(!lane_allows_escalation(
            Some(RiskLane::IrreversibleDanger),
            true
        ));
        assert!(!lane_allows_escalation(Some(RiskLane::Catastrophic), true));
        assert!(!lane_allows_escalation(None, true));
    }

    #[tokio::test]
    async fn a_process_probe_needs_no_card() {
        for script in [
            "ps aux | grep python | grep -v grep",
            "cat /tmp/run.log",
            "sleep 20; wc -c /tmp/run.log",
        ] {
            assert!(
                escalation("command_execution", zsh(script)).await,
                "read-only heads are on the guard's static allowlist: {script}"
            );
        }
    }

    #[tokio::test]
    async fn a_catastrophic_command_still_cards() {
        assert!(!escalation("command_execution", zsh("rm -rf /")).await);
    }

    #[tokio::test]
    async fn an_unreadable_command_still_cards() {
        for input in [
            serde_json::json!({}),
            serde_json::json!({ "command": "" }),
            serde_json::json!({ "command": 7 }),
        ] {
            assert!(
                !escalation("command_execution", input).await,
                "nothing can be classified, so nothing is auto-allowed"
            );
        }
    }

    #[tokio::test]
    async fn a_refused_shape_never_reaches_the_judge() {
        for script in [
            "PATH=bin ls",
            "./scripts/deploy.sh",
            "echo hi >> /etc/hosts",
            "cat $(whoami)",
        ] {
            let input = zsh(script);
            let refused = matches!(
                static_command_verdict("command_execution", &input, &TrustedHeads::NONE),
                CommandStatic::Verdict(StaticVerdict::NeedsJudge(ref ji)) if ji.fast_path_refused
            );
            assert!(refused, "fixture must be a fast-path refusal: {script}");
            assert!(
                !escalation("command_execution", input).await,
                "a refusal is never waved through: {script}"
            );
        }
    }

    #[test]
    fn a_scratch_handoff_through_a_literal_heredoc_reaches_the_judge() {
        let command = "/bin/zsh -lc \"cat > /private/tmp/handoff.md <<'HANDOFF'\n\
            # Outcomes\n\n\
            Phase 3 is committed as \"'`feat: add x`. Run `make lint` > log next.'\"\n\
            HANDOFF\n\
            lucidos successor start --narrative /private/tmp/handoff.md --at phase-commit --progress 4/7\"";
        let input = serde_json::json!({ "command": command, "cwd": "/ws" });
        assert!(matches!(
            static_command_verdict(CODEX_COMMAND_TOOL, &input, &TrustedHeads::NONE),
            CommandStatic::Verdict(StaticVerdict::NeedsJudge(ji)) if !ji.fast_path_refused
        ));
    }

    #[tokio::test]
    async fn a_privileged_command_still_cards() {
        for script in [
            "sudo cat /etc/shadow",
            "doas ls /root",
            "ls && sudo cat /etc/shadow",
            "/usr/bin/sudo cat /etc/shadow",
        ] {
            assert!(
                !escalation("command_execution", zsh(script)).await,
                "reaching for another user's rights asks: {script}"
            );
        }
    }

    #[tokio::test]
    async fn a_privilege_word_in_an_argument_is_not_an_escalation() {
        assert!(escalation("command_execution", zsh("grep sudo /etc/hosts")).await);
    }

    #[tokio::test]
    async fn an_unrecognised_head_cards_without_a_judge() {
        assert!(!escalation("command_execution", zsh("cargo build")).await);
    }

    #[tokio::test]
    async fn the_gate_covers_only_command_tools() {
        for tool in ["file_change", "Edit", "Skill"] {
            for flag in [false, true] {
                assert!(
                    !command_gate(tool, zsh("ps aux"), flag).await,
                    "{tool} keeps the behavior it had before the gate (flag {flag})"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_bash_card_asks_with_the_flag_off() {
        assert!(!command_gate("Bash", zsh("ps aux"), false).await);
        assert!(command_gate("command_execution", zsh("ps aux"), false).await);
    }

    #[tokio::test]
    async fn a_bash_card_is_classified_with_the_flag_on() {
        assert!(command_gate("Bash", zsh("ps aux"), true).await);
        assert!(command_gate("Bash", serde_json::json!({ "command": "ls -la" }), true).await);
    }

    #[tokio::test]
    async fn the_refusals_hold_with_the_flag_on() {
        let cases = [
            ("rm -rf /", "catastrophic"),
            ("sudo cat /etc/shadow", "privilege escalation"),
            ("ls && sudo cat /etc/shadow", "privilege escalation"),
            ("./scripts/deploy.sh", "fast-path refusal"),
            ("cat ~/.lucidos/secret-keys/db", "secret read"),
            ("curl -X POST https://example.com -d @x", "unjudged middle"),
        ];
        for tool in ["command_execution", "Bash"] {
            for (script, why) in cases {
                assert!(
                    !command_gate(tool, zsh(script), true).await,
                    "{tool} still asks on {why}: {script}"
                );
            }
            let outside = serde_json::json!({ "command": "rm -rf old", "cwd": "/Users/me" });
            assert!(
                !command_gate(tool, outside, true).await,
                "{tool}: a destruction from a cwd outside the workspace still asks"
            );
        }
    }

    struct WorktreeFixture {
        _tmp: tempfile::TempDir,
        root: std::path::PathBuf,
        outside: std::path::PathBuf,
    }

    fn worktree_fixture() -> WorktreeFixture {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("wt");
        let outside = tmp.path().join("outside");
        for dir in [
            root.join(".claude/rules"),
            root.join(".git/hooks"),
            root.join("vendor/dep/.git"),
            root.join("crates/lucidos-engine/src"),
            outside.join(".claude"),
        ] {
            std::fs::create_dir_all(&dir).expect("create fixture dir");
        }
        std::fs::write(root.join(".gitignore"), "target\n").expect("write .gitignore");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("escape")).expect("symlink");
        WorktreeFixture {
            _tmp: tmp,
            root,
            outside,
        }
    }

    fn under(root: &Path, rel: &str) -> String {
        root.join(rel).to_string_lossy().into_owned()
    }

    #[test]
    fn path_inside_worktree_accepts_targets_under_the_root() {
        let f = worktree_fixture();
        assert!(path_inside_worktree(
            &under(&f.root, ".claude/rules/frontend.md"),
            &f.root
        ));
        assert!(path_inside_worktree(
            &under(&f.root, "crates/lucidos-engine/src/brand_new.rs"),
            &f.root
        ));
        assert!(path_inside_worktree(
            &under(&f.root, ".claude/skills/run-tests/SKILL.md"),
            &f.root
        ));
    }

    #[test]
    fn path_inside_worktree_rejects_targets_outside_the_root() {
        let f = worktree_fixture();
        assert!(!path_inside_worktree(
            &under(&f.outside, ".claude/settings.json"),
            &f.root
        ));
        assert!(!path_inside_worktree("/etc/cron.d/evil", &f.root));
    }

    #[cfg(unix)]
    #[test]
    fn path_inside_worktree_rejects_symlink_escapes() {
        let f = worktree_fixture();
        assert!(!path_inside_worktree(
            &under(&f.root, "escape/.claude/settings.json"),
            &f.root
        ));
        assert!(!path_inside_worktree(
            &under(&f.root, "escape/.claude/brand_new.json"),
            &f.root
        ));
    }

    #[cfg(unix)]
    #[test]
    fn path_inside_worktree_rejects_symlink_into_git_metadata() {
        let f = worktree_fixture();
        std::os::unix::fs::symlink(f.root.join(".git"), f.root.join("gitlink"))
            .expect("symlink into .git");
        assert!(!path_inside_worktree(
            &under(&f.root, "gitlink/hooks/pre-commit"),
            &f.root
        ));
    }

    #[test]
    fn path_inside_worktree_rejects_prefix_sibling_of_the_root() {
        let f = worktree_fixture();
        let sibling = f.root.with_file_name("wt-sibling");
        std::fs::create_dir_all(sibling.join("src")).expect("create sibling");
        assert!(!path_inside_worktree(
            &sibling.join("src/main.rs").to_string_lossy(),
            &f.root
        ));
    }

    #[test]
    fn path_inside_worktree_rejects_parent_dir_escapes() {
        let f = worktree_fixture();
        assert!(!path_inside_worktree("../../etc/cron.d/evil", &f.root));
        assert!(!path_inside_worktree(
            &under(&f.root, "../outside/.claude/settings.json"),
            &f.root
        ));
    }

    #[test]
    fn path_inside_worktree_rejects_relative_paths() {
        let f = worktree_fixture();
        assert!(!path_inside_worktree(".claude/rules/frontend.md", &f.root));
    }

    #[test]
    fn path_inside_worktree_rejects_git_metadata_at_any_depth() {
        let f = worktree_fixture();
        assert!(!path_inside_worktree(
            &under(&f.root, ".git/hooks/pre-commit"),
            &f.root
        ));
        assert!(!path_inside_worktree(
            &under(&f.root, "vendor/dep/.git/config"),
            &f.root
        ));
        assert!(path_inside_worktree(&under(&f.root, ".gitignore"), &f.root));
    }

    #[test]
    fn path_inside_worktree_fails_closed_on_an_unresolvable_root() {
        let f = worktree_fixture();
        let gone = f.root.join("never-existed");
        assert!(!path_inside_worktree(
            &under(&f.root, ".claude/rules/frontend.md"),
            &gone
        ));
    }

    #[test]
    fn worktree_write_auto_allowed_covers_the_file_write_tools() {
        let f = worktree_fixture();
        for tool in ["Edit", "Write", "MultiEdit", "NotebookEdit"] {
            let key = if tool == "NotebookEdit" {
                "notebook_path"
            } else {
                "file_path"
            };
            let input = serde_json::json!({ key: under(&f.root, ".claude/rules/db.md") });
            assert!(
                worktree_write_auto_allowed(tool, &input, Some(&f.root)),
                "{tool} on an in-worktree .claude/ path must skip the card"
            );
        }
    }

    #[test]
    fn worktree_write_auto_allowed_ignores_commands() {
        let f = worktree_fixture();
        for tool in ["Bash", "command_execution"] {
            let input = serde_json::json!({
                "command": format!("rm -rf {}", under(&f.root, ".claude/rules"))
            });
            assert!(
                !worktree_write_auto_allowed(tool, &input, Some(&f.root)),
                "{tool} must never take the worktree fast path"
            );
        }
    }

    #[test]
    fn worktree_write_auto_allowed_fails_closed_without_a_known_worktree() {
        let input = serde_json::json!({ "file_path": "/anything/x.md" });
        assert!(
            !worktree_write_auto_allowed("Edit", &input, None),
            "an unknown worktree root must render the card"
        );
    }

    #[test]
    fn worktree_write_auto_allowed_rejects_out_of_worktree_and_git_targets() {
        let f = worktree_fixture();
        assert!(!worktree_write_auto_allowed(
            "Edit",
            &serde_json::json!({ "file_path": under(&f.outside, ".claude/settings.json") }),
            Some(&f.root)
        ));
        assert!(!worktree_write_auto_allowed(
            "Write",
            &serde_json::json!({ "file_path": under(&f.root, ".git/hooks/pre-commit") }),
            Some(&f.root)
        ));
    }

    fn file_change_input(paths: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "item_id": "exec-1",
            "changes": paths.iter()
                .map(|p| serde_json::json!({ "path": p, "kind": {"type": "add"} }))
                .collect::<Vec<_>>(),
        })
    }

    #[test]
    fn worktree_write_auto_allowed_requires_every_codex_target_inside() {
        let f = worktree_fixture();
        let inside_a = under(&f.root, "src/a.rs");
        let inside_b = under(&f.root, ".claude/rules/db.md");
        let outside = under(&f.outside, "notes.txt");
        assert!(worktree_write_auto_allowed(
            "file_change",
            &file_change_input(&[&inside_a, &inside_b]),
            Some(&f.root)
        ));
        assert!(
            !worktree_write_auto_allowed(
                "file_change",
                &file_change_input(&[&inside_a, &outside]),
                Some(&f.root)
            ),
            "one unplaceable path in the set is enough to ask"
        );
    }

    fn partly_readable_change_set(readable: &str) -> serde_json::Value {
        serde_json::json!({
            "item_id": "exec-1",
            "changes": [
                { "path": readable, "kind": {"type": "add"} },
                { "path": "", "kind": {"type": "add"} },
            ],
        })
    }

    #[test]
    fn one_unreadable_change_entry_makes_the_whole_set_unresolved() {
        let f = worktree_fixture();
        let inside = under(&f.root, "src/a.rs");
        assert!(
            !worktree_write_auto_allowed(
                "file_change",
                &partly_readable_change_set(&inside),
                Some(&f.root)
            ),
            "a half-understood patch must still render a card"
        );
        assert_eq!(
            build_permission_summary("file_change", &partly_readable_change_set("/ws/src/a.rs")),
            "file_change",
            "naming only the half that parsed would read as a complete card"
        );
    }

    #[test]
    fn a_stray_changes_key_never_speaks_for_a_claude_code_file_write() {
        let f = worktree_fixture();
        let input = serde_json::json!({
            "file_path": under(&f.outside, "hosts"),
            "changes": [{ "path": under(&f.root, "ok.txt") }],
        });
        assert!(
            !worktree_write_auto_allowed("Write", &input, Some(&f.root)),
            "the card must be decided by file_path, not by a stray changes key"
        );
    }

    #[test]
    fn a_change_set_of_an_unknown_shape_never_reaches_the_worktree_fast_path() {
        let f = worktree_fixture();
        let inside =
            serde_json::json!({ "changes": "surprise", "grant_root": under(&f.root, "sub") });
        assert!(!worktree_write_auto_allowed(
            "file_change",
            &inside,
            Some(&f.root)
        ));
        let input = serde_json::json!({ "changes": "surprise", "grant_root": "/ws/sub" });
        assert_eq!(
            build_permission_summary("file_change", &input),
            "file_change /ws/sub"
        );
    }

    #[test]
    fn worktree_write_auto_allowed_rejects_an_empty_target_set() {
        let f = worktree_fixture();
        for input in [
            serde_json::json!({ "item_id": "exec-1" }),
            serde_json::json!({ "item_id": "exec-1", "changes": [] }),
        ] {
            assert!(!worktree_write_auto_allowed(
                "file_change",
                &input,
                Some(&f.root)
            ));
        }
    }

    #[test]
    fn path_outside_workspace_cases() {
        let ws = Path::new("/ws");
        assert!(!path_outside_workspace("/ws/data/x", ws));
        assert!(!path_outside_workspace("relative/path", ws));
        assert!(path_outside_workspace("/etc/passwd", ws));
        assert!(path_outside_workspace("/ws/../etc", ws));
        assert!(path_outside_workspace("../../etc/cron.d/evil", ws));
    }

    #[test]
    #[cfg(unix)]
    fn path_outside_workspace_rejects_a_symlink_escape() {
        let f = worktree_fixture();
        assert!(path_outside_workspace(
            &under(&f.root, "escape/.claude/settings.json"),
            &f.root
        ));
        assert!(path_outside_workspace(
            &under(&f.root, "escape/not-created-yet.txt"),
            &f.root
        ));
        assert!(path_outside_workspace(
            &f.outside.join("x").to_string_lossy(),
            &f.root
        ));
        assert!(!path_outside_workspace(
            &under(&f.root, ".claude/rules/frontend.md"),
            &f.root
        ));
        assert!(!path_outside_workspace(
            &under(&f.root, "crates/lucidos-engine/src/brand_new.rs"),
            &f.root
        ));
    }

    #[test]
    #[cfg(unix)]
    fn a_dangling_symlink_is_never_read_as_contained() {
        let f = worktree_fixture();
        std::os::unix::fs::symlink(f.outside.join("not-yet.plist"), f.root.join("dangling"))
            .expect("symlink");
        let link = under(&f.root, "dangling");
        assert!(!path_inside_worktree(&link, &f.root));
        assert!(path_outside_workspace(&link, &f.root));
        assert_eq!(
            canonical_existing_prefix(Path::new(&link)),
            ExistingPrefix::DanglingLink
        );
    }

    fn secret_read_requests() -> Vec<(&'static str, serde_json::Value)> {
        use serde_json::json;
        vec![
            (
                "Bash",
                json!({ "command": "cat ~/.lucidos/secret-keys/db" }),
            ),
            (
                "command_execution",
                json!({ "command": "/bin/zsh -lc 'cat $HOME/.lucidos/pg-shared-password'" }),
            ),
            (
                "Read",
                json!({ "file_path": "/Users/me/.lucidos/secret-keys/db" }),
            ),
            (
                "Grep",
                json!({ "pattern": "x", "path": "/Users/me/.lucidos/secret-keys" }),
            ),
            (
                "Glob",
                json!({ "pattern": "secret-keys/*", "path": "/Users/me/.lucidos" }),
            ),
            (
                "Glob",
                json!({ "pattern": "/home/u/.lucidos/pg-shared-password" }),
            ),
            (
                "Bash",
                json!({ "command": r#"L=~/.lucidos/gateway/gateway.log; grep -a "\[Voice\]" $L | tail -3"# }),
            ),
            (
                "Read",
                json!({ "file_path": "/Users/me/.lucidos/gateway/gateway.log" }),
            ),
            (
                "Grep",
                json!({ "pattern": "Voice", "path": "/Users/me/.lucidos/gateway" }),
            ),
        ]
    }

    #[test]
    fn a_secret_read_is_offered_no_grant() {
        for (tool, input) in secret_read_requests() {
            let offer = grant_offer(tool, &input, &TrustedHeads::NONE);
            assert_eq!(
                offer.withheld,
                Some(OfferWithheld::SecretRead),
                "{tool}: {input}"
            );
            assert!(offer.session.is_none() && offer.narrow.is_none() && offer.broad.is_none());
        }
    }

    #[test]
    fn a_literal_narrative_naming_a_secret_path_is_offered_a_grant() {
        let command = "cat > /tmp/h.md <<'EOF'\n\
                       - Do NOT read `~/.lucidos/gateway/gateway.log` or other raw logs.\n\
                       EOF\n\
                       lucidos successor start --narrative /tmp/h.md --at phase-commit --progress 1/7";
        let offer = grant_offer(
            "Bash",
            &serde_json::json!({ "command": command }),
            &TrustedHeads::NONE,
        );
        assert_ne!(
            (offer.broad, offer.narrow, offer.session),
            (None, None, None)
        );
    }

    #[test]
    fn a_permission_event_stores_its_input_masked() {
        let input = serde_json::json!({
            "command": "curl -H 'Authorization: Bearer ghp_Ab3dEf6hIj9kLm2nOp5qRs8tUv1wXy4zAb7d' \
                        && psql postgres://lucidos:hunter2@localhost/db",
        });
        assert_eq!(
            permission_event_input(&input),
            serde_json::json!({
                "command": "curl -H 'Authorization: Bearer ghp_[REDACTED]' \
                            && psql postgres://lucidos:***@localhost/db",
            })
        );
    }

    #[test]
    fn a_permission_event_masks_a_token_used_as_a_key() {
        let token = "ghp_Ab3dEf6hIj9kLm2nOp5qRs8tUv1wXy4zAb7d";
        let input = serde_json::json!({ token: 1, "headers": [{ token: "x" }] });
        assert_eq!(
            permission_event_input(&input),
            serde_json::json!({ "ghp_[REDACTED]": 1, "headers": [{ "ghp_[REDACTED]": "x" }] })
        );
    }

    #[test]
    fn no_stored_grant_covers_a_secret_read_in_the_interactive_lane() {
        let all = |_: &str| true;
        for (tool, input) in secret_read_requests() {
            assert!(
                !session_allow_covers(tool, &input, &TrustedHeads::NONE, all),
                "{tool}: {input}"
            );
            assert!(
                !persisted_allow_covers(tool, &input, &TrustedHeads::NONE, all),
                "{tool}: {input}"
            );
        }
    }

    #[test]
    fn a_bare_bash_grant_covers_a_read_through_sed_a_loop_or_a_substitution() {
        let bare_bash = |p: &str| p == CLAUDE_CODE_COMMAND_TOOL;
        for command in [
            "rg -n x .claude/rules | sed \"s|a||\"",
            "R=/repo; rg -n x .claude/rules | sed \"s|$R/||\"",
            "for f in a .claude/rules/x.md:84; do ln=${f##*:}; line=$(sed -n \"${ln}p\" \"$f\"); done",
        ] {
            let input = serde_json::json!({ "command": command });
            assert!(
                persisted_allow_covers(
                    CLAUDE_CODE_COMMAND_TOOL,
                    &input,
                    &TrustedHeads::NONE,
                    bare_bash
                ),
                "{command}"
            );
        }
    }

    #[tokio::test]
    async fn secret_command_cards_only_without_a_complete_executing_boundary() {
        use crate::runtime::agent_sandbox::SandboxMode;
        for mode in [
            SandboxMode::Enforced,
            SandboxMode::Inherited,
            SandboxMode::OwnerOptedOut,
            SandboxMode::PermittedOutside,
        ] {
            for truncation in [false, true] {
                let paths = command_guard::SecretPaths::for_policy_with_truncation(
                    &lucidos_agent_sandbox::Policy::default(),
                    Some(mode),
                    truncation,
                );
                for tool in [CODEX_COMMAND_TOOL, CLAUDE_CODE_COMMAND_TOOL] {
                    let input = serde_json::json!({"command": "cat ~/.lucidos/pg-shared-password"});
                    assert_eq!(
                        guard_allows_command_with_paths(
                            None,
                            tool,
                            &input,
                            Path::new("/workspace"),
                            Some(Path::new("/workspace")),
                            Uuid::new_v4(),
                            true,
                            &TrustedHeads::NONE,
                            &paths
                        )
                        .await,
                        mode == SandboxMode::Enforced && truncation,
                        "{tool}: {mode:?}, truncation={truncation}"
                    );
                }
                assert!(
                    !guard_allows_command_with_paths(
                        None,
                        CLAUDE_CODE_COMMAND_TOOL,
                        &serde_json::json!({"command": "cat ~/.lucidos/pg-shared-password"}),
                        Path::new("/workspace"),
                        Some(Path::new("/workspace")),
                        Uuid::new_v4(),
                        false,
                        &TrustedHeads::NONE,
                        &paths
                    )
                    .await,
                    "auto-answer off: {mode:?}, truncation={truncation}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_secret_read_escalation_still_cards() {
        let input =
            serde_json::json!({ "command": "/bin/zsh -lc 'cat ~/.lucidos/secret-keys/db'" });
        for flag in [false, true] {
            assert!(
                !guard_allows_command(
                    None,
                    CODEX_COMMAND_TOOL,
                    &input,
                    Path::new("/ws"),
                    None,
                    Uuid::new_v4(),
                    flag,
                    &TrustedHeads::NONE
                )
                .await
            );
        }
    }

    #[test]
    fn a_neighbouring_file_read_is_not_a_secret_read() {
        use serde_json::json;
        for (tool, input) in [
            ("Read", json!({ "file_path": "/Users/me/.lucidos/ports" })),
            (
                "Grep",
                json!({ "pattern": "secret-keys", "path": "/ws/docs" }),
            ),
            (
                "Glob",
                json!({ "pattern": "**/*.rs", "path": "/Users/me/.lucidos" }),
            ),
        ] {
            assert!(!file_tool_reads_secret(tool, &input), "{tool}: {input}");
        }
    }

    #[test]
    fn a_trusted_repo_script_is_judged_and_granted_only_first() {
        let trusted =
            TrustedHeads::new([("./scripts/x.sh".to_string(), "scripts/x.sh".to_string())]);
        let codex = |script: &str| serde_json::json!({ "command": format!("/bin/zsh -lc '{script}'"), "cwd": "/ws" });
        let late = codex("git status && ./scripts/x.sh");
        assert!(matches!(
            static_command_verdict(CODEX_COMMAND_TOOL, &late, &trusted),
            CommandStatic::Verdict(StaticVerdict::NeedsJudge(ji)) if !ji.fast_path_refused
        ));
        let all = |p: &str| p.starts_with("command_execution(") || p.starts_with("Bash(");
        assert!(!session_allow_covers(
            CODEX_COMMAND_TOOL,
            &late,
            &trusted,
            all
        ));
        assert!(!persisted_allow_covers(
            CODEX_COMMAND_TOOL,
            &late,
            &trusted,
            all
        ));

        let first = codex("./scripts/x.sh --check");
        let path_line = |p: &str| p == "Bash(./scripts/x.sh:*)";
        assert!(persisted_allow_covers(
            CODEX_COMMAND_TOOL,
            &first,
            &trusted,
            path_line
        ));
        assert!(session_allow_covers(
            CODEX_COMMAND_TOOL,
            &first,
            &trusted,
            |p| { p == "command_execution(./scripts/x.sh:*)" }
        ));
        assert!(!persisted_allow_covers(
            CODEX_COMMAND_TOOL,
            &first,
            &TrustedHeads::NONE,
            path_line
        ));
        assert!(!persisted_allow_covers(
            CODEX_COMMAND_TOOL,
            &first,
            &trusted,
            |p| { p == "Bash(x.sh:*)" }
        ));
    }

    #[test]
    fn a_bash_grant_covers_a_protected_path_mention() {
        let bash = |p: &str| p == "Bash";
        for command in [
            "N=\"${TMPDIR:-/tmp}/handoff.md\"; cat > \"$N\" <<'EOF'\nSee `.claude/rules/dev-runtime.md`.\nEOF",
            "find . -name '*.rs' -not -path '*/.git/*' | head",
        ] {
            let input = serde_json::json!({ "command": command });
            assert!(
                persisted_allow_covers("Bash", &input, &TrustedHeads::NONE, bash),
                "{command}"
            );
            assert!(
                persisted_allow_covers(CODEX_COMMAND_TOOL, &input, &TrustedHeads::NONE, bash),
                "{command}"
            );
            assert_eq!(
                grant_offer("Bash", &input, &TrustedHeads::NONE).broad.as_deref(),
                Some("Bash"),
                "{command}"
            );
        }
    }

    #[test]
    fn only_a_bare_bash_grant_covers_a_protected_path_write() {
        let granted = |set: &'static [&'static str]| move |p: &str| set.contains(&p);
        let cmd = |c: &str| serde_json::json!({ "command": c });
        for c in [
            "for f in .claude/x; do awk '{print}' \"$f\"; done",
            "echo x > .claude/y",
            "cat x > .git/hooks/pre-commit",
        ] {
            for tool in [CLAUDE_CODE_COMMAND_TOOL, CODEX_COMMAND_TOOL] {
                assert!(
                    persisted_allow_covers(tool, &cmd(c), &TrustedHeads::NONE, granted(&["Bash"])),
                    "bare Bash must cover {c} for {tool}"
                );
            }
            let offer = grant_offer("Bash", &cmd(c), &TrustedHeads::NONE);
            assert_eq!(offer.broad.as_deref(), Some("Bash"), "{c}");
            assert_eq!(offer.narrow, None, "{c}");
            assert_eq!(offer.session, None, "{c}");
        }
        let hook = cmd("cat x > .git/hooks/pre-commit");
        assert!(!persisted_allow_covers(
            "Bash",
            &hook,
            &TrustedHeads::NONE,
            granted(&["Bash(cat:*)"])
        ));
        assert!(!persisted_allow_covers(
            CODEX_COMMAND_TOOL,
            &hook,
            &TrustedHeads::NONE,
            granted(&["Bash(cat:*)"])
        ));
        assert!(!session_allow_covers(
            "Bash",
            &hook,
            &TrustedHeads::NONE,
            granted(&["Bash(cat:*)"])
        ));
        assert!(!session_allow_covers(
            CODEX_COMMAND_TOOL,
            &hook,
            &TrustedHeads::NONE,
            granted(&["command_execution(cat:*)"])
        ));
    }

    #[test]
    fn a_codex_session_grant_covers_one_head_not_the_whole_thread() {
        let granted = |set: &'static [&'static str]| move |p: &str| set.contains(&p);
        let req = |cmd: &str| serde_json::json!({ "command": cmd });
        assert!(session_allow_covers(
            "command_execution",
            &req("/bin/zsh -lc 'git log --oneline'"),
            &TrustedHeads::NONE,
            granted(&["command_execution(git:*)"])
        ));
        assert!(!session_allow_covers(
            "command_execution",
            &req("/bin/zsh -lc 'rm -rf /'"),
            &TrustedHeads::NONE,
            granted(&["command_execution(git:*)"])
        ));
        assert!(!session_allow_covers(
            "command_execution",
            &req("/bin/zsh -lc 'git status && rm -rf /'"),
            &TrustedHeads::NONE,
            granted(&["command_execution(git:*)"])
        ));
        assert!(!session_allow_covers(
            "Bash",
            &req("LD_PRELOAD=/tmp/evil.so ls"),
            &TrustedHeads::NONE,
            granted(&["Bash(ls:*)"])
        ));
        assert!(session_allow_covers(
            "Edit",
            &serde_json::json!({ "file_path": "/tmp/foo.md" }),
            &TrustedHeads::NONE,
            granted(&["Edit(/tmp/foo.md)"])
        ));
    }

    #[test]
    fn a_persisted_grant_covers_the_patterns_its_click_stored() {
        let granted = |set: &'static [&'static str]| move |p: &str| set.contains(&p);
        let cmd = |c: &str| serde_json::json!({ "command": c });

        assert!(persisted_allow_covers(
            "Bash",
            &cmd("cargo test"),
            &TrustedHeads::NONE,
            granted(&["Bash"])
        ));
        assert!(persisted_allow_covers(
            "Bash",
            &cmd("git status"),
            &TrustedHeads::NONE,
            granted(&["Bash(git:*)"])
        ));
        assert!(!persisted_allow_covers(
            "Bash",
            &cmd("rm -rf /"),
            &TrustedHeads::NONE,
            granted(&["Bash(git:*)"])
        ));
        assert!(persisted_allow_covers(
            "Read",
            &serde_json::json!({ "file_path": "/tmp/foo.md" }),
            &TrustedHeads::NONE,
            granted(&["Read"])
        ));
        let skill = serde_json::json!({ "skill": "code-review:code-review" });
        assert!(persisted_allow_covers(
            "Skill",
            &skill,
            &TrustedHeads::NONE,
            granted(&["Skill"])
        ));
        assert!(persisted_allow_covers(
            "Skill",
            &skill,
            &TrustedHeads::NONE,
            granted(&["Skill(code-review:*)"])
        ));
        assert!(!persisted_allow_covers(
            "Read",
            &skill,
            &TrustedHeads::NONE,
            granted(&[])
        ));
    }

    #[test]
    fn a_persisted_grant_never_reaches_what_claude_code_ignores() {
        let granted = |set: &'static [&'static str]| move |p: &str| set.contains(&p);
        let cmd = |c: &str| serde_json::json!({ "command": c });

        for tool in ["Edit", "Write", "NotebookEdit", "ExitPlanMode"] {
            assert!(
                !persisted_allow_covers(
                    tool,
                    &serde_json::json!({ "file_path": "/tmp/foo.md" }),
                    &TrustedHeads::NONE,
                    granted(&["Edit", "Write", "NotebookEdit", "ExitPlanMode"])
                ),
                "a bare {tool} line must not cover its own request"
            );
        }
        for c in ["rm -rf .git/hooks", "echo '{}' > .claude/settings.json"] {
            assert!(
                !persisted_allow_covers(
                    "Bash",
                    &cmd(c),
                    &TrustedHeads::NONE,
                    granted(&["Bash(rm:*)", "Bash(echo:*)"])
                ),
                "a protected-path command must still ask: {c}"
            );
        }
        assert!(!persisted_allow_covers(
            "command_execution",
            &cmd("/bin/zsh -lc 'git status'"),
            &TrustedHeads::NONE,
            granted(&["command_execution", "command_execution(git:*)"])
        ));
        assert!(!persisted_allow_covers(
            "file_change",
            &serde_json::json!({ "changes": [{ "path": "/tmp/x" }] }),
            &TrustedHeads::NONE,
            granted(&["file_change"])
        ));
    }

    #[test]
    fn a_persisted_command_grant_covers_every_segment_or_none() {
        let granted = |set: &'static [&'static str]| move |p: &str| set.contains(&p);
        let cmd = |c: &str| serde_json::json!({ "command": c });

        assert!(!persisted_allow_covers(
            "Bash",
            &cmd("git status && rm -rf /"),
            &TrustedHeads::NONE,
            granted(&["Bash(git:*)"])
        ));
        assert!(!persisted_allow_covers(
            "Bash",
            &cmd("LD_PRELOAD=/tmp/evil.so ls"),
            &TrustedHeads::NONE,
            granted(&["Bash(ls:*)"])
        ));
        assert!(!persisted_allow_covers(
            "Bash",
            &serde_json::json!({}),
            &TrustedHeads::NONE,
            granted(&["Bash(ls:*)"])
        ));
    }

    #[test]
    fn a_persisted_bash_grant_covers_a_codex_command_segment_by_segment() {
        let granted = |set: &'static [&'static str]| move |p: &str| set.contains(&p);
        let codex = |c: &str| serde_json::json!({ "command": c, "cwd": "/ws" });
        let tool = CODEX_COMMAND_TOOL;

        assert!(persisted_allow_covers(
            tool,
            &codex("/bin/zsh -lc 'git status'"),
            &TrustedHeads::NONE,
            granted(&["Bash(git:*)"])
        ));
        assert!(!persisted_allow_covers(
            tool,
            &codex("/bin/zsh -lc 'git status && rm -rf x'"),
            &TrustedHeads::NONE,
            granted(&["Bash(git:*)"])
        ));
        assert!(!persisted_allow_covers(
            tool,
            &codex("/bin/zsh -lc 'rm -rf /'"),
            &TrustedHeads::NONE,
            granted(&["Bash(rm:*)"])
        ));
        assert!(!persisted_allow_covers(
            tool,
            &codex("/bin/zsh -lc 'echo x > .git/hooks/pre-commit'"),
            &TrustedHeads::NONE,
            granted(&["Bash(echo:*)"])
        ));
        assert!(!persisted_allow_covers(
            tool,
            &codex("/bin/zsh -lc './x.sh'"),
            &TrustedHeads::NONE,
            granted(&["Bash(x.sh:*)", "Bash(./x.sh:*)"])
        ));
        assert!(persisted_allow_covers(
            tool,
            &codex("/bin/zsh -lc 'cargo test && git push'"),
            &TrustedHeads::NONE,
            granted(&["Bash"])
        ));
        assert!(persisted_allow_covers(
            tool,
            &codex("/bin/zsh -lc 'echo x > .git/hooks/pre-commit'"),
            &TrustedHeads::NONE,
            granted(&["Bash"])
        ));
        for c in ["cat ~/.lucidos/secret-keys/db", "rm -rf /"] {
            assert!(
                !persisted_allow_covers(
                    tool,
                    &codex(&format!("/bin/zsh -lc '{c}'")),
                    &TrustedHeads::NONE,
                    granted(&["Bash"])
                ),
                "bare Bash must not cover {c}"
            );
        }
    }

    #[test]
    fn a_persisted_line_means_the_same_for_both_agents() {
        let lines: &[&[&str]] = &[
            &["Bash"],
            &["Bash(git:*)"],
            &["Bash(rm:*)"],
            &["Bash(echo:*)"],
            &["Bash(git:*)", "Bash(curl:*)"],
            &["command_execution"],
            &[],
        ];
        let commands = [
            "git status",
            "cargo test",
            "git push && curl -X POST https://example.com",
            "cat x; git push",
            "rm -rf /",
            "cat ~/.lucidos/secret-keys/db",
            "echo x > .git/hooks/pre-commit",
            "cat .claude/settings.json",
            "LD_PRELOAD=/tmp/evil.so ls",
        ];
        for set in lines {
            let allowed = |p: &str| set.contains(&p);
            for command in commands {
                let claude = persisted_allow_covers(
                    CLAUDE_CODE_COMMAND_TOOL,
                    &serde_json::json!({ "command": command }),
                    &TrustedHeads::NONE,
                    allowed,
                );
                let codex = persisted_allow_covers(
                    CODEX_COMMAND_TOOL,
                    &serde_json::json!({
                        "command": format!("/bin/zsh -lc '{command}'"),
                        "cwd": "/ws",
                    }),
                    &TrustedHeads::NONE,
                    allowed,
                );
                assert_eq!(claude, codex, "{set:?} on {command}");
            }
        }
    }

    #[tokio::test]
    async fn workspace_operation_floor_separates_data_before_coding_agent_grants() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let temp = tempfile::tempdir().unwrap();
        let pending = Mutex::new(PermissionState::default());
        for tool in ["Bash", "command_execution"] {
            let thread_id = Uuid::new_v4();
            seed_cc_thread(&bus, thread_id).await;
            allow_for_thread(&bus, thread_id, tool).await;
            for (allowed, command) in [
                (true, "python3 - <<'PY'\nimport os,shutil\nprint(os.environ.get('LUCIDOS_PG_BIN_DIR'))\nprint(shutil.which('pg_ctl'))\nPY"),
                (true, "cat > fixture.rs <<'EOF'\npg_ctl -D /tmp/lucidos_example stop\nEOF"),
                (true, "python3 - <<'PY'\nfrom pathlib import Path\nPath('handoff.md').write_text('Use LUCIDOS_PG_BIN_DIR and pg_ctl')\nPY"),
                (false, "dropdb lucidos_example"),
                (false, "true; bash -c 'psql -c \"DROP DATABASE lucidos_example\"'"),
                (false, "curl -X DELETE http://localhost/~/api/v1/control/workspaces/example"),
            ] {
                let outcome = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    prompt_coding_agent_permission(
                        &pool, &bus, &pending, temp.path(), None, None,
                        CodingAgentPermissionInput {
                            thread_id,
                            tool_use_id: Uuid::new_v4().to_string(),
                            tool_name: tool.into(),
                            input: serde_json::json!({"command": command, "cwd": temp.path()}),
                        },
                    ),
                ).await.expect("a granted or catastrophic command never waits for a card");
                assert_eq!(outcome.allowed, allowed, "{tool}: {command}: {:?}", outcome.reason);
                if allowed {
                    assert!(matches!(
                        outcome.reason.as_deref(),
                        Some(SESSION_ALLOW_REASON | COMMAND_GUARD_ALLOW_REASON)
                    ), "{tool}: {command}: {:?}", outcome.reason);
                }
            }
        }
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn an_unproven_floor_match_cards_past_every_grant() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let temp = tempfile::tempdir().unwrap();
        let command =
            "cat > fixture.rs <<'EOF'\npg_ctl -D /tmp/lucidos_example stop\nEOF\ncargo fmt --all";
        for tool in ["Bash", "command_execution"] {
            let pending = Arc::new(Mutex::new(PermissionState::default()));
            let thread_id = Uuid::new_v4();
            seed_cc_thread(&bus, thread_id).await;
            allow_for_thread(&bus, thread_id, tool).await;
            let waiter = tokio::spawn({
                let (pool, bus, pending) = (pool.clone(), bus.clone(), pending.clone());
                let cwd = temp.path().to_path_buf();
                async move {
                    prompt_coding_agent_permission(
                        &pool,
                        &bus,
                        &pending,
                        &cwd,
                        None,
                        None,
                        CodingAgentPermissionInput {
                            thread_id,
                            tool_use_id: tool.to_string(),
                            tool_name: tool.to_string(),
                            input: serde_json::json!({"command": command, "cwd": cwd}),
                        },
                    )
                    .await
                }
            });
            let request_id = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                await_canonical_request(&pending),
            )
            .await
            .expect("an unproven floor match raises a card");
            let offer = {
                let state = pending.lock().unwrap();
                state.by_dedup_key[&state.by_request_id[&request_id]]
                    .grant_offer
                    .clone()
            };
            assert_eq!(
                offer,
                GrantOffer::withheld(OfferWithheld::UnprovenWorkspaceDestruction),
                "{tool}"
            );
            waiter.abort();
        }
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_duplicate_hears_a_refusal_when_the_waiting_canonical_caller_goes_away() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::UserQuestionAsked {
                tool_use_id: "open-card".into(),
                cc_session_id: String::new(),
                question: "Holds the slot?".into(),
                options: vec![],
                worktree_path: None,
                multi_select: false,
                owner_approval: None,
                permission_request: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();

        let input = serde_json::json!({"command": "echo duplicate"});
        let canonical = tokio::spawn({
            let (pool, bus, pending, input) =
                (pool.clone(), bus.clone(), pending.clone(), input.clone());
            async move {
                park_card(
                    &pool,
                    &bus,
                    &pending,
                    thread_id,
                    "first".into(),
                    "Bash".into(),
                    &input,
                    GrantOffer::default(),
                )
                .await
            }
        });
        while pending.lock().unwrap().by_request_id.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let mut duplicate = park_card(
            &pool,
            &bus,
            &pending,
            thread_id,
            "second".into(),
            "Bash".into(),
            &input,
            GrantOffer::default(),
        )
        .await;
        canonical.abort();
        let _ = canonical.await;

        let answer = tokio::time::timeout(std::time::Duration::from_secs(5), duplicate.recv())
            .await
            .expect("the duplicate is not left waiting");
        assert!(
            matches!(answer, Ok(PermissionAnswer::Refused(_))),
            "{answer:?}"
        );
        assert!(pending.lock().unwrap().by_request_id.is_empty());

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn cleanup_floor_precedes_broad_session_grants_on_coding_agent_gate() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("protected");
        std::fs::create_dir_all(root.join(".lucidos")).unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        let pending = Mutex::new(PermissionState::default());
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        allow_for_thread(&bus, thread_id, "Bash").await;
        for tool in ["Bash", "command_execution"] {
            let outcome = prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                temp.path(),
                None,
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: tool.to_string(),
                    tool_name: tool.to_string(),
                    input: serde_json::json!({"command": "rm -rf protected", "cwd": temp.path()}),
                },
            )
            .await;
            assert!(!outcome.allowed, "{tool}");
        }
        let worktree = temp.path().join("own-worktree");
        let gitdir = temp.path().join("git-admin");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(gitdir.join("commondir"), "..").unwrap();
        std::fs::create_dir_all(worktree.join(".lucidos")).unwrap();
        std::fs::create_dir_all(worktree.join("data")).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}", gitdir.display()),
        )
        .unwrap();
        let outcome = prompt_coding_agent_permission(&pool, &bus, &pending,
            temp.path(), Some(&worktree), None, CodingAgentPermissionInput { thread_id,
                tool_use_id: "own-worktree".into(), tool_name: "Bash".into(),
                input: serde_json::json!({"command": format!("rm -rf {}", worktree.display()), "cwd": worktree})
            }).await;
        assert!(outcome.allowed, "the session's own worktree is exempt");
        assert!(root.exists());
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_standing_deny_beats_a_standing_allow_on_the_coding_agent_lane() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        let workspace = tempfile::TempDir::new().unwrap();
        let by = tool_grants::ChangedBy {
            source: crate::core::grants::GrantSource::Card,
            thread_id: Some(thread_id),
            actor: None,
        };
        tool_grants::change_standing(
            &bus,
            GrantFile::CodingAgentTools,
            tool_grants::StandingChange::Add(&["Bash(curl:*)".to_string()]),
            by,
        )
        .await
        .unwrap();
        let ask = |tool: &'static str| {
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                workspace.path(),
                None,
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: tool.into(),
                    tool_name: tool.into(),
                    input: serde_json::json!({"command": "curl https://example.com"}),
                },
            )
        };
        assert!(ask("Bash").await.allowed, "the allow covers it first");

        standing_deny::record(
            &bus,
            thread_id,
            "Bash(curl:*)".into(),
            AnsweredBy::tap(None),
        )
        .await
        .unwrap();
        for tool in ["Bash", "command_execution"] {
            let outcome = ask(tool).await;
            assert!(!outcome.allowed, "{tool}");
            let reason = outcome.reason.unwrap_or_default();
            assert!(reason.contains("standing deny"), "{tool}: {reason}");
            assert!(reason.contains("`Bash(curl:*)`"), "{tool}: {reason}");
        }
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[test]
    fn an_unavailable_standing_deny_check_uses_the_permission_card() {
        crate::test_support::run_on_engine_runtime(
            "engine::cc_permission::tests::an_unavailable_standing_deny_check_uses_the_permission_card",
            standing_deny_fallback_case,
        );
    }

    async fn standing_deny_fallback_case() {
        use crate::engine::agent_host::NoAgentHost;
        use crate::engine::thread_events::AllowScope;
        use crate::test_support::{boot_mock_engine_with_host, setup_test_db, teardown_test_db};

        let (pool, db) = setup_test_db().await;
        let thread_id = Uuid::new_v4();
        let host = Arc::new(NoAgentHost {
            held_message_release_thread: Some(thread_id),
        });
        let (engine, workspace) = boot_mock_engine_with_host(&db, host).await;
        let bus = &engine.event_bus;
        let pending = &engine.pending_cc_permission;
        seed_cc_thread(bus, thread_id).await;
        allow_for_thread(bus, thread_id, "Bash").await;
        let ask = |tool_name: &'static str, tool_use_id: String, thread_id| {
            prompt_coding_agent_permission(
                &pool,
                bus,
                pending,
                workspace.path(),
                None,
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id,
                    tool_name: tool_name.into(),
                    input: serde_json::json!({"command": "git status"}),
                },
            )
        };
        assert!(ask("Bash", "granted".into(), thread_id).await.allowed);

        let unavailable = |is_unavailable| {
            let pool = &pool;
            async move {
                let ddl = if is_unavailable {
                    "ALTER TABLE permissions RENAME TO unavailable_permissions"
                } else {
                    "ALTER TABLE unavailable_permissions RENAME TO permissions"
                };
                sqlx::query(ddl).execute(pool).await.unwrap();
            }
        };
        unavailable(true).await;
        for (index, (tool, approved, scope, deny_on_answer, expected)) in [
            (CLAUDE_CODE_COMMAND_TOOL, false, None, false, false),
            (CODEX_COMMAND_TOOL, true, None, false, true),
            (
                CLAUDE_CODE_COMMAND_TOOL,
                true,
                Some(AllowScope::Session),
                false,
                false,
            ),
            (CODEX_COMMAND_TOOL, true, None, true, false),
        ]
        .into_iter()
        .enumerate()
        {
            let cards_before = card_count(&pool, thread_id).await;
            let decide = async {
                let request_id = await_canonical_request(pending).await;
                while card_count(&pool, thread_id).await == cards_before {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                {
                    let state = pending.lock().unwrap();
                    let key = &state.by_request_id[&request_id];
                    assert_eq!(
                        state.by_dedup_key[key].grant_offer,
                        GrantOffer::withheld(OfferWithheld::StandingDenyUnavailable),
                    );
                }
                if deny_on_answer {
                    unavailable(false).await;
                    standing_deny::record(
                        bus,
                        thread_id,
                        "Bash(git:*)".into(),
                        AnsweredBy::tap(Some(device("owner"))),
                    )
                    .await
                    .unwrap();
                }
                assert!(resolve_coding_agent_permission(
                    &engine,
                    request_id,
                    approved,
                    scope,
                    false,
                    AnsweredBy::tap(Some(device("owner"))),
                    "[test] fallback decision",
                )
                .await
                .unwrap());
            };
            let (outcome, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                tokio::join!(ask(tool, format!("fallback-{index}"), thread_id), decide)
            })
            .await
            .expect("fallback card and decision");
            assert_eq!(outcome.allowed, expected);
            if deny_on_answer {
                assert!(outcome.reason.unwrap().contains("Bash(git:*)"));
            }
        }

        let mut ordinary = park_card(
            &pool,
            bus,
            pending,
            thread_id,
            "ordinary".into(),
            "Bash".into(),
            &serde_json::json!({"command": "echo ready"}),
            GrantOffer::default(),
        )
        .await;
        let request_id = await_canonical_request(pending).await;
        unavailable(true).await;
        assert!(resolve_coding_agent_permission(
            &engine,
            request_id,
            true,
            None,
            false,
            AnsweredBy::tap(Some(device("owner"))),
            "[test] ordinary decision",
        )
        .await
        .unwrap());
        let outcome = outcome_from_permission_recv(ordinary.recv().await);
        assert!(!outcome.allowed);
        assert!(outcome
            .reason
            .unwrap()
            .contains("standing denies could not be read"));

        drop(engine);
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[test]
    fn an_always_allow_answer_reaches_the_waiter_after_the_grant_is_stored() {
        crate::test_support::run_on_engine_runtime(
            "engine::cc_permission::tests::an_always_allow_answer_reaches_the_waiter_after_the_grant_is_stored",
            always_allow_stores_before_answering_case,
        );
    }

    async fn always_allow_stores_before_answering_case() {
        use crate::engine::agent_host::NoAgentHost;
        use crate::engine::thread_events::AllowScope;
        use crate::test_support::{boot_mock_engine_with_host, setup_test_db, teardown_test_db};

        let (pool, db) = setup_test_db().await;
        let thread_id = Uuid::new_v4();
        let host = Arc::new(NoAgentHost {
            held_message_release_thread: Some(thread_id),
        });
        let (engine, _workspace) = boot_mock_engine_with_host(&db, host).await;
        let bus = &engine.event_bus;
        let pending = &engine.pending_cc_permission;
        seed_cc_thread(bus, thread_id).await;
        let mut waiter = park_card(
            &pool,
            bus,
            pending,
            thread_id,
            "always".into(),
            "Skill".into(),
            &serde_json::json!({"skill": "demo-plugin:demo"}),
            GrantOffer {
                narrow: Some("Skill(demo-plugin:*)".into()),
                ..GrantOffer::default()
            },
        )
        .await;
        let request_id = await_canonical_request(pending).await;

        let held = tool_grants::STANDING_LOCK.lock().await;
        {
            let resolve = resolve_coding_agent_permission(
                &engine,
                request_id,
                true,
                Some(AllowScope::Narrow),
                false,
                AnsweredBy::tap(Some(device("owner"))),
                "[test] always allow",
            );
            tokio::pin!(resolve);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(300), &mut resolve)
                    .await
                    .is_err(),
                "the answer must wait for the grant write"
            );
            assert!(
                waiter.try_recv().is_err(),
                "the waiter was answered before the grant was stored"
            );
            drop(held);
            assert!(resolve.await.unwrap());
        }
        assert!(outcome_from_permission_recv(waiter.recv().await).allowed);
        let stored = tool_grants::standing_lines(&pool, GrantFile::CodingAgentTools)
            .await
            .unwrap();
        assert!(
            stored.contains(&"Skill(demo-plugin:*)".to_string()),
            "{stored:?}"
        );

        drop(engine);
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_classified_escalation_needs_no_card() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                Path::new("/ws"),
                None,
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "i".into(),
                    tool_name: "command_execution".into(),
                    input: zsh("ps aux | grep bench | grep -v grep"),
                },
            ),
        )
        .await
        .expect("a classified escalation must not wait for a card");

        assert!(outcome.allowed);
        assert_eq!(outcome.reason.as_deref(), Some(COMMAND_GUARD_ALLOW_REASON));
        assert_eq!(
            count_permission_events(&pool, thread_id).await,
            (0, 0),
            "an auto-allow renders no card, like every other fast path here"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn auto_answer_defaults_off_and_ignores_the_master_guard() {
        use crate::core::preferences::PreferenceStore;
        use crate::core::prefs::{CODING_AGENT_AUTO_ANSWER, COMMAND_GUARD};
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let set = |key: &'static str, value: &'static str| {
            PreferenceStore::set_row_for_test(&pool, key, value)
        };
        assert!(!coding_agent_auto_answer(&pool).await, "off by default");
        set(COMMAND_GUARD.key(), "true").await.unwrap();
        assert!(
            !coding_agent_auto_answer(&pool).await,
            "the master guard does not turn it on"
        );
        set(COMMAND_GUARD.key(), "false").await.unwrap();
        set(CODING_AGENT_AUTO_ANSWER.key(), "true").await.unwrap();
        assert!(
            coding_agent_auto_answer(&pool).await,
            "on with the master guard off"
        );
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_bash_card_is_classified_only_under_auto_answer() {
        use crate::core::preferences::PreferenceStore;
        use crate::core::prefs::{CODING_AGENT_AUTO_ANSWER, COMMAND_GUARD};
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        let ask = || {
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                Path::new("/ws"),
                None,
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "b".into(),
                    tool_name: "Bash".into(),
                    input: serde_json::json!({ "command": "ps aux | grep bench" }),
                },
            )
        };

        PreferenceStore::set_row_for_test(&pool, CODING_AGENT_AUTO_ANSWER.key(), "true")
            .await
            .unwrap();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), ask())
            .await
            .expect("a classified Bash card must not wait for a card");
        assert!(outcome.allowed, "auto-answer answers with the guard off");
        assert_eq!(outcome.reason.as_deref(), Some(COMMAND_GUARD_ALLOW_REASON));
        assert_eq!(count_permission_events(&pool, thread_id).await, (0, 0));

        PreferenceStore::set_row_for_test(&pool, CODING_AGENT_AUTO_ANSWER.key(), "false")
            .await
            .unwrap();
        PreferenceStore::set_row_for_test(&pool, COMMAND_GUARD.key(), "true")
            .await
            .unwrap();
        let waiting = tokio::time::timeout(std::time::Duration::from_millis(500), ask()).await;
        assert!(
            waiting.is_err(),
            "auto-answer off: the Bash card waits for the user, guard on or not"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_trigger_session_reaches_the_command_gate() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        insert_origin_event(
            &pool,
            thread_id,
            "TriggerStarted",
            &scheduler_origin("trig-order"),
        )
        .await;

        let outcome = prompt_coding_agent_permission(
            &pool,
            &bus,
            &pending,
            Path::new("/ws"),
            None,
            None,
            CodingAgentPermissionInput {
                thread_id,
                tool_use_id: "i".into(),
                tool_name: "command_execution".into(),
                input: zsh("ps aux"),
            },
        )
        .await;

        assert!(outcome.allowed);
        assert_eq!(outcome.reason.as_deref(), Some(COMMAND_GUARD_ALLOW_REASON));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn every_session_uses_the_workspace_grants_without_a_card() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let workspace = workspace_granting(&bus, &["Bash"]).await;

        for origin in [
            device("owner-phone"),
            MessageOrigin::engine(EngineReason::MainCheckRed),
            MessageOrigin::engine(EngineReason::MergeConflict),
            scheduler_origin("trig-granted"),
        ] {
            let thread_id = Uuid::new_v4();
            seed_cc_thread(&bus, thread_id).await;
            insert_origin_event(&pool, thread_id, "MessageReceived", &origin).await;

            let outcome = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                prompt_coding_agent_permission(
                    &pool,
                    &bus,
                    &pending,
                    workspace.path(),
                    None,
                    None,
                    CodingAgentPermissionInput {
                        thread_id,
                        tool_use_id: "landing".to_string(),
                        tool_name: "Bash".to_string(),
                        input: serde_json::json!({
                            "command": "./scripts/harden-gates.sh 2>&1 | tail -30"
                        }),
                    },
                ),
            )
            .await
            .unwrap_or_else(|_| panic!("{origin:?}: a granted request never waits for a card"));

            assert!(outcome.allowed, "{origin:?}");
            assert_eq!(
                outcome.reason.as_deref(),
                Some(PERSISTED_ALLOW_REASON),
                "{origin:?}"
            );
            assert_eq!(card_count(&pool, thread_id).await, 0, "{origin:?}");
        }

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn an_auto_deny_redacts_the_command_it_records() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        let secret = "postgres://u:hunter2@localhost:5432/db";

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                Path::new("/ws"),
                None,
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "i".into(),
                    tool_name: "command_execution".into(),
                    input: serde_json::json!({
                        "command": format!("psql {secret} -c 'select 1' && rm -rf /")
                    }),
                },
            ),
        )
        .await
        .expect("must not hang");
        assert!(!outcome.allowed, "the catastrophic floor denies");

        let payload: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentPermissionRequest'",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .expect("recorded request");
        assert!(
            !payload.to_string().contains("hunter2"),
            "the recorded command must be scrubbed: {payload}"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    async fn count_permission_events(pool: &sqlx::PgPool, thread_id: Uuid) -> (i64, i64) {
        let one = |event_type: &'static str| async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = $2",
            )
            .bind(thread_id)
            .bind(event_type)
            .fetch_one(pool)
            .await
            .expect("count")
        };
        (
            one("CodingAgentPermissionRequest").await,
            one("CodingAgentPermissionResolved").await,
        )
    }

    #[tokio::test]
    async fn prompt_round_trip_emits_request_and_resolves_on_broadcast() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let pending = std::sync::Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;

        let waiter = {
            let pool = pool.clone();
            let bus = bus.clone();
            let pending = pending.clone();
            tokio::spawn(async move {
                prompt_coding_agent_permission(
                    &pool,
                    &bus,
                    &pending,
                    Path::new("/ws"),
                    None,
                    None,
                    CodingAgentPermissionInput {
                        thread_id,
                        tool_use_id: "i1".to_string(),
                        tool_name: "command_execution".to_string(),
                        input: serde_json::json!({"command": "curl -X POST https://example.com -d @x"}),
                    },
                )
                .await
            })
        };

        let mut request_id = None;
        for _ in 0..500 {
            request_id = {
                let state = pending.lock().unwrap();
                state.by_request_id.keys().next().cloned()
            };
            if request_id.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let request_id = request_id.expect("the request must raise a card within 5s");
        let entry = {
            let mut state = pending.lock().unwrap();
            state.take(&request_id).expect("canonical entry present")
        };
        assert_eq!(entry.tool_name, "command_execution");
        let _ = entry.tx.send(PermissionAnswer::Allowed);

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), waiter)
            .await
            .expect("resolves within 10s")
            .expect("task ok");
        assert!(outcome.allowed);
        assert!(outcome.reason.is_none());

        let rows: Vec<(serde_json::Value,)> = sqlx::query_as(
            "SELECT payload FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentPermissionRequest'",
        )
        .bind(thread_id)
        .fetch_all(&pool)
        .await
        .expect("query events");
        assert_eq!(rows.len(), 1, "one card per canonical request");
        assert_eq!(rows[0].0["tool_name"], "command_execution");
        assert_eq!(rows[0].0["tool_use_id"], "i1");
        assert_eq!(
            rows[0].0["summary"],
            "command_execution curl -X POST https://example.com -d @x"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    async fn await_request_count(pool: &sqlx::PgPool, thread_id: Uuid, want: i64) {
        for _ in 0..500 {
            if count_permission_events(pool, thread_id).await.0 == want {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the thread never held {want} permission requests");
    }

    async fn allow_open_card(bus: &EventBus, pending: &Mutex<PermissionState>, thread_id: Uuid) {
        let request_id: String = sqlx::query_scalar(
            "SELECT p.payload->>'request_id' FROM events p \
             WHERE p.thread_id = $1 AND p.event_type = 'CodingAgentPermissionRequest' \
               AND NOT EXISTS (SELECT 1 FROM events r \
                 WHERE r.event_type = 'CodingAgentPermissionResolved' \
                   AND r.payload->>'request_id' = p.payload->>'request_id')",
        )
        .bind(thread_id)
        .fetch_one(bus.pool())
        .await
        .expect("one open card");
        let entry = pending.lock().unwrap().take(&request_id).expect("entry");
        let _ = entry.tx.send(PermissionAnswer::Allowed);
        emit_coding_agent_permission_resolved(
            bus,
            thread_id,
            request_id,
            true,
            None,
            None,
            None,
            EventMeta::NONE,
            "test",
        )
        .await;
    }

    #[tokio::test]
    async fn parallel_prompts_show_one_card_at_a_time() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let pending = std::sync::Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;

        let prompt = |id: &str, command: &str| {
            let (pool, bus, pending) = (pool.clone(), bus.clone(), pending.clone());
            let input = CodingAgentPermissionInput {
                thread_id,
                tool_use_id: id.to_string(),
                tool_name: "command_execution".to_string(),
                input: serde_json::json!({ "command": command }),
            };
            tokio::spawn(async move {
                prompt_coding_agent_permission(
                    &pool,
                    &bus,
                    &pending,
                    Path::new("/ws"),
                    None,
                    None,
                    input,
                )
                .await
            })
        };
        let first = prompt("i1", "curl -X POST https://example.com -d @a");
        let second = prompt("i2", "curl -X POST https://example.com -d @b");

        await_request_count(&pool, thread_id, 1).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(
            count_permission_events(&pool, thread_id).await,
            (1, 0),
            "the second prompt waits while the first card is open"
        );

        allow_open_card(&bus, &pending, thread_id).await;
        await_request_count(&pool, thread_id, 2).await;
        allow_open_card(&bus, &pending, thread_id).await;

        for waiter in [first, second] {
            let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), waiter)
                .await
                .expect("resolves within 10s")
                .expect("task ok");
            assert!(outcome.allowed);
        }
        assert_eq!(count_permission_events(&pool, thread_id).await, (2, 2));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn prompt_skips_card_on_session_allow_match() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let pending = std::sync::Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        let workspace = tempfile::tempdir().expect("tempdir");

        let input = serde_json::json!({"command": "git status"});
        let pattern = grant_offer("Bash", &input, &TrustedHeads::NONE)
            .session
            .expect("bash offers a session pattern");
        allow_for_thread(&bus, thread_id, &pattern).await;

        let outcome = prompt_coding_agent_permission(
            &pool,
            &bus,
            &pending,
            workspace.path(),
            None,
            None,
            CodingAgentPermissionInput {
                thread_id,
                tool_use_id: "i2".to_string(),
                tool_name: "Bash".to_string(),
                input,
            },
        )
        .await;
        assert!(outcome.allowed);
        assert_eq!(outcome.reason.as_deref(), Some(SESSION_ALLOW_REASON));

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentPermissionRequest'",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .expect("count");
        assert_eq!(count, 0, "session-allow must not render a card");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn standing_deny_precedes_coding_agent_grants_and_safe_commands() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        for grants in [&["Bash"][..], &[][..]] {
            let (pool, db_name) = setup_test_db().await;
            crate::core::preferences::PreferenceStore::set_row_for_test(
                &pool,
                crate::core::prefs::CODING_AGENT_AUTO_ANSWER.key(),
                "true",
            )
            .await
            .unwrap();
            let (bus, _rx) = EventBus::new(pool.clone());
            let pending = Arc::new(Mutex::new(PermissionState::default()));
            let workspace = workspace_granting(&bus, grants).await;
            let thread_id = Uuid::new_v4();
            seed_cc_thread(&bus, thread_id).await;
            let ask = |tool_name: &str, tool_use_id: &str| {
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    prompt_coding_agent_permission(
                        &pool,
                        &bus,
                        &pending,
                        workspace.path(),
                        None,
                        None,
                        CodingAgentPermissionInput {
                            thread_id,
                            tool_use_id: tool_use_id.into(),
                            tool_name: tool_name.into(),
                            input: serde_json::json!({"command": "git status"}),
                        },
                    ),
                )
            };
            let tools = [CLAUDE_CODE_COMMAND_TOOL, CODEX_COMMAND_TOOL];
            for tool in tools {
                let before = ask(tool, "before").await.expect("baseline must auto-allow");
                assert!(before.allowed, "{tool} with {grants:?}");
            }
            standing_deny::record(
                &bus,
                thread_id,
                "Bash(git:*)".into(),
                AnsweredBy::tap(Some(device("owner"))),
            )
            .await
            .expect("write standing deny");
            for tool in tools {
                let after = ask(tool, "after")
                    .await
                    .expect("a deny must resolve without a card");
                assert!(!after.allowed, "{tool} with {grants:?}");
                assert!(after.reason.unwrap_or_default().contains("Bash(git:*)"));
            }
            pool.close().await;
            teardown_test_db(&db_name).await;
        }
    }

    async fn workspace_granting(bus: &EventBus, patterns: &[&str]) -> tempfile::TempDir {
        let patterns: Vec<String> = patterns.iter().map(|pattern| pattern.to_string()).collect();
        tool_grants::change_standing(
            bus,
            GrantFile::CodingAgentTools,
            tool_grants::StandingChange::Replace(&patterns),
            tool_grants::ChangedBy {
                source: crate::core::grants::GrantSource::Settings,
                thread_id: None,
                actor: None,
            },
        )
        .await
        .expect("save coding-agent grants");
        tempfile::tempdir().expect("tempdir")
    }

    #[tokio::test]
    async fn prompt_skips_card_on_persisted_allow_match() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let pending = std::sync::Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        let workspace = workspace_granting(&bus, &["Bash"]).await;

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                workspace.path(),
                None,
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "i3".to_string(),
                    tool_name: "Bash".to_string(),
                    input: serde_json::json!({ "command": "cargo test" }),
                },
            ),
        )
        .await
        .expect("a granted request never waits for a card");
        assert!(outcome.allowed);
        assert_eq!(outcome.reason.as_deref(), Some(PERSISTED_ALLOW_REASON));
        assert_eq!(
            card_count(&pool, thread_id).await,
            0,
            "a persisted grant must not render a card"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    async fn card_count(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentPermissionRequest'",
        )
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("count cards")
    }

    #[tokio::test]
    async fn prompt_skips_card_for_in_worktree_write() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        let f = worktree_fixture();

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                Path::new("/ws"),
                Some(&f.root),
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "i-wt".into(),
                    tool_name: "Edit".into(),
                    input: serde_json::json!({
                        "file_path": under(&f.root, ".claude/rules/frontend.md"),
                        "old_string": "x",
                        "new_string": "y"
                    }),
                },
            ),
        )
        .await
        .expect("in-worktree write must not wait for a card");

        assert!(outcome.allowed);
        assert_eq!(outcome.reason.as_deref(), Some(WORKTREE_WRITE_ALLOW_REASON));
        assert_eq!(
            card_count(&pool, thread_id).await,
            0,
            "the worktree fast path must not render a card"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn prompt_renders_card_for_out_of_worktree_write() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;

        let waiter = {
            let (pool, bus, pending) = (pool.clone(), bus.clone(), pending.clone());
            tokio::spawn(async move {
                prompt_coding_agent_permission(
                    &pool,
                    &bus,
                    &pending,
                    Path::new("/ws"),
                    Some(Path::new("/ws/.lucidos/worktrees/thread-abc")),
                    None,
                    CodingAgentPermissionInput {
                        thread_id,
                        tool_use_id: "i-out".into(),
                        tool_name: "Edit".into(),
                        input: serde_json::json!({ "file_path": "/home/u/.claude/settings.json" }),
                    },
                )
                .await
            })
        };

        let request_id = await_canonical_request(&pending).await;
        let entry = pending
            .lock()
            .unwrap()
            .take(&request_id)
            .expect("canonical entry present");
        let _ = entry.tx.send(PermissionAnswer::Allowed);

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), waiter)
            .await
            .expect("resolves within 10s")
            .expect("task ok");
        assert!(outcome.allowed);
        assert_eq!(
            outcome.reason, None,
            "an out-of-worktree write must be answered by the user, not a fast path"
        );
        assert_eq!(card_count(&pool, thread_id).await, 1, "one card rendered");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_withdrawn_card_never_tells_the_agent_the_user_denied() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        for (label, sweep_reason) in [
            ("superseded", SUPERSEDED_REASON),
            ("session ended", SESSION_ENDED_REASON),
        ] {
            let (pool, db_name) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let pending = Arc::new(Mutex::new(PermissionState::default()));
            let thread_id = Uuid::new_v4();
            seed_cc_thread(&bus, thread_id).await;

            let waiter = {
                let (pool, bus, pending) = (pool.clone(), bus.clone(), pending.clone());
                tokio::spawn(async move {
                    prompt_coding_agent_permission(
                        &pool,
                        &bus,
                        &pending,
                        Path::new("/ws"),
                        Some(Path::new("/ws/.lucidos/worktrees/thread-abc")),
                        None,
                        CodingAgentPermissionInput {
                            thread_id,
                            tool_use_id: "i-withdrawn".into(),
                            tool_name: "Write".into(),
                            input: serde_json::json!({ "file_path": "/home/u/notes.md" }),
                        },
                    )
                    .await
                })
            };
            await_canonical_request(&pending).await;
            while card_count(&pool, thread_id).await == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            if sweep_reason == SUPERSEDED_REASON {
                resolve_pending_permissions_as_superseded(&pool, &bus, &pending, thread_id, None)
                    .await;
            } else {
                resolve_pending_permissions_as_session_ended(
                    &pool, &bus, &pending, thread_id, None,
                )
                .await;
            }

            let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), waiter)
                .await
                .expect("resolves within 10s")
                .expect("task ok");
            assert!(!outcome.allowed, "{label}: a withdrawn card allows nothing");
            let reason = outcome.reason.unwrap_or_default();
            assert_ne!(
                reason, DENIAL_REASON,
                "{label}: nobody clicked Deny, so the agent must not read a user denial"
            );
            assert!(
                reason.starts_with(sweep_reason) && reason.contains("not a user decision"),
                "{label}: the agent reads why the card ended: {reason:?}"
            );

            pool.close().await;
            teardown_test_db(&db_name).await;
        }
    }

    async fn await_canonical_request(pending: &Mutex<PermissionState>) -> String {
        loop {
            let id = {
                let state = pending.lock().unwrap();
                state.by_request_id.keys().next().cloned()
            };
            if let Some(id) = id {
                return id;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    async fn allow_for_thread(bus: &EventBus, thread_id: Uuid, pattern: &str) {
        tool_grants::grant_for_thread(
            bus,
            GrantFile::CodingAgentTools,
            thread_id,
            "req-allow",
            pattern.to_string(),
            AnsweredBy::tap(None),
        )
        .await
        .expect("thread grant recorded");
    }

    async fn seed_resolved_permission(
        bus: &EventBus,
        thread_id: Uuid,
        request_id: &str,
        tool_name: &str,
        input: serde_json::Value,
        allowed: bool,
        persist_scope: Option<crate::engine::thread_events::AllowScope>,
    ) {
        for event in [
            ThreadEvent::CodingAgentPermissionRequest {
                request_id: request_id.to_string(),
                tool_use_id: format!("tu-{request_id}"),
                tool_name: tool_name.to_string(),
                input: input.clone(),
                summary: build_permission_summary(tool_name, &input),
                grant_offer: Some(grant_offer(tool_name, &input, &TrustedHeads::NONE)),
            },
            ThreadEvent::CodingAgentPermissionResolved {
                request_id: request_id.to_string(),
                allowed,
                reason: None,
                persist_scope,
                standing_deny: None,
            },
        ] {
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta: EventMeta::NONE,
            })
            .await
            .expect("seed emit")
            .expect("seed persisted");
        }
    }

    #[tokio::test]
    async fn an_old_session_allow_survives_the_upgrade() {
        use crate::engine::thread_events::AllowScope;
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        seed_resolved_permission(
            &bus,
            thread_id,
            "req-granted",
            "Bash",
            serde_json::json!({ "command": "git status" }),
            true,
            Some(AllowScope::Session),
        )
        .await;

        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let workspace = tempfile::tempdir().expect("tempdir");

        tool_grants::import_legacy_files(&bus, &crate::core::grants_dir(workspace.path())).await;
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                workspace.path(),
                None,
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "i-after-restart".into(),
                    tool_name: "Bash".into(),
                    input: serde_json::json!({ "command": "git commit -m wip" }),
                },
            ),
        )
        .await
        .expect("an imported grant must not wait for a card");

        assert!(outcome.allowed);
        assert_eq!(outcome.reason.as_deref(), Some(SESSION_ALLOW_REASON));
        assert_eq!(
            card_count(&pool, thread_id).await,
            1,
            "only the seeded card exists, so no new one was rendered"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn the_import_keeps_the_stamped_session_pattern() {
        use crate::engine::thread_events::AllowScope;
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        let stamped = "command_execution(./scripts/x.sh:*)";
        for event in [
            ThreadEvent::CodingAgentPermissionRequest {
                request_id: "req-path".to_string(),
                tool_use_id: "tu-path".to_string(),
                tool_name: CODEX_COMMAND_TOOL.to_string(),
                input: serde_json::json!({ "command": "/bin/zsh -lc ./scripts/x.sh", "cwd": "/ws" }),
                summary: "command_execution ./scripts/x.sh".to_string(),
                grant_offer: Some(GrantOffer {
                    session: Some(stamped.to_string()),
                    ..GrantOffer::default()
                }),
            },
            ThreadEvent::CodingAgentPermissionResolved {
                request_id: "req-path".to_string(),
                allowed: true,
                reason: None,
                persist_scope: Some(AllowScope::Session),
                standing_deny: None,
            },
        ] {
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta: EventMeta::NONE,
            })
            .await
            .expect("seed emit")
            .expect("seed persisted");
        }

        let workspace = tempfile::tempdir().expect("tempdir");
        let dir = crate::core::grants_dir(workspace.path());
        tool_grants::import_legacy_files(&bus, &dir).await;
        let file = GrantFile::CodingAgentTools;
        let granted = tool_grants::granted_lines(&bus, file, Some(thread_id)).await;
        assert_eq!(granted.thread, vec![stamped.to_string()]);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn allow_once_and_deny_do_not_import() {
        use crate::engine::thread_events::AllowScope;
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        seed_resolved_permission(
            &bus,
            thread_id,
            "req-once",
            "Bash",
            serde_json::json!({ "command": "git status" }),
            true,
            None,
        )
        .await;
        seed_resolved_permission(
            &bus,
            thread_id,
            "req-denied",
            "Bash",
            serde_json::json!({ "command": "git push" }),
            false,
            Some(AllowScope::Session),
        )
        .await;

        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let workspace = tempfile::tempdir().expect("tempdir");
        let dir = crate::core::grants_dir(workspace.path());
        tool_grants::import_legacy_files(&bus, &dir).await;
        let waiter = {
            let (pool, bus, pending) = (pool.clone(), bus.clone(), pending.clone());
            let workspace = workspace.path().to_path_buf();
            tokio::spawn(async move {
                prompt_coding_agent_permission(
                    &pool,
                    &bus,
                    &pending,
                    &workspace,
                    None,
                    None,
                    CodingAgentPermissionInput {
                        thread_id,
                        tool_use_id: "i-again".into(),
                        tool_name: "Bash".into(),
                        input: serde_json::json!({ "command": "git status" }),
                    },
                )
                .await
            })
        };

        let request_id = await_canonical_request(&pending).await;
        let entry = pending
            .lock()
            .unwrap()
            .take(&request_id)
            .expect("a card must be rendered");
        let _ = entry.tx.send(PermissionAnswer::Allowed);

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), waiter)
            .await
            .expect("resolves within 10s")
            .expect("task ok");
        assert_eq!(
            outcome.reason, None,
            "answered by the user, not short-circuited by a bogus import"
        );
        let file = GrantFile::CodingAgentTools;
        let granted = tool_grants::granted_lines(&bus, file, Some(thread_id)).await;
        assert_eq!(
            granted,
            tool_grants::GrantedLines::default(),
            "neither an Allow once nor a Deny may become a grant"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[test]
    fn gc_dead_entries_removes_orphans_with_no_receivers() {
        let mut state = PermissionState::default();
        let live_key: DedupKey = (Uuid::nil(), "Edit".into(), "{\"a\":1}".into());
        let live_entry = entry("req-live");
        let _live_rx = live_entry.tx.subscribe();
        state.by_dedup_key.insert(live_key.clone(), live_entry);
        state
            .by_request_id
            .insert("req-live".into(), live_key.clone());

        let dead_key: DedupKey = (Uuid::nil(), "Edit".into(), "{\"b\":2}".into());
        state
            .by_dedup_key
            .insert(dead_key.clone(), entry("req-dead"));
        state
            .by_request_id
            .insert("req-dead".into(), dead_key.clone());

        state.gc_dead_entries();

        assert!(state.by_dedup_key.contains_key(&live_key), "live kept");
        assert!(state.by_request_id.contains_key("req-live"), "live kept");
        assert!(!state.by_dedup_key.contains_key(&dead_key), "dead swept");
        assert!(!state.by_request_id.contains_key("req-dead"), "dead swept");
    }

    #[tokio::test]
    async fn a_prompt_beside_an_open_question_is_refused() {
        use crate::engine::question_park_sql::OPEN_QUESTION_TOOL_REFUSAL;
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        let emit = |event: ThreadEvent| {
            let bus = bus.clone();
            async move {
                bus.emit(BusEvent::Thread {
                    thread_id,
                    event,
                    meta: EventMeta::NONE,
                })
                .await
                .unwrap();
            }
        };
        emit(ThreadEvent::UserQuestionAsked {
            tool_use_id: "q1".into(),
            cc_session_id: String::new(),
            question: "Which one?".into(),
            options: vec![],
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            permission_request: None,
        })
        .await;
        let pending = Mutex::new(PermissionState::default());
        let workspace = tempfile::tempdir().unwrap();
        let outcome = prompt_coding_agent_permission(
            &pool,
            &bus,
            &pending,
            workspace.path(),
            None,
            None,
            CodingAgentPermissionInput {
                thread_id,
                tool_use_id: "cmd-1".into(),
                tool_name: "Bash".into(),
                input: serde_json::json!({"command": "cargo build"}),
            },
        )
        .await;
        assert!(!outcome.allowed);
        assert_eq!(outcome.reason.as_deref(), Some(OPEN_QUESTION_TOOL_REFUSAL));

        emit(ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: "q1".into(),
            answer: crate::engine::thread_events::AnswerKind::FreeText {
                text: "the first".into(),
                image_hashes: vec![],
            },
            quiet: false,
        })
        .await;
        assert_eq!(open_question_refusal(&pool, thread_id).await, None);
        teardown_test_db(&db).await;
    }
}
