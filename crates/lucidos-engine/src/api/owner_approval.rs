//! *Owner approval*: the owner's Allow once on an engine-worded card lets one
//! thread press one clause-4 act, once (ADR 0387).
//!
//! The third thing `api::thread_reach` asks, after reach and the standing
//! instruction, and narrower than both. Four properties make it safe to ask:
//!
//! - **The engine words the card.** The agent supplies a reason, which is
//!   quoted as the agent's. It writes no part of what the owner taps.
//! - **The MAC names the thread.** The request and the spend take the thread
//!   from the *thread-bound origin token*, never from a body. The card lands
//!   only on the thread whose request it names.
//! - **Only a registered device's Allow counts.** The answer route refuses any
//!   other caller for this card, and the lookup reads the answer's actor kind.
//! - **One act, once, this turn.** The verb and target must match. A unique
//!   index refuses a second spend. A newer turn start ends an unspent approval.
//!
//! A question card belongs to the turn that asks it, so the card rides the
//! agent's own blocking question tool. `lucidos ask-owner-approval` records
//! the request and prints its id. The agent then asks `AskUserQuestion` with
//! that id as its question, and [`approval_ask`] swaps in the engine's card.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::actor::SubprocessOrigin;
use crate::api::error::ApiError;
use crate::api::thread_reach::{authority_held, ThreadReachVerb};
use crate::api::AppState;
use crate::engine::agent_question::is_owner_approval_card;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, OwnerApproval, ThreadEvent};

/// The label of the option whose selection, by a registered device, is the
/// approval. Matched by label because the engine writes it and the owner
/// reads it, while an option's id is the question parser's own numbering.
pub(crate) const ALLOW_ONCE_LABEL: &str = "Allow once";
pub(crate) const DONT_ALLOW_LABEL: &str = "Don't allow";

/// Every request id starts with this, so the ask route can tell an approval
/// ask from an ordinary question by its text alone.
const REQUEST_ID_PREFIX: &str = "owner-approval:";

/// The longest reason the card quotes, in characters. The card is one screen.
const REASON_MAX_CHARS: usize = 600;

/// The longest target title the card shows, in characters.
const TITLE_MAX_CHARS: usize = 120;

/// The unique index that makes a spend single-use.
const SPENT_UNIQUE_INDEX: &str = "events_owner_approval_spent_unique";

