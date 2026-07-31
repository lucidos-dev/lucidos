//! Applied-change resume note (sibling of `external_edits`).
//!
//! When a coding agent proposes a change and then the user clicks Apply, the
//! agent's branch is merged into `main` and its worktree is reset to match
//! `main` — but the resumed agent has no idea this happened. `--resume`
//! replays the prior conversation, in which the change was still "pending,
//! awaiting Apply". Left uncorrected, the agent will tell the user the work
//! is still waiting to be applied, or re-propose what is already merged.
//!
//! [`compute_applied_change_note`] closes that gap with a short, stateless
//! note prepended to the resumed prompt. It surfaces every `ChangeApplied`
//! that landed in the gap between the agent's **previous** turn and the
//! **current** one — i.e. with a `sequence` strictly between the previous turn
//! boundary and the current turn's triggering message:
//!
//! - **Upper bound (exclusive):** the current turn's triggering event
//!   (`current_origin_id`). The engine persists the user's `MessageReceived`
//!   *before* spawning/resuming the agent (see `chat/process/run.rs` — "Emit
//!   MessageReceived FIRST"), so by the time this runs the current message is
//!   already in the events table. Excluding it is what makes the threshold the
//!   *previous* boundary rather than the current one.
//! - **Lower bound (exclusive):** the most-recent turn-boundary event
//!   *before* the current one — `MessageReceived` (the real user-message event
//!   on coding-agent threads), `CodingAgentUserMessageSent` (legacy), or
//!   `CodingAgentPromptSent` (engine-synthesized prompts).
//!
//! This is **stateless and self-clearing**: once the note rides on a turn,
//! that turn's own `MessageReceived` becomes the *previous* boundary for the
//! next turn, so the same applies fall below the threshold and never re-fire —
//! without needing any per-turn engine-synthesized prompt to exist. No new
//! event, no projection column, no state to clear: the events table is the
//! cursor.
//!
//! Like `external_edits`, the helper is best-effort: any query error degrades
//! silently to `None` (resume proceeds without the note rather than failing).

use uuid::Uuid;

/// Max number of applied-change bullet lines to render before truncating, so a
/// thread that applied a large batch can't blow up the resumed prompt. Mirrors
/// the `MAX_LINES` convention in `external_edits`.
const MAX_LINES: usize = 50;

/// One applied change pulled from a `ChangeApplied` payload.
struct AppliedChange {
    change_id: String,
    /// Commit subjects merged to `main`, oldest first. Empty for no-op applies.
    commits: Vec<String>,
    /// SHA of `main` after the merge, already shortened to <= 8 chars.
    short_sha: Option<String>,
}

/// Build a note telling the resuming agent which of its proposed changes were
/// applied (merged into main) in the gap before the current turn. Returns
/// `None` when none.
///
/// `current_origin_id` is the event that triggered this turn (the user's
/// `MessageReceived`, already persisted — see the module docs). It is the
/// exclusive upper bound, so the threshold becomes the *previous* turn
/// boundary. Stateless & self-clearing: this turn's message is the next turn's
/// previous boundary, so each apply is surfaced exactly once.
pub(crate) async fn compute_applied_change_note(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    current_origin_id: Uuid,
) -> Option<String> {
    // Every `ChangeApplied` after the previous turn boundary but before the
    // current turn's message, oldest first. The lower-bound subquery EXCLUDES
    // the current message (`id <> $2`) so it resolves to the *previous*
    // boundary; `COALESCE(.., 0)` covers a thread with no earlier boundary.
    // `MessageReceived` is the real user-message event on coding-agent threads
    // (`CodingAgentUserMessageSent` is legacy/near-unused); including it is
    // what makes the threshold advance on every plain turn, i.e. self-clear.
    let rows: Vec<serde_json::Value> = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT payload FROM events \
         WHERE thread_id = $1 AND event_type = 'ChangeApplied' \
           AND sequence > COALESCE(( \
             SELECT MAX(sequence) FROM events \
             WHERE thread_id = $1 \
               AND id <> $2 \
               AND event_type IN ( \
                 'MessageReceived', \
                 'CodingAgentUserMessageSent', \
                 'CodingAgentPromptSent' \
               ) \
           ), 0) \
         ORDER BY sequence ASC",
    )
    .bind(thread_id)
    .bind(current_origin_id)
    .fetch_all(pool)
    .await
    .map_err(|e| {
        log!(
            "[AgentSession] Failed to look up applied changes for {}: {}",
            thread_id,
            e
        );
        e
    })
    .ok()?;

    let applied: Vec<AppliedChange> = rows.into_iter().map(parse_applied_change).collect();

    if applied.is_empty() {
        return None;
    }

    Some(build_note(&applied))
}

/// Pull `change_id`, `commits`, and a shortened `post_merge_sha` out of a
/// `ChangeApplied` payload. Missing fields degrade to empty rather than
/// failing the whole note.
fn parse_applied_change(payload: serde_json::Value) -> AppliedChange {
    let change_id = payload
        .get("change_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let commits = payload
        .get("commits")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let short_sha = payload
        .get("post_merge_sha")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(short_sha);

    AppliedChange {
        change_id,
        commits,
        short_sha,
    }
}

/// Shorten a SHA to its first 8 characters on a char boundary (never a raw
/// byte slice — see CLAUDE.md "Never slice strings by byte index").
fn short_sha(sha: &str) -> String {
    sha[..sha.floor_char_boundary(8)].to_string()
}

fn build_note(applied: &[AppliedChange]) -> String {
    let mut note = String::from(
        "[Note from engine: the change(s) you proposed were APPLIED since your last turn — \
         their commits are now merged into `main` and your worktree has been reset to match \
         `main`. They are NOT pending anymore; do not tell the user it's awaiting Apply.\n\
         Applied:",
    );

    // One bullet per commit across all applied changes, oldest-first, capped.
    let mut lines = 0usize;
    let mut truncated = 0usize;
    for change in applied {
        let sha_suffix = match &change.short_sha {
            Some(s) => format!(" (now in main at {})", s),
            None => " (now in main)".to_string(),
        };

        if change.commits.is_empty() {
            // No-op apply (nothing to merge / already merged). Render a row
            // anyway so the agent knows the change resolved, rather than an
            // empty bullet.
            if lines < MAX_LINES {
                note.push_str(&format!(
                    "\n- (change {}: applied, no commits / already merged)",
                    short_change_id(&change.change_id)
                ));
                lines += 1;
            } else {
                truncated += 1;
            }
            continue;
        }

        for subject in &change.commits {
            if lines < MAX_LINES {
                note.push_str(&format!("\n- {}{}", subject, sha_suffix));
                lines += 1;
            } else {
                truncated += 1;
            }
        }
    }

    if truncated > 0 {
        note.push_str(&format!("\n… and {} more", truncated));
    }

    note.push_str("\nTreat main as already containing this work; build on it.]");
    note
}

/// Short id for a no-op apply bullet — first 8 chars, char-boundary safe.
fn short_change_id(id: &str) -> String {
    if id.is_empty() {
        "?".to_string()
    } else {
        id[..id.floor_char_boundary(8)].to_string()
    }
}

#[cfg(test)]
#[path = "applied_changes_tests.rs"]
mod tests;
