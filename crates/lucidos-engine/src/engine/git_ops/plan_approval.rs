//! The evidence `lucidos planned approve` needs: the user's own approval of the
//! proposed plan, read from the thread that owns the branch.
//!
//! The gate reads the newest human input recorded after the marker was last
//! written. A tapped option approves its scope unless it is `Request changes`. A typed
//! reply approves only when its whole text is a plain approval, so a question or
//! a counter-proposal never counts.
//!
//! A pick a confirmed *batch* made binds to the plan text the batch showed
//! (ADR 0664 amends ADR 0524). That pick approves only while the text is
//! unchanged.

use super::canonical_repo_root;
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

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

pub(crate) const PLAN_FOR_LATER_LABEL: &str = "Commit plan for later";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApprovalTarget {
    Implementation,
    PlanForLater,
    Design,
}

/// Whether the user's input grants implementation approval.
pub(crate) fn approves(input: &HumanInput) -> bool {
    approves_target(input, ApprovalTarget::Implementation)
}

/// Approval for later never grants permission to implement.
pub(crate) fn approves_target(input: &HumanInput, target: ApprovalTarget) -> bool {
    let later = match input {
        HumanInput::Picked { label } => label
            .as_deref()
            .is_some_and(|l| l.trim().eq_ignore_ascii_case(PLAN_FOR_LATER_LABEL)),
        HumanInput::Typed { text } => ["for later", "approve for later", "commit plan for later"]
            .contains(&normalize_reply(text).as_str()),
        HumanInput::Other => false,
    };
    if target == ApprovalTarget::PlanForLater {
        return later;
    }
    if later {
        return false;
    }
    match input {
        HumanInput::Picked { label } => label.as_deref().is_some_and(|l| {
            let label = l.trim();
            !label.eq_ignore_ascii_case(REQUEST_CHANGES_LABEL)
                && (target == ApprovalTarget::Design
                    || !["Approve design", "Approve design and plan here"]
                        .iter()
                        .any(|design| label.eq_ignore_ascii_case(design)))
        }),
        HumanInput::Typed { text } => {
            let reply = normalize_reply(text);
            PLAIN_APPROVALS.contains(&reply.as_str())
                || (target == ApprovalTarget::Design && reply == "approve design")
        }
        HumanInput::Other => false,
    }
}

/// Why `approve_plan` did not flip a proposed marker.
#[derive(Debug)]
pub(crate) enum PlanApprovalError {
    /// The user has not approved since the plan was proposed.
    NotApproved,
    /// A batch approved the plan's text, and the text has changed since.
    PlanChanged,
    /// The evidence could not be read. Unknown is never approval.
    Unreadable(sqlx::Error),
}