/// The verb as stored in events, for SQL that compares the stored spelling.
fn verb_wire(verb: ThreadReachVerb) -> String {
    serde_json::to_value(verb)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Spend a live owner approval for this exact act, if the caller holds one.
///
/// `true` means an `OwnerApprovalSpent` landed and the act may proceed. Every
/// other outcome is `false`: none held, a lost race for the same approval, or
/// a read or write that failed. An unknown must not stand in for the owner.
pub(super) async fn spend_owner_approval(
    bus: &EventBus,
    caller_thread_id: Uuid,
    verb: ThreadReachVerb,
    target: Option<Uuid>,
) -> bool {
    // No card can propose this verb, so none can be live for it.
    if !verb.proposable() {
        return false;
    }
    let Some(tool_use_id) = live_owner_approval(bus.pool(), caller_thread_id, verb, target).await
    else {
        return false;
    };
    match bus
        .emit(BusEvent::Thread {
            thread_id: caller_thread_id,
            event: ThreadEvent::OwnerApprovalSpent {
                tool_use_id: tool_use_id.clone(),
                verb,
                target_thread_id: target,
            },
            meta: EventMeta::NONE,
        })
        .await
    {
        Ok(_) => true,
        Err(e) => {
            let why = if e.to_string().contains(SPENT_UNIQUE_INDEX) {
                "another call spent it first".to_string()
            } else {
                e.to_string()
            };
            crate::log!(
                "[OwnerApproval] Could not spend approval {tool_use_id} on thread \
                 {caller_thread_id}: {why}; refusing"
            );
            false
        }
    }
}

/// The newest live approval on `thread_id` for this verb and target, by card
/// id. Live means allowed by a device, unspent, and raised in the current turn.
async fn live_owner_approval(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    verb: ThreadReachVerb,
    target: Option<Uuid>,
) -> Option<String> {
    // The card must be newer than the current turn start, so its answer is
    // too. The inner `FROM events` is unaliased, so the shared fragment's bare
    // columns bind to it.
    let sql = format!(
        "SELECT q.payload->>'tool_use_id' \
         FROM events q \
         JOIN events a ON a.aggregate_id = q.aggregate_id \
              AND a.event_type = 'UserQuestionAnswered' \
              AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id' \
         WHERE q.aggregate_id = $1 AND q.event_type = 'UserQuestionAsked' \
           AND q.payload->'owner_approval'->>'verb' = $2 \
           AND q.payload->'owner_approval'->>'target_thread_id' IS NOT DISTINCT FROM $3::text \
           AND a.payload->'answer'->>'kind' = 'Selected' \
           AND a.payload->'actor'->>'kind' = 'device' \
           AND EXISTS ( \
                 SELECT 1 FROM jsonb_array_elements(q.payload->'options') o \
                 WHERE o->>'id' = a.payload->'answer'->>'option_id' AND o->>'label' = $4) \
           AND q.sequence > COALESCE(( \
                 SELECT MAX(sequence) FROM events WHERE aggregate_id = $1 AND {opens} \
               ), 0) \
           AND NOT EXISTS ( \
                 SELECT 1 FROM events s WHERE s.aggregate_id = $1 \
                   AND s.event_type = 'OwnerApprovalSpent' \
                   AND s.payload->>'tool_use_id' = q.payload->>'tool_use_id') \
         ORDER BY a.sequence DESC LIMIT 1",
        opens = crate::api::standing_instruction::opens_current_turn_sql(),
    );
    sqlx::query_scalar::<_, String>(&sql)
        .bind(thread_id.to_string())
        .bind(verb_wire(verb))
        .bind(target.map(|t| t.to_string()))
        .bind(ALLOW_ONCE_LABEL)
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|e| {
            crate::log!(
                "[OwnerApproval] Could not read the approvals of thread {thread_id}: {e}; \
                 treating it as holding none"
            );
            None
        })
}

/// The card's question, in the engine's words. The agent's reason is quoted
/// line by line, so no blank line can close the quote and pose as engine text.
/// A lone `\r` counts as a line break here, as it does in the card's renderer.
/// `target_title` is `None` for a verb aimed at the root.
fn owner_approval_question(
    verb: ThreadReachVerb,
    target_title: Option<&str>,
    reason: &str,
) -> String {
    let act = match target_title {
        Some(title) => format!("{} on \u{201c}{}\u{201d}", verb.act(), one_line(title)),
        None => verb.act().to_string(),
    };
    let quoted: String = reason
        .split(['\n', '\r'])
        .map(|line| format!("> {line}\n"))
        .collect();
    format!(
        "**Let this thread act outside its own subtree, once?**\n\n\
         It asks for exactly one act: **{act}**. Allow once lets it do that one \
         time, and nothing else.\n\n\
         The agent's reason:\n\n{quoted}"
    )
}

/// The card as the question tool's input, so the walk and the hook's answer
/// map read it exactly as they read an agent's own question.
fn owner_approval_questions(question: &str) -> serde_json::Value {
    serde_json::json!([{
        "question": question,
        "header": "Approval",
        "multiSelect": false,
        "options": [
            { "label": ALLOW_ONCE_LABEL, "description": "Let the thread do this one act, once." },
            { "label": DONT_ALLOW_LABEL, "description": "The thread stays inside its own subtree." },
        ],
    }])
}

