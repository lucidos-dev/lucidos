//! The message a start failure leaves unread.
//!
//! A coding agent whose process exits before its session began never read the
//! user's message. The card offers Try again, which is the existing Continue.
//! A bare "Continue from where you left off" would then ask the agent to resume
//! work it was never told about. So Try again carries the message itself.

use uuid::Uuid;

/// What a coding agent was owed and never read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UndeliveredInput {
    pub text: String,
    pub image_hashes: Vec<String>,
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

/// How far back the search reads. A start failure sits at the end of the
/// thread, so a few events cover it.
const SEARCH_WINDOW: i64 = 100;

/// The human messages the thread's latest start failures left unread, oldest
/// first, or `None` when the agent read everything it was sent.
///
/// Reads backward from the newest event. A start failure and a human message are
/// skipped or collected. Anything else (another terminal, or the agent reading an
/// input) means the agent saw everything before it, so the search ends there.
pub(crate) async fn undelivered_input_after_start_failure(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Option<UndeliveredInput> {
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT event_type, payload FROM events \
          WHERE thread_id = $1 AND event_type = ANY($2::text[]) \
          ORDER BY sequence DESC LIMIT $3",
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

/// [`undelivered_input_after_start_failure`] over events, newest first.
fn undelivered_input_from(
    newest_first: &[(String, serde_json::Value)],
) -> Option<UndeliveredInput> {
    let mut texts = Vec::new();
    let mut image_hashes = Vec::new();
    for (event_type, payload) in newest_first {
        match event_type.as_str() {
            "ResponseFailed" if is_start_failure(payload) => {}
            "MessageReceived" if is_human(payload) => {
                if let Some(text) = payload.get("text").and_then(|t| t.as_str()) {
                    texts.push(text.to_string());
                }
                let hashes = payload
                    .get("user_image_hashes")
                    .and_then(|h| h.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|h| h.as_str().map(str::to_string));
                image_hashes.splice(0..0, hashes);
            }
            _ => break,
        }
    }
    if texts.is_empty() {
        return None;
    }
    texts.reverse();
    Some(UndeliveredInput {
        text: texts.join("\n\n"),
        image_hashes,
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

    fn event(event_type: &str, payload: serde_json::Value) -> (String, serde_json::Value) {
        (event_type.to_string(), payload)
    }

    fn start_failure() -> (String, serde_json::Value) {
        event(
            "ResponseFailed",
            json!({ "error": "no login", "agent_exit": { "stage": "startup", "code": 1 } }),
        )
    }

    fn user_says(text: &str) -> (String, serde_json::Value) {
        event("MessageReceived", json!({ "text": text, "mode": "human" }))
    }

    #[test]
    fn the_message_a_start_failure_never_delivered_comes_back() {
        let newest_first = vec![start_failure(), user_says("add prompt caching")];
        assert_eq!(
            undelivered_input_from(&newest_first),
            Some(UndeliveredInput {
                text: "add prompt caching".into(),
                image_hashes: vec![],
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