impl std::fmt::Display for PlanApprovalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanApprovalError::NotApproved => write!(
                f,
                "The user's latest input does not approve this plan for the requested scope. \
                 Use `planned approve --plan-only` only after `Commit plan for later` or a plain \
                 `for later` approval. Plain `planned approve` needs implementation approval. \
                 A question or suggestion is not approval."
            ),
            PlanApprovalError::PlanChanged => write!(
                f,
                "The user approved this plan in a batch, for the text the batch showed. The \
                 committed plan has changed since, so that approval no longer covers it. Run \
                 `lucidos planned mark --plan <path>` again and ask for a new approval."
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

type InputRow = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// The newest input a device recorded on `thread_id` after the marker's
/// `planned_at`, with the card it answered. Both timestamps come from
/// Postgres, so the comparison stays in SQL (ADR 0053). A tap on an engine
/// card, such as a step request's Keep paused, is never plan evidence.
async fn latest_human_input(
    pool: &sqlx::PgPool,
    thread_id: &str,
    repo_root: &Path,
    branch_name: &str,
) -> Result<Option<(HumanInput, Option<String>)>, sqlx::Error> {
    let row: Option<InputRow> = sqlx::query_as(
        "SELECT e.event_type, \
                e.payload->'answer'->>'kind', \
                COALESCE(e.payload->'answer'->>'text', e.payload->>'text'), \
                e.payload->>'tool_use_id', \
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
            AND NOT EXISTS (SELECT 1 FROM events c \
                             WHERE c.aggregate_id = e.aggregate_id \
                               AND c.event_type = 'UserQuestionAsked' \
                               AND c.payload->>'tool_use_id' = e.payload->>'tool_use_id' \
                               AND (c.payload ? 'owner_approval' OR c.payload ? 'unattested_act' \
                                    OR c.payload ? 'permission_request')) \
            AND e.created > (SELECT planned_at FROM planned_branches \
                              WHERE repo_root = $2 AND branch_name = $3) \
          ORDER BY e.sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .fetch_optional(pool)
    .await?;
    Ok(
        row.map(|(event_type, answer_kind, text, tool_use_id, label)| {
            let input = match (event_type.as_str(), answer_kind.as_deref()) {
                ("MessageReceived", _) | (_, Some("FreeText")) => HumanInput::Typed {
                    text: text.unwrap_or_default(),
                },
                (_, Some("Selected")) => HumanInput::Picked { label },
                _ => HumanInput::Other,
            };
            (input, tool_use_id)
        }),
    )
}

/// The plan text a batch item bound its pick to: the committed plan at the
/// marker it saw.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanBinding {
    pub repo_root: String,
    pub branch_name: String,
    pub plan_path: String,
    /// When the marker was written. A re-mark moves it, so it needs a new yes.
    pub marked_at: chrono::DateTime<chrono::Utc>,
    /// SHA-256 of the committed plan text, in hex.
    pub digest: String,
}

/// The proposed plan on the branch `thread_id`'s newest session works, with
/// its committed text's digest. `Ok(None)` when no plan waits for approval.
/// `Err` names why the plan could not be bound: an uncommitted plan has no
/// text to bind.
pub(crate) async fn proposed_plan_binding(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<Option<PlanBinding>, String> {
    let row: Option<(String, String, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT p.repo_root, p.branch_name, p.plan_path, p.planned_at FROM planned_branches p \
         WHERE p.state = 'proposed' AND p.plan_path IS NOT NULL \
           AND p.branch_name = (SELECT payload->>'branch' FROM events \
                                 WHERE thread_id = $1 AND event_type = 'SessionStarted' \
                                 ORDER BY sequence DESC LIMIT 1) \
         ORDER BY p.planned_at DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("the plan marker could not be read ({e})"))?;
    let Some((repo_root, branch_name, plan_path, marked_at)) = row else {
        return Ok(None);
    };
    let digest = plan_text_digest(Path::new(&repo_root), &branch_name, &plan_path).await?;
    Ok(Some(PlanBinding {
        repo_root,
        branch_name,
        plan_path,
        marked_at,
        digest,
    }))
}

/// SHA-256 of `plan_path` as committed on `branch_name`, in hex.
async fn plan_text_digest(
    repo_root: &Path,
    branch_name: &str,
    plan_path: &str,
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let spec = format!("refs/heads/{branch_name}:{plan_path}");
    let output = super::git_cmd(&["show", &spec], repo_root).await?;
    if !output.status.success() {
        return Err(format!(
            "the plan {plan_path} is not committed on {branch_name}, so its text cannot be bound"
        ));
    }
    Ok(crate::api::hex::hex_lower(&Sha256::digest(&output.stdout)))
}

/// The plan binding a confirmed batch gave the pick `tool_use_id` on
/// `thread_id`, if a batch made it. A batch that skipped the item made no
/// pick, so a later direct tap on the card owes it nothing.
async fn batch_plan_binding(
    pool: &sqlx::PgPool,
    thread_id: &str,
    tool_use_id: &str,
) -> Result<Option<PlanBinding>, sqlx::Error> {
    let item = serde_json::json!([{ "thread_id": thread_id, "tool_use_id": tool_use_id }]);
    let binding: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT i->'plan' FROM events q, \
                jsonb_array_elements(q.payload->'permission_request'->'batch'->'items') i \
          WHERE q.event_type = 'UserQuestionAsked' \
            AND q.payload->'permission_request'->'batch'->'items' @> $1 \
            AND i->>'thread_id' = $2 AND i->>'tool_use_id' = $3 AND i ? 'plan' \
            AND EXISTS (SELECT 1 FROM events g \
                         WHERE g.event_type = 'PermissionGranted' \
                           AND g.thread_id = q.thread_id \
                           AND g.payload->>'request_tool_use_id' = q.payload->>'tool_use_id' \
                           AND NOT EXISTS (SELECT 1 FROM events r \
                                            WHERE r.thread_id = g.thread_id \
                                              AND r.event_type = 'BatchItemRan' \
                                              AND r.payload->>'permission_id' \
                                                  = g.payload->>'permission_id' \
                                              AND r.payload->'item' @> ($1->0) \
                                              AND r.payload->'outcome'->>'kind' = 'skipped')) \
          ORDER BY q.sequence DESC LIMIT 1",
    )
    .bind(item)
    .bind(thread_id)
    .bind(tool_use_id)
    .fetch_optional(pool)
    .await?;
    Ok(binding.and_then(|b| serde_json::from_value(b).ok()))
}

/// Whether a batch's plan binding still covers the plan on `branch_name`:
/// the same marker and the same committed text. A bound pick from an
/// earlier marker is not this marker's approval at all, so it never counts.
async fn binding_still_holds(
    pool: &sqlx::PgPool,
    binding: &PlanBinding,
    repo_root: &Path,
    branch_name: &str,
) -> Result<bool, sqlx::Error> {
    let marked_at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT planned_at FROM planned_branches WHERE repo_root = $1 AND branch_name = $2",
    )
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .fetch_optional(pool)
    .await?;
    if marked_at != Some(binding.marked_at) {
        return Ok(false);
    }
    let current = plan_text_digest(
        Path::new(&binding.repo_root),
        &binding.branch_name,
        &binding.plan_path,
    )
    .await;
    Ok(current.as_deref() == Ok(binding.digest.as_str()))
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
    user_approved_plan_for(
        pool,
        repo_root,
        branch_name,
        caller_thread,
        ApprovalTarget::Implementation,
    )
    .await
}