/// A title on one line, capped, with its markdown escaped, so it cannot end
/// the card's bold sentence or start one of its own. Another thread may have
/// written it.
fn one_line(title: &str) -> String {
    let flat = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let capped = match flat.char_indices().nth(TITLE_MAX_CHARS) {
        Some((end, _)) => format!("{}\u{2026}", &flat[..end]),
        None => flat,
    };
    capped
        .chars()
        .flat_map(|c| {
            let escape = "\\`*_{}[]<>()#+-.!|~".contains(c);
            escape.then_some('\\').into_iter().chain(std::iter::once(c))
        })
        .collect()
}

/// What a question batch an agent asked turns out to be.
#[derive(Debug)]
pub(in crate::api) enum ApprovalAsk {
    /// An ordinary question card.
    Ordinary,
    /// An ask naming an owner approval request: the engine's card replaces it.
    Approval {
        approval: OwnerApproval,
        questions: serde_json::Value,
    },
}

/// Is this ask an owner approval request? An agent asks one by putting the id
/// `lucidos ask-owner-approval` printed as its only question. The request must
/// exist on this same thread, which the caller has already bound to the token.
///
/// `Err` carries the refusal the agent reads, and no card is raised.
pub(in crate::api) async fn approval_ask(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    questions: &serde_json::Value,
) -> Result<ApprovalAsk, String> {
    let texts: Vec<String> = questions
        .as_array()
        .map(|qs| {
            qs.iter()
                .filter_map(crate::engine::agent_session::question_text)
                .collect()
        })
        .unwrap_or_default();
    let Some(request_id) = texts.iter().find(|t| t.starts_with(REQUEST_ID_PREFIX)) else {
        return Ok(ApprovalAsk::Ordinary);
    };
    if texts.len() != 1 {
        return Err(
            "Ask an owner approval card on its own: one question, whose text is \
                    the request id and nothing else."
                .to_string(),
        );
    }
    let row: Option<(serde_json::Value, String)> = sqlx::query_as(
        "SELECT payload->'approval', payload->>'question' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'OwnerApprovalRequested' \
           AND payload->>'request_id' = $2 \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id.to_string())
    .bind(request_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("Could not read the owner approval request: {e}"))?;
    let Some((approval, question)) = row else {
        return Err(format!(
            "No owner approval request {request_id} exists on this thread. Run \
             `lucidos ask-owner-approval` from this thread first, then ask with the id it \
             prints."
        ));
    };
    let approval: OwnerApproval = serde_json::from_value(approval)
        .map_err(|e| format!("The owner approval request is unreadable: {e}"))?;
    Ok(ApprovalAsk::Approval {
        approval,
        questions: owner_approval_questions(&question),
    })
}

#[derive(Deserialize)]
pub(in crate::api) struct RequestOwnerApprovalBody {
    verb: ThreadReachVerb,
    #[serde(default)]
    target_thread_id: Option<Uuid>,
    reason: String,
}

/// POST /api/v1/owner-approvals: record an owner approval request on the
/// calling thread, and return the id the agent asks `AskUserQuestion` with.
///
/// Only a coding agent inside its own thread may call it, and the request
/// lands on the thread its token names. Body: `{verb, target_thread_id?,
/// reason}`.
pub(in crate::api) async fn request_owner_approval(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RequestOwnerApprovalBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let caller = calling_agent_thread(&headers)?;
    let reason = checked_request(body.verb, body.target_thread_id, &body.reason)?;
    let target_title = match body.target_thread_id {
        Some(target) => Some(thread_title(&state.pool, target).await?),
        None => None,
    };
    if authority_held(&state.pool, Some(caller), None, body.target_thread_id)
        .await
        .map_err(ApiError::from)?
        .is_some()
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!(
                "You already hold the authority for {}: it is inside your own subtree, or \
                 this turn carries the owner's standing instruction. Do it without a card.",
                body.verb.act()
            ),
        ));
    }

    let request_id = format!("{REQUEST_ID_PREFIX}{}", Uuid::new_v4());
    let question = owner_approval_question(body.verb, target_title.as_deref(), reason);
    state
        .engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id: caller,
            event: ThreadEvent::OwnerApprovalRequested {
                request_id: request_id.clone(),
                approval: OwnerApproval {
                    verb: body.verb,
                    target_thread_id: body.target_thread_id,
                },
                question,
            },
            meta: EventMeta::NONE,
        })
        .await
        .map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Could not record the request: {e}"),
            )
        })?;
    Ok(Json(serde_json::json!({ "request_id": request_id })))
}

