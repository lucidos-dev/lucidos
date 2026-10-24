//! The inputs a coding agent was sent and never read.
//!
//! A process that exits before its session began leaves the user's message
//! unread at the end of the thread ([`undelivered_input`]). The resume then
//! carries the input itself. A bare "Continue from where you left off" would
//! ask the agent to resume work it was never told about.
//!
//! A thread whose agent never engaged is the extreme case: every prompt it
//! holds is unread, engine-written ones included ([`resume_input`]).

use uuid::Uuid;

use super::USER_CLICKED_CONTINUE_REASON;

/// What a coding agent was owed and never read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UndeliveredInput {
    pub text: String,
    pub image_hashes: Vec<String>,
    /// The owed inputs' event ids. The resume's first prompt carries them, so
    /// its read settles each one.
    pub event_ids: Vec<Uuid>,
}

/// Events that end the search, or name what the search is looking for.
const SEARCHED_EVENT_TYPES: &[&str] = &[
    "MessageReceived",
    "ResponseFailed",
    "ResponseGenerated",
    "ResponseCanceled",
    "ResponseAborted",
    "CodingAgentInputRead",
];

/// How far back the search reads. A stopped start sits at the end of the
/// thread, so a few events cover it.
const SEARCH_WINDOW: i64 = 100;

type Row = (Uuid, String, serde_json::Value);

/// What a resume of `thread_id` carries as its input, or `None` for the bare
/// continue message.
///
/// A thread whose agent never engaged has no conversation to resume, so every
/// reason hands it the prompts it was never given. A thread whose agent engaged
/// resumes its session, and only Continue replays what a failed start left
/// unread.
pub(crate) async fn resume_input(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    continue_reason: Option<&str>,
) -> Option<UndeliveredInput> {
    if agent_never_engaged(pool, thread_id).await {
        return unengaged_input(pool, thread_id).await;
    }
    match continue_reason {
        Some(USER_CLICKED_CONTINUE_REASON) => undelivered_input(pool, thread_id).await,
        _ => None,
    }
}

/// The events that prove the agent took an input in: it read one, or it
/// answered. A session that opened and died first leaves none of them, however
/// far its spawn got (`SessionStarted` is written before the first read).
const AGENT_ENGAGED_EVENT_TYPES: &str = "'CodingAgentInputRead','CodingAgentTextStreamed',\
    'CodingAgentThoughtStreamed','CodingAgentToolCalled','ResponseGenerated'";

/// SQL boolean: no [`AGENT_ENGAGED_EVENT_TYPES`] event exists on the thread at
/// `id_expr` (its `aggregate_id`, text). One definition for the resume input
/// and the boot scan that queues the resume.
pub(crate) fn agent_never_engaged_sql(id_expr: &str) -> String {
    format!(
        "NOT EXISTS (SELECT 1 FROM events g \
          WHERE g.aggregate_id = {id_expr} \
            AND g.event_type IN ({AGENT_ENGAGED_EVENT_TYPES}))"
    )
}

/// Whether the agent never took an input in. An unreadable answer counts as
/// no, so a failed probe never replays a prompt.
async fn agent_never_engaged(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar(&format!("SELECT {}", agent_never_engaged_sql("$1")))
        .bind(thread_id.to_string())
        .fetch_one(pool)
        .await
        .unwrap_or_else(|e| {
            crate::log!(
                "[Recovery] Could not tell whether the agent of thread {} engaged: {}",
                thread_id,
                e
            );
            false
        })
}

/// Every message of a thread whose agent never engaged, oldest first, minus the
/// ones the user took back.
async fn unengaged_input(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<UndeliveredInput> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT m.id, m.event_type, m.payload FROM events m \
          WHERE m.thread_id = $1 AND m.event_type = 'MessageReceived' \
            AND NOT EXISTS ( \
                SELECT 1 FROM events t \
                 WHERE t.thread_id = $1 \
                   AND t.event_type = 'QueuedMessageRemoved' \
                   AND t.payload->>'removed_message_id' = m.id::text) \
          ORDER BY m.sequence ASC",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log!(
            "[Recovery] Could not read the messages of unengaged thread {}: {}",
            thread_id,
            e
        );
        Vec::new()
    });
    joined(rows.iter().map(|(id, _, payload)| {
        (
            *id,
            payload.get("text").and_then(|t| t.as_str()).unwrap_or(""),
            payload,
        )
    }))
}

/// The human messages the thread's latest failed starts left unread, oldest
/// first, or `None` when the agent read everything it was sent.
///
/// Reads backward from the newest event. A start failure and a human message
/// are skipped or collected. Anything else (another terminal, or the agent
/// reading an input) means the agent saw everything before it, so the search
/// ends there. A follow-up a session forwarded is left out: the next session
/// redelivers it (`stranded_follow_ups`).
pub(crate) async fn undelivered_input(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Option<UndeliveredInput> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT e.id, e.event_type, e.payload FROM events e \
          WHERE e.thread_id = $1 AND e.event_type = ANY($2::text[]) \
            AND NOT (e.event_type = 'MessageReceived' AND EXISTS ( \
                  SELECT 1 FROM events p \
                   WHERE p.thread_id = $1 \
                     AND p.event_type = 'CodingAgentPromptSent' \
                     AND p.payload->>'input_event_id' = e.id::text)) \
          ORDER BY e.sequence DESC LIMIT $3",
    )
    .bind(thread_id)
    .bind(SEARCHED_EVENT_TYPES)
    .bind(SEARCH_WINDOW)
    .fetch_all(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log!(
            "[Recovery] Could not read thread {} for an undelivered message: {}",
            thread_id,
            e
        );
        Vec::new()
    });
    undelivered_input_from(&rows)
}

