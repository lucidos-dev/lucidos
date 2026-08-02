//! Turn-gap resume note (sibling of `external_edits`).
//!
//! A coding agent's `--resume` replays its own conversation, not what happened
//! to its work while it was idle. So when the user clicks Apply, Discard or
//! Revert, when an Apply attempt fails, or when the cleanup worker reclaims the
//! worktree, the resumed agent is blind to it: it tells the user the change is
//! still awaiting Apply, offers to Apply commits that no longer exist on the
//! branch, or treats reverted work as still in `main`.
//!
//! [`compute_turn_gap_note`] closes that gap with a short, stateless note
//! prepended to the resumed prompt. It surfaces every covered event that landed
//! in the **turn gap**: the window between the agent's *previous* turn boundary
//! and the *current* one, i.e. with a `sequence` strictly between the previous
//! boundary and the current turn's triggering event.
//!
//! - **Upper bound (exclusive):** the current turn's triggering event
//!   (`current_origin_id`), by its **sequence**, not merely its id. The engine
//!   persists the user's `MessageReceived` *before* spawning/resuming the agent
//!   (see `chat/process/run.rs`, "Emit MessageReceived FIRST"), so by the time
//!   this runs the current message is already in the events table. Excluding it
//!   is what makes the threshold the *previous* boundary rather than the
//!   current one. Excluding everything *after* it matters too: when two
//!   messages race a spawn, `cc_spawn_coalesce` runs one turn anchored on the
//!   first while the second is already persisted, and an id-only bound would
//!   let that later message become the "previous" boundary and swallow the
//!   entire gap. An event that lands after the origin is not lost, it simply
//!   belongs to the next turn's gap.
//! - **Lower bound (exclusive):** the most-recent turn-boundary event *before*
//!   the current one. The boundary set is [`boundary_event_types`]: every type
//!   that can ORIGINATE a coding-agent turn (`CC_ORIGINATING_EVENT_TYPES`, the
//!   shared definition, so the two cannot drift) plus `CodingAgentPromptSent`,
//!   an engine-synthesized prompt that is a real turn boundary even though it
//!   is an audit marker rather than an origin id. Hand-listing the boundary set
//!   is what let `TriggerStarted` and `ChildThreadCompleted` fall out of it: a
//!   turn woken by a finished child did not advance the threshold, so the note
//!   fired on that turn and then fired *again* on the next one.
//!
//! This is **stateless and self-clearing**: once the note rides on a turn, that
//! turn's own origin event becomes the *previous* boundary for the next turn,
//! so the same events fall below the threshold and never re-fire. No new event,
//! no projection column, no state to clear: the events table is the cursor.
//!
//! ## What is covered, and what is deliberately not
//!
//! Covered: `ChangeApplied`, `ChangeDiscarded`, `ChangeReverted`,
//! `ChangeApplyFailed`, `WorktreeCleaned`. Those are the persisted events that
//! land in the gap and that the replay cannot show the agent.
//!
//! Deliberately NOT covered, because another mechanism already delivers them:
//! `ChildThreadCompleted` (itself a CC turn origin, it wakes the parent
//! carrying the child's summary), `UserQuestionAnswered` and
//! `CodingAgentPermissionResolved` (delivered in-band to the waiting call),
//! `BackgroundBashCompleted` (`spawn_bash_completion_watcher` pushes its own
//! resume prompt), and everything the agent itself emitted, which the replay
//! already contains.
//!
//! Deliberately NOT covered, because a note would add nothing:
//! `MergeConflictDetected` / `MergeResolutionStarted` / `MergeResolutionCleared`
//! (the conflict session spawns with a purpose-built system prompt naming the
//! conflicted files, strictly more informative), `ChangeHardened` (the agent's
//! own `/harden` run caused it), `CodingAgentSettingsChanged` (the resumed
//! process already runs with the new model / effort), `ThreadTitleRenamed` /
//! `ThreadSaved` / `ThreadUnsaved` / `QueuedMessageRemoved` (cosmetic, or
//! removed before delivery), and `ThreadArchived` / `ThreadDiscarded`
//! (terminal; archive's pending-change discard already arrives as
//! `ChangeDiscarded`).
//!
//! A newly-hidden event is one more arm in [`GapEvent`], not a redesign.
//!
//! Like `external_edits`, the helper is best-effort: any query error degrades
//! silently to `None` (resume proceeds without the note rather than failing),
//! and a payload that will not parse degrades to vaguer wording rather than
//! dropping the event.