/// The calling agent's own thread, as its origin token's MAC proves it. Any
/// other caller is refused: the card must land on the thread that asks.
fn calling_agent_thread(headers: &HeaderMap) -> Result<Uuid, ApiError> {
    match crate::api::actor::subprocess_origin(headers) {
        SubprocessOrigin::Subprocess {
            source_thread_id: Some(caller),
            ..
        } => Ok(caller),
        _ => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "An owner approval is asked for by a coding agent inside its own thread, \
             through `lucidos ask-owner-approval`. The card lands on that thread.",
        )),
    }
}

/// Check the request's shape, and return the reason trimmed and capped.
fn checked_request(
    verb: ThreadReachVerb,
    target: Option<Uuid>,
    reason: &str,
) -> Result<&str, ApiError> {
    let bad = |msg: String| ApiError::new(StatusCode::BAD_REQUEST, msg);
    if !verb.proposable() {
        return Err(bad(format!(
            "A card cannot propose {}: that is the owner's own answer, on screen.",
            verb.act()
        )));
    }
    match (verb.aims_at_root(), target) {
        (true, Some(_)) => {
            return Err(bad(format!(
                "{} aims at the workspace root, so it takes no --thread.",
                verb.act()
            )))
        }
        (false, None) => {
            return Err(bad(format!(
                "Name the thread {} aims at, with --thread <id>.",
                verb.act()
            )))
        }
        _ => {}
    }
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(bad(
            "Give a reason with --reason. The owner reads it on the card.".to_string(),
        ));
    }
    Ok(match reason.char_indices().nth(REASON_MAX_CHARS) {
        Some((end, _)) => &reason[..end],
        None => reason,
    })
}

/// The target's title, or a 404 naming the id when no such thread exists.
async fn thread_title(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<String, ApiError> {
    let row: Option<Option<String>> =
        sqlx::query_scalar("SELECT title FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    match row {
        Some(title) => Ok(title.unwrap_or_else(|| thread_id.to_string())),
        None => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("No thread {thread_id} exists."),
        )),
    }
}

/// What an agent is told when it tries to answer an owner approval card.
const OWNER_APPROVAL_IS_THE_OWNERS: &str =
    "An owner approval card is answered by the workspace owner alone, from one of their \
     own devices. Its Allow lets a thread act outside its subtree, so no agent may press \
     it, including the one that raised it.";

/// Who is answering this card. On an owner approval card, anyone but the
/// owner at a registered device is refused, because its Allow is spendable
/// authority. A card that cannot be classified is refused rather than read as
/// an ordinary one.
pub(in crate::api) async fn answering_actor(
    pool: &sqlx::PgPool,
    headers: &HeaderMap,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<Option<MessageOrigin>, ApiError> {
    let approval_card = is_owner_approval_card(pool, thread_id, tool_use_id)
        .await
        .map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Could not read the question card: {e}"),
            )
        })?;
    if !approval_card {
        return Ok(crate::api::actor::user_actor(headers, None));
    }
    crate::api::actor::require_owner_device(headers, pool)
        .await
        .map(Some)
        .map_err(|_| ApiError::new(StatusCode::FORBIDDEN, OWNER_APPROVAL_IS_THE_OWNERS))
}

pub(super) fn router() -> axum::Router<AppState> {
    axum::Router::new().route(
        "/owner-approvals",
        axum::routing::post(request_owner_approval),
    )
}

#[cfg(test)]
#[path = "owner_approval_tests.rs"]
mod tests;