/// [`undelivered_input`] over events, newest first.
fn undelivered_input_from(newest_first: &[Row]) -> Option<UndeliveredInput> {
    let mut messages = Vec::new();
    for (id, event_type, payload) in newest_first {
        match event_type.as_str() {
            "ResponseFailed" if is_start_failure(payload) => {}
            "MessageReceived" if is_human(payload) => {
                let text = payload.get("text").and_then(|t| t.as_str()).unwrap_or("");
                messages.push((*id, text, payload));
            }
            _ => break,
        }
    }
    messages.reverse();
    joined(messages.into_iter())
}

/// One input from `inputs` (id, text, payload), oldest first, or `None` when
/// there are none.
fn joined<'a>(
    inputs: impl Iterator<Item = (Uuid, &'a str, &'a serde_json::Value)>,
) -> Option<UndeliveredInput> {
    let mut texts = Vec::new();
    let mut image_hashes = Vec::new();
    let mut event_ids = Vec::new();
    for (id, text, payload) in inputs {
        event_ids.push(id);
        if !text.is_empty() {
            texts.push(text);
        }
        image_hashes.extend(
            payload
                .get("user_image_hashes")
                .and_then(|h| h.as_array())
                .into_iter()
                .flatten()
                .filter_map(|h| h.as_str().map(str::to_string)),
        );
    }
    if event_ids.is_empty() {
        return None;
    }
    Some(UndeliveredInput {
        text: texts.join("\n\n"),
        image_hashes,
        event_ids,
    })
}

fn is_start_failure(payload: &serde_json::Value) -> bool {
    payload["agent_exit"]["stage"] == "startup"
}

fn is_human(payload: &serde_json::Value) -> bool {
    payload
        .get("mode")
        .and_then(|m| m.as_str())
        .unwrap_or("human")
        == "human"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(event_type: &str, payload: serde_json::Value) -> Row {
        (Uuid::new_v4(), event_type.to_string(), payload)
    }

    fn start_failure() -> Row {
        event(
            "ResponseFailed",
            json!({ "error": "no login", "agent_exit": { "stage": "startup", "code": 1 } }),
        )
    }

    fn user_says(text: &str) -> Row {
        event("MessageReceived", json!({ "text": text, "mode": "human" }))
    }

    #[test]
    fn the_message_a_start_failure_never_delivered_comes_back() {
        let message = user_says("add prompt caching");
        let newest_first = vec![start_failure(), message.clone()];
        assert_eq!(
            undelivered_input_from(&newest_first),
            Some(UndeliveredInput {
                text: "add prompt caching".into(),
                image_hashes: vec![],
                event_ids: vec![message.0],
            })
        );
    }

    #[test]
    fn it_survives_a_second_start_failure_on_a_retry() {
        let newest_first = vec![start_failure(), start_failure(), user_says("first request")];
        assert_eq!(
            undelivered_input_from(&newest_first).map(|u| u.text),
            Some("first request".into())
        );
    }

    #[test]
    fn several_messages_come_back_in_the_order_they_were_sent() {
        let newest_first = vec![user_says("second"), start_failure(), user_says("first")];
        assert_eq!(
            undelivered_input_from(&newest_first).map(|u| u.text),
            Some("first\n\nsecond".into())
        );
    }

    #[test]
    fn images_ride_with_their_messages() {
        let newest_first = vec![
            start_failure(),
            event(
                "MessageReceived",
                json!({ "text": "look", "mode": "human", "user_image_hashes": ["h1", "h2"] }),
            ),
        ];
        assert_eq!(
            undelivered_input_from(&newest_first).map(|u| u.image_hashes),
            Some(vec!["h1".to_string(), "h2".to_string()])
        );
    }

    #[test]
    fn a_user_stop_ends_the_search() {
        let newest_first = vec![
            event("ResponseCanceled", json!({ "cause": "user_stop" })),
            user_says("stopped on purpose"),
        ];
        assert_eq!(undelivered_input_from(&newest_first), None);
    }

    #[test]
    fn a_message_the_agent_read_is_not_owed() {
        let newest_first = vec![
            start_failure(),
            event("CodingAgentInputRead", json!({})),
            user_says("already read"),
        ];
        assert_eq!(undelivered_input_from(&newest_first), None);
    }

    #[test]
    fn a_finished_turn_ends_the_search() {
        let newest_first = vec![
            start_failure(),
            event("ResponseGenerated", json!({ "text": "done" })),
            user_says("answered long ago"),
        ];
        assert_eq!(undelivered_input_from(&newest_first), None);
    }

    #[test]
    fn a_failure_that_is_not_a_start_failure_ends_the_search() {
        let newest_first = vec![
            event(
                "ResponseFailed",
                json!({ "error": "x", "agent_exit": { "stage": "mid_turn", "code": 1 } }),
            ),
            user_says("was read"),
        ];
        assert_eq!(undelivered_input_from(&newest_first), None);
    }

    #[test]
    fn an_old_failure_with_no_agent_exit_ends_the_search() {
        let newest_first = vec![
            event("ResponseFailed", json!({ "error": "old" })),
            user_says("was read"),
        ];
        assert_eq!(undelivered_input_from(&newest_first), None);
    }

    #[test]
    fn an_engine_authored_message_is_not_the_users() {
        let newest_first = vec![
            start_failure(),
            event(
                "MessageReceived",
                json!({ "text": "Your changes have not been hardened.", "mode": "engine" }),
            ),
        ];
        assert_eq!(undelivered_input_from(&newest_first), None);
    }

    #[test]
    fn a_thread_with_no_failure_owes_nothing() {
        assert_eq!(undelivered_input_from(&[]), None);
    }
}
