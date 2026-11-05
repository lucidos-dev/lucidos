//! The evidence `lucidos planned approve` needs: the user's own approval of the
//! proposed plan, read from the thread that owns the branch.
//!
//! The gate reads the newest human input recorded after the marker was last
//! written. A tapped option approves unless it is `Request changes`. A typed
//! reply approves only when its whole text is a plain approval, so a question or
//! a counter-proposal never counts.

use super::canonical_repo_root;
use std::path::Path;

/// The one plan-card option that never approves, whatever else the card offers.
pub(crate) const REQUEST_CHANGES_LABEL: &str = "Request changes";

/// Typed replies that approve, compared after [`normalize_reply`]. The set is
/// closed: anything longer, or any question, asks for changes.
const PLAIN_APPROVALS: [&str; 8] = [
    "approve",
    "approved",
    "approve it",
    "approve the plan",
    "yes",
    "yes approve",
    "lgtm",
    "go ahead",
];

/// What the user last did on the owning thread, after the marker was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HumanInput {
    /// A tapped option. `label` is `None` when the card's option cannot be found.
    Picked { label: Option<String> },
    /// A typed answer or message.
    Typed { text: String },
    /// A cancelled, superseded or multi-select answer. None of them approve.
    Other,
}

/// Trim, lowercase and strip a closing `.` or `!`, so "Approve." matches
/// "approve". A `?` stays: "approve?" asks, it does not approve.
fn normalize_reply(text: &str) -> String {
    text.trim()
        .trim_end_matches(['.', '!'])
        .trim()
        .to_lowercase()
}

/// Whether the user's last input approves the plan.
pub(crate) fn approves(input: &HumanInput) -> bool {
    match input {
        HumanInput::Picked { label } => label
            .as_deref()
            .is_some_and(|l| !l.trim().eq_ignore_ascii_case(REQUEST_CHANGES_LABEL)),
        HumanInput::Typed { text } => PLAIN_APPROVALS.contains(&normalize_reply(text).as_str()),
        HumanInput::Other => false,
    }
}

/// Why `approve_plan` did not flip a proposed marker.
#[derive(Debug)]
pub(crate) enum PlanApprovalError {
    /// The user has not approved since the plan was proposed.
    NotApproved,
    /// The evidence could not be read. Unknown is never approval.
    Unreadable(sqlx::Error),
}

impl std::fmt::Display for PlanApprovalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanApprovalError::NotApproved => write!(
                f,
                "The user has not approved this plan since you proposed it. A typed question, \
                 suggestion or counter-proposal is a request for changes, not an approval. \
                 Revise the plan, run `lucidos planned mark --plan <path>` again, and ask on a \
                 question card that carries the plan summary (or what changed). Run \
                 `lucidos planned approve` only after the user taps an approving option."
            ),
            PlanApprovalError::Unreadable(e) => write!(
                f,
                "The engine could not read the user's answer, so it cannot tell whether they \
                 approved ({e}). Retry; if it keeps failing, report it as blocked."
            ),
        }
    }
}

impl std::error::Error for PlanApprovalError {}

/// The thread a coding agent works the branch on: the newest `SessionStarted`
/// naming it. `None` for a branch no agent session started, such as a human's
/// own, which has no thread to read.
async fn owning_thread(
    pool: &sqlx::PgPool,
    branch_name: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT aggregate_id FROM events \
         WHERE event_type = 'SessionStarted' AND payload->>'branch' = $1 \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(branch_name)
    .fetch_optional(pool)
    .await
}

type InputRow = (String, Option<String>, Option<String>, Option<String>);

/// The newest input a device recorded on `thread_id` after the marker's
/// `planned_at`. Both timestamps come from Postgres, so the comparison stays in
/// SQL (ADR 0053).
async fn latest_human_input(
    pool: &sqlx::PgPool,
    thread_id: &str,
    repo_root: &Path,
    branch_name: &str,
) -> Result<Option<HumanInput>, sqlx::Error> {
    let row: Option<InputRow> = sqlx::query_as(
        "SELECT e.event_type, \
                e.payload->'answer'->>'kind', \
                COALESCE(e.payload->'answer'->>'text', e.payload->>'text'), \
                (SELECT o->>'label' \
                   FROM events q, jsonb_array_elements(q.payload->'options') o \
                  WHERE q.aggregate_id = e.aggregate_id \
                    AND q.event_type = 'UserQuestionAsked' \
                    AND q.payload->>'tool_use_id' = e.payload->>'tool_use_id' \
                    AND o->>'id' = e.payload->'answer'->>'option_id' \
                  LIMIT 1) \
           FROM events e \
          WHERE e.aggregate_id = $1 \
            AND e.event_type IN ('UserQuestionAnswered', 'MessageReceived') \
            AND COALESCE(e.payload->'answer'->>'kind', '') <> 'Superseded' \
            AND (e.payload->'actor'->>'kind' = 'device' \
                 OR e.payload->'origin'->>'kind' = 'device') \
            AND e.created > (SELECT planned_at FROM planned_branches \
                              WHERE repo_root = $2 AND branch_name = $3) \
          ORDER BY e.sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(event_type, answer_kind, text, label)| {
        match (event_type.as_str(), answer_kind.as_deref()) {
            ("MessageReceived", _) | (_, Some("FreeText")) => HumanInput::Typed {
                text: text.unwrap_or_default(),
            },
            (_, Some("Selected")) => HumanInput::Picked { label },
            _ => HumanInput::Other,
        }
    }))
}

/// Whether the user approved the proposed plan on `branch_name`.
///
/// `caller_thread` is the thread an agent's origin token names. The token is
/// signed, so it holds even when the agent renamed its branch. Without it the
/// thread is the one that started a session on the branch. A branch no session
/// started, such as a human's own, passes.
pub(crate) async fn user_approved_plan(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
    caller_thread: Option<&str>,
) -> Result<(), PlanApprovalError> {
    let thread_id = match caller_thread {
        Some(thread) => thread.to_string(),
        None => match owning_thread(pool, branch_name)
            .await
            .map_err(PlanApprovalError::Unreadable)?
        {
            Some(thread) => thread,
            None => return Ok(()),
        },
    };
    let input = latest_human_input(pool, &thread_id, repo_root, branch_name)
        .await
        .map_err(PlanApprovalError::Unreadable)?;
    match input {
        Some(input) if approves(&input) => Ok(()),
        _ => Err(PlanApprovalError::NotApproved),
    }
}