pub(crate) async fn user_approved_plan_for(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
    caller_thread: Option<&str>,
    target: ApprovalTarget,
) -> Result<(), PlanApprovalError> {
    let thread_id = match caller_thread {
        Some(thread) => thread.to_string(),
        None => match owning_thread(pool, branch_name)
            .await
            .map_err(PlanApprovalError::Unreadable)?
        {
            Some(thread) => thread,
            None if target == ApprovalTarget::PlanForLater => {
                return Err(PlanApprovalError::NotApproved)
            }
            None => return Ok(()),
        },
    };
    let input = latest_human_input(pool, &thread_id, repo_root, branch_name)
        .await
        .map_err(PlanApprovalError::Unreadable)?;
    let card = match input {
        Some((input, card))
            if match target {
                ApprovalTarget::Implementation => approves(&input),
                _ => approves_target(&input, target),
            } =>
        {
            card
        }
        _ => return Err(PlanApprovalError::NotApproved),
    };
    let Some(card) = card else {
        return Ok(());
    };
    let binding = batch_plan_binding(pool, &thread_id, &card)
        .await
        .map_err(PlanApprovalError::Unreadable)?;
    match binding {
        None => Ok(()),
        Some(binding) => match binding_still_holds(pool, &binding, repo_root, branch_name).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(PlanApprovalError::PlanChanged),
            Err(e) => Err(PlanApprovalError::Unreadable(e)),
        },
    }
}