use std::collections::HashMap;

use uuid::Uuid;

use super::resume::CC_ORIGINATING_EVENT_TYPES;

/// Max number of bullet lines to render before truncating, so a thread that
/// applied a large batch can't blow up the resumed prompt. Mirrors the
/// `MAX_LINES` convention in `external_edits`.
const MAX_LINES: usize = 50;

/// Event types the note surfaces. See the module docs for the reasoned
/// exclusions.
const COVERED_EVENT_TYPES: &[&str] = &[
    "ChangeApplied",
    "ChangeDiscarded",
    "ChangeReverted",
    "ChangeApplyFailed",
    "WorktreeCleaned",
];

/// A change description is the agent's own summary and can be long; keep the
/// bullet readable.
const MAX_DESCRIPTION_CHARS: usize = 80;

/// Apply errors are user-facing toast copy and can carry a git stderr dump.
const MAX_ERROR_CHARS: usize = 200;

/// The turn-gap note plus the one fact its caller needs back.
pub(crate) struct TurnGapNote {
    /// The rendered note, ready to prepend to the resumed prompt.
    pub note: String,
    /// True when the gap held an event that moved the worktree out from under
    /// the agent: an Apply or a Discard (both reset the worktree to `main`), or
    /// a tier-2 `WorktreeCleaned` (the worktree is removed, and with
    /// `branch_deleted` the next spawn recreates it at `main`).
    ///
    /// `external_edits::compute_external_edit_note` takes this as
    /// `head_move_explained`: the HEAD move it would otherwise report as
    /// "the user edited files … HEAD moved (no log available)" has a known
    /// cause, and this note is the one stating it. A Revert (which runs in the
    /// main repo, not the worktree), an apply failure and a tier-1 clean (build
    /// artifacts only) touch no ref, so they leave the flag alone.
    pub explains_worktree_reset: bool,
}

/// One covered event, parsed out of its payload.
enum GapEvent {
    Applied {
        change_id: String,
        /// Commit subjects merged to `main`, oldest first. Empty for no-op
        /// applies.
        commits: Vec<String>,
        /// SHA of `main` after the merge, already shortened to <= 8 chars.
        short_sha: Option<String>,
    },
    Discarded {
        change_id: String,
    },
    Reverted {
        change_id: String,
    },
    ApplyFailed {
        change_id: String,
        error: String,
    },
    WorktreeCleaned {
        tier: u8,
        freed_bytes: u64,
        branch_deleted: bool,
    },
}

impl GapEvent {
    /// The change this event resolved, when it names one. Drives the single
    /// `changes` lookup that resolves branch names and descriptions.
    fn change_id(&self) -> Option<&str> {
        match self {
            Self::Applied { change_id, .. }
            | Self::Discarded { change_id }
            | Self::Reverted { change_id }
            | Self::ApplyFailed { change_id, .. } => Some(change_id),
            Self::WorktreeCleaned { .. } => None,
        }
    }

    /// Whether this event moved *this session's* worktree. See
    /// [`TurnGapNote::explains_worktree_reset`].
    ///
    /// Apply and Discard reset the worktree of the CHANGE'S branch, which is
    /// not always this session's: the reconcile path
    /// (`discard_pending_for_thread_except`) discards stale siblings on other
    /// branches, and answering "yes, a reset explains the HEAD move" for one of
    /// those would silence a genuine external-edit report about a worktree
    /// nothing touched. An unresolved branch counts as ours, because the note
    /// itself says a reset happened and the two must not disagree.
    fn moved_the_worktree(&self, branch: &str, session_branch: Option<&str>) -> bool {
        match self {
            Self::Applied { .. } | Self::Discarded { .. } => {
                branch.is_empty() || session_branch.is_none_or(|current| current == branch)
            }
            Self::WorktreeCleaned { tier, .. } => *tier >= 2,
            Self::Reverted { .. } | Self::ApplyFailed { .. } => false,
        }
    }
}

/// The branch of the change an event resolved, or `""` when it names no change
/// or the `changes` row could not be resolved.
fn branch_for<'a>(event: &GapEvent, facts: &'a HashMap<Uuid, ChangeFacts>) -> &'a str {
    facts_for(event, facts)
        .map(|f| f.branch_name.as_str())
        .unwrap_or("")
}

fn facts_for<'a>(
    event: &GapEvent,
    facts: &'a HashMap<Uuid, ChangeFacts>,
) -> Option<&'a ChangeFacts> {
    event
        .change_id()
        .and_then(|id| Uuid::parse_str(id).ok())
        .and_then(|id| facts.get(&id))
}

/// What the `changes` projection knows about a change the note mentions.
struct ChangeFacts {
    branch_name: String,
    description: String,
}

/// Turn-boundary event types: everything that can originate a coding-agent turn
/// plus the engine-synthesized prompt marker. Derived from
/// `CC_ORIGINATING_EVENT_TYPES` rather than hand-listed so a new origin type
/// cannot silently reopen the double-fire hole.
fn boundary_event_types() -> Vec<String> {
    CC_ORIGINATING_EVENT_TYPES
        .iter()
        .map(|s| (*s).to_string())
        .chain(std::iter::once("CodingAgentPromptSent".to_string()))
        .collect()
}

/// Build a note telling the resuming agent what happened to its work in the gap
/// before the current turn. Returns `None` when nothing did.
///
/// `current_origin_id` is the event that triggered this turn (already
/// persisted, see the module docs). It is the exclusive upper bound, so the
/// threshold becomes the *previous* turn boundary. `session_branch` is the
/// branch this session is resuming on, used to tell "your branch was reset"
/// apart from "a stale change on another branch was cleaned up".
pub(crate) async fn compute_turn_gap_note(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    current_origin_id: Uuid,
    session_branch: Option<&str>,
) -> Option<TurnGapNote> {
    // The gap's upper bound is the current origin's POSITION, not just its id.
    // Excluding the origin by id alone leaves `MAX(sequence)` free to pick a
    // boundary event persisted AFTER it, which really happens: when two user
    // messages race a spawn, `cc_spawn_coalesce` runs one turn whose origin is
    // the first message while the second is already in the events table. The
    // later message would then become the "previous" boundary and suppress the
    // whole gap. `None` (an origin that isn't persisted, which the module
    // contract says cannot happen) degrades to the id-only bound rather than to
    // an empty window, so an unresolvable origin loses no note.
    let origin_sequence: Option<i64> =
        sqlx::query_scalar::<_, i64>("SELECT sequence FROM events WHERE id = $1")
            .bind(current_origin_id)
            .fetch_optional(pool)
            .await
            .unwrap_or_else(|e| {
                log!(
                    "[AgentSession] Failed to resolve turn-gap origin sequence for {}: {}",
                    thread_id,
                    e
                );
                None
            });

    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as::<_, (String, serde_json::Value)>(
        "SELECT event_type, payload FROM events \
         WHERE thread_id = $1 AND event_type = ANY($2) \
           AND ($3::bigint IS NULL OR sequence < $3) \
           AND sequence > COALESCE(( \
             SELECT MAX(sequence) FROM events \
             WHERE thread_id = $1 \
               AND id <> $4 \
               AND ($3::bigint IS NULL OR sequence < $3) \
               AND event_type = ANY($5) \
           ), 0) \
         ORDER BY sequence ASC",
    )
    .bind(thread_id)
    .bind(COVERED_EVENT_TYPES)
    .bind(origin_sequence)
    .bind(current_origin_id)
    .bind(boundary_event_types())
    .fetch_all(pool)
    .await
    .map_err(|e| {
        log!(
            "[AgentSession] Failed to look up turn-gap events for {}: {}",
            thread_id,
            e
        );
        e
    })
    .ok()?;

    let events: Vec<GapEvent> = rows.into_iter().filter_map(parse_gap_event).collect();

    if events.is_empty() {
        return None;
    }

    let facts = load_change_facts(pool, &events).await;
    let explains_worktree_reset = events
        .iter()
        .any(|e| e.moved_the_worktree(branch_for(e, &facts), session_branch));

    Some(TurnGapNote {
        note: build_note(&events, &facts, session_branch),
        explains_worktree_reset,
    })
}

/// Turn one `(event_type, payload)` row into a [`GapEvent`]. Missing fields
/// degrade to empty rather than dropping the event: knowing a change was
/// discarded matters more than knowing which one.
fn parse_gap_event((event_type, payload): (String, serde_json::Value)) -> Option<GapEvent> {
    let change_id = str_field(&payload, "change_id");
    match event_type.as_str() {
        "ChangeApplied" => Some(GapEvent::Applied {
            change_id,
            commits: payload
                .get("commits")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
            short_sha: payload
                .get("post_merge_sha")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(short),
        }),
        "ChangeDiscarded" => Some(GapEvent::Discarded { change_id }),
        "ChangeReverted" => Some(GapEvent::Reverted { change_id }),
        "ChangeApplyFailed" => Some(GapEvent::ApplyFailed {
            change_id,
            error: truncate(&str_field(&payload, "error"), MAX_ERROR_CHARS),
        }),
        "WorktreeCleaned" => Some(GapEvent::WorktreeCleaned {
            tier: payload
                .get("tier")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                .min(u64::from(u8::MAX)) as u8,
            freed_bytes: payload
                .get("freed_bytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            branch_deleted: payload
                .get("branch_deleted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        }),
        // A covered type the query returned but this match doesn't know would
        // be a programming error; skip it rather than panic mid-resume.
        _ => None,
    }
}

fn str_field(payload: &serde_json::Value, key: &str) -> String {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Resolve `branch_name` + `description` for every change the note mentions, in
/// one query. The `changes` row survives every outcome (only `status` flips),
/// so this works for discarded and reverted changes alike. Ids are parsed in
/// Rust and unparseable ones dropped, so a legacy payload carrying no
/// `change_id` cannot abort the query on a failed cast.
async fn load_change_facts(pool: &sqlx::PgPool, events: &[GapEvent]) -> HashMap<Uuid, ChangeFacts> {
    let ids: Vec<Uuid> = events
        .iter()
        .filter_map(GapEvent::change_id)
        .filter_map(|id| Uuid::parse_str(id).ok())
        .collect();
    if ids.is_empty() {
        return HashMap::new();
    }

    sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT id, branch_name, description FROM changes WHERE id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|e| {
        log!(
            "[AgentSession] Failed to resolve change facts for the turn-gap note: {}",
            e
        );
        e
    })
    .map(|rows| {
        rows.into_iter()
            .map(|(id, branch_name, description)| {
                (
                    id,
                    ChangeFacts {
                        branch_name,
                        description,
                    },
                )
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Shorten to the first 8 characters on a char boundary (never a raw byte
/// slice, see CLAUDE.md "Never slice strings by byte index").
fn short(s: &str) -> String {
    s[..s.floor_char_boundary(8)].to_string()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    format!("{}…", &s[..s.floor_char_boundary(max)])
}

fn build_note(
    events: &[GapEvent],
    facts: &HashMap<Uuid, ChangeFacts>,
    session_branch: Option<&str>,
) -> String {
    let mut note = String::from(
        "[Note from engine: while you were idle, the user or the engine acted on your work. \
         Reconcile what you are about to say with this:",
    );

    let mut lines = 0usize;
    let mut truncated = 0usize;

    for event in events {
        let branch = branch_for(event, facts);
        let label = change_label(event.change_id().unwrap_or(""), facts_for(event, facts));

        for line in render(event, &label, branch, session_branch) {
            if lines < MAX_LINES {
                note.push('\n');
                note.push_str(&line);
                lines += 1;
            } else {
                truncated += 1;
            }
        }
    }

    if truncated > 0 {
        note.push_str(&format!("\n… and {} more", truncated));
    }

    for line in closing_guidance(events) {
        note.push('\n');
        note.push_str(line);
    }
    note.push(']');
    note
}

/// The bullet(s) for one event. Applies render one line per merged commit; every
/// other kind renders exactly one line.
fn render(
    event: &GapEvent,
    label: &str,
    branch: &str,
    session_branch: Option<&str>,
) -> Vec<String> {
    match event {
        GapEvent::Applied {
            change_id,
            commits,
            short_sha,
        } => {
            if commits.is_empty() {
                // No-op apply (nothing to merge / already merged). Render a row
                // anyway so the agent knows the change resolved, rather than an
                // empty bullet.
                return vec![format!(
                    "- APPLIED: change {}: no commits / already merged.",
                    short_change_id(change_id)
                )];
            }
            let sha_suffix = match short_sha {
                Some(s) => format!(" (now in main at {})", s),
                None => " (now in main)".to_string(),
            };
            commits
                .iter()
                .map(|subject| format!("- APPLIED: {}{}", subject, sha_suffix))
                .collect()
        }
        GapEvent::Discarded { .. } => vec![discarded_line(label, branch, session_branch)],
        GapEvent::Reverted { .. } => vec![format!(
            "- REVERTED: {}{} had been applied and has now been undone in main by revert \
             commits. main no longer contains that work; your branch and worktree were not \
             touched by the revert.",
            label,
            on_branch(branch)
        )],
        GapEvent::ApplyFailed { error, .. } => vec![format!(
            "- APPLY FAILED: {}{} is STILL PENDING; the Apply attempt did not land. The engine \
             showed the user this message: \"{}\"",
            label,
            on_branch(branch),
            error
        )],
        GapEvent::WorktreeCleaned {
            tier,
            freed_bytes,
            branch_deleted,
        } => vec![worktree_cleaned_line(*tier, *freed_bytes, *branch_deleted)],
    }
}

/// A Discard resets the change's branch to `main`. Whether that is the agent's
/// own branch is the difference between "your work is gone" and "a stale change
/// of yours was cleaned up" (the `discard_pending_for_thread_except` reconcile
/// path discards siblings on OTHER branches), so say which.
fn discarded_line(label: &str, branch: &str, session_branch: Option<&str>) -> String {
    match (branch.is_empty(), session_branch) {
        (false, Some(current)) if branch != current => format!(
            "- DISCARDED: {} on branch `{}`, which is NOT your current branch `{}`. That branch \
             was reset to main; your current work is untouched.",
            label, branch, current
        ),
        (false, _) => format!(
            "- DISCARDED: {} on your branch `{}`. Discard reset that branch to main and cleaned \
             the worktree, so those commits are gone. It is NOT pending: do not tell the user it \
             is awaiting Apply, and do not offer to Apply it.",
            label, branch
        ),
        (true, _) => format!(
            "- DISCARDED: {}. Discard reset its branch to main and cleaned the worktree, so those \
             commits are gone. It is NOT pending: do not tell the user it is awaiting Apply, and \
             do not offer to Apply it.",
            label
        ),
    }
}

fn worktree_cleaned_line(tier: u8, freed_bytes: u64, branch_deleted: bool) -> String {
    let freed = format_bytes(freed_bytes);
    if tier >= 2 {
        let branch = if branch_deleted {
            " Its branch was deleted as fully merged."
        } else {
            ""
        };
        format!(
            "- WORKTREE CLEANED: the cleanup worker removed your whole worktree ({} reclaimed) \
             after a long idle period. It has been recreated, so any untracked file you left \
             there is gone.{}",
            freed, branch
        )
    } else {
        format!(
            "- WORKTREE CLEANED: the cleanup worker reclaimed build artifacts from your worktree \
             ({} from `target/`, `node_modules/`, `.lucidos/cache/`). Commits and tracked files \
             are untouched, but the next build starts cold.",
            freed
        )
    }
}

/// Closing instructions, one per outcome class actually present. Kept out of
/// the bullets so a batch of ten applies doesn't repeat the same sentence ten
/// times.
///
/// The APPLIED line carries all three facts the pre-merge `applied_changes`
/// note put in its header: main contains the work, the worktree was reset to
/// match, and the change is no longer pending. That last clause is the whole
/// reason the note exists, so it cannot be dropped in favour of the shorter
/// "build on it".
fn closing_guidance(events: &[GapEvent]) -> Vec<&'static str> {
    let mut out = Vec::new();
    if events.iter().any(|e| matches!(e, GapEvent::Applied { .. })) {
        out.push(
            "Treat main as already containing the APPLIED work and your worktree as reset to \
             match main; build on it. Applied work is NOT pending: do not tell the user it is \
             awaiting Apply, and do not re-propose it.",
        );
    }
    if events
        .iter()
        .any(|e| matches!(e, GapEvent::Discarded { .. } | GapEvent::Reverted { .. }))
    {
        out.push(
            "Do not describe discarded or reverted work as pending, and do not offer to Apply it.",
        );
    }
    out
}

/// `"the description"` when the `changes` row has one, else `change abc12345`.
fn change_label(change_id: &str, facts: Option<&ChangeFacts>) -> String {
    match facts
        .map(|f| f.description.trim())
        .filter(|d| !d.is_empty())
    {
        Some(description) => format!("\"{}\"", truncate(description, MAX_DESCRIPTION_CHARS)),
        None => format!("change {}", short_change_id(change_id)),
    }
}

fn on_branch(branch: &str) -> String {
    if branch.is_empty() {
        String::new()
    } else {
        format!(" (branch `{}`)", branch)
    }
}

/// Short id for a bullet, char-boundary safe.
fn short_change_id(id: &str) -> String {
    if id.is_empty() {
        "?".to_string()
    } else {
        short(id)
    }
}

fn format_bytes(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else {
        format!("{} MB", bytes / MB)
    }
}

#[cfg(test)]
#[path = "turn_gap_tests.rs"]
mod tests;
