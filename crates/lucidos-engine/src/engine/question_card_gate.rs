//! Refuses a question card once when the agent owes the user an answer first.
//!
//! The user reads only the agent's text and its cards, never a tool result or
//! the agent's reasoning. So a card may not be the first thing they get after
//! a reply they typed into a card, or after tool work nobody reported. One
//! rule for the chat agent, Claude Code and Codex, read from the thread's
//! events so every agent sees the same facts.
//!
//! A model-tolerance measure: `docs/temporary-measures.md` § "A question card
//! with no answer before it". Plan:
//! `docs/plans/2026-09-23-a-card-never-replaces-the-answer.md`.

use sqlx::PgPool;
use uuid::Uuid;

/// Starts every refusal, and is how the query knows one was already sent.
const REFUSAL_MARKER: &str = "Question card not shown.";

/// The tool result a refused card gets in place of the user's answer. Every
/// agent's notes between tool calls arrive as text, so it asks for prose.
pub(crate) const CARD_REFUSAL: &str = "Question card not shown. Since the user's last input you \
     have written them nothing. They read only your text and your cards, never your tool results \
     or your reasoning. If they asked something, answer it in plain prose now. If you ran tools, \
     say what you found. Then ask your question again. If there is truly nothing to say, send \
     the same question again unchanged: it will not be refused twice.";

/// Tools whose calls are not work the user needs to hear about: the question
/// tools themselves, and the todo list.
fn not_work() -> Vec<&'static str> {
    let mut names = vec![
        crate::llm::tool_names::ASK_USER_QUESTION,
        crate::llm::tool_names::TODO_WRITE,
        "TodoWrite",
    ];
    names.extend(crate::runtime::USER_QUESTION_TOOLS);
    names
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LastInput {
    /// Nothing the user said is on the thread yet.
    None,
    /// A message or an injected follow-up.
    Message,
    /// A card answer that only picked options.
    Picked,
    /// A card answer carrying text the user typed.
    Typed,
}

impl LastInput {
    fn from_answer(answer: &serde_json::Value) -> Self {
        let typed_text = answer
            .get("text")
            .and_then(|t| t.as_str())
            .is_some_and(|t| !t.trim().is_empty());
        match answer.get("kind").and_then(|k| k.as_str()) {
            Some("FreeText" | "MultiSelected") if typed_text => Self::Typed,
            _ => Self::Picked,
        }
    }
}

/// What happened on the thread since the user's last input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SinceLastInput {
    pub(crate) last_input: LastInput,
    pub(crate) spoke: bool,
    /// Tool calls after the agent's last words, or after the input if it said
    /// nothing. "Let me check the log" reports nothing the log said.
    pub(crate) unreported_tool_calls: i64,
    pub(crate) refused: bool,
}

/// Refuse when a typed reply got no words back, or tool work went
/// unreported, unless a card was already refused since that input.
pub(crate) fn should_refuse(s: SinceLastInput) -> bool {
    let unanswered = s.last_input == LastInput::Typed && !s.spoke;
    !s.refused && (unanswered || s.unreported_tool_calls > 0)
}

/// Whether to refuse the card `tool_use_id` is about to raise on `thread_id`.
/// A card already shown always passes, so a crash-recovery re-POST is safe.
/// A query error shows the card: blocking a question on a DB blip helps nobody.
pub(crate) async fn refuse_card(pool: &PgPool, thread_id: Uuid, tool_use_id: &str) -> bool {
    match read_since_last_input(pool, thread_id, tool_use_id).await {
        Ok(Some(since)) => should_refuse(since),
        Ok(None) => false,
        Err(e) => {
            crate::log!(
                "[QuestionCardGate] thread={thread_id} query failed, showing the card: {e}"
            );
            false
        }
    }
}

/// How long a coding agent's refusal waits before its second look.
const SECOND_LOOK: std::time::Duration = std::time::Duration::from_millis(500);

/// [`refuse_card`] for a coding agent's hook, which races the session loop
/// persisting the text written just before the card. A refusal needs two
/// looks, so a card that followed real text is never refused.
pub(crate) async fn refuse_coding_agent_card(
    pool: &PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
) -> bool {
    if !refuse_card(pool, thread_id, tool_use_id).await {
        return false;
    }
    tokio::time::sleep(SECOND_LOOK).await;
    refuse_card(pool, thread_id, tool_use_id).await
}

/// `None` when the card was already shown.
async fn read_since_last_input(
    pool: &PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<Option<SinceLastInput>, sqlx::Error> {
    let row: (bool, Option<String>, Option<serde_json::Value>, bool, i64, bool) =
        sqlx::query_as(
            "WITH last_input AS ( \
               SELECT sequence, event_type, payload->'answer' AS answer FROM events \
               WHERE thread_id = $1 \
                 AND event_type IN ('MessageReceived', 'UserPromptInjected', 'UserQuestionAnswered') \
               ORDER BY sequence DESC LIMIT 1 \
             ), since AS ( \
               SELECT sequence, event_type, payload FROM events \
               WHERE thread_id = $1 \
                 AND sequence > COALESCE((SELECT sequence FROM last_input), 0) \
                 AND event_type IN ('TextStreamed', 'CodingAgentTextStreamed', 'ToolCalled', \
                   'CodingAgentToolCalled', 'ToolResult', 'CodingAgentToolResult') \
             ), last_words AS ( \
               SELECT MAX(sequence) AS sequence FROM since \
               WHERE event_type IN ('TextStreamed', 'CodingAgentTextStreamed') \
                 AND btrim(payload->>'text', E' \\t\\r\\n') <> '' \
             ) \
             SELECT \
               EXISTS (SELECT 1 FROM events WHERE thread_id = $1 \
                 AND event_type = 'UserQuestionAsked' \
                 AND starts_with(payload->>'tool_use_id', $2 || '#')), \
               (SELECT event_type FROM last_input), \
               (SELECT answer FROM last_input), \
               (SELECT sequence FROM last_words) IS NOT NULL, \
               (SELECT COUNT(*) FROM since \
                 WHERE event_type IN ('ToolCalled', 'CodingAgentToolCalled') \
                   AND NOT (payload->>'name' = ANY($3)) \
                   AND sequence > COALESCE((SELECT sequence FROM last_words), 0)), \
               EXISTS (SELECT 1 FROM since \
                 WHERE event_type IN ('ToolResult', 'CodingAgentToolResult') \
                   AND strpos(payload->>'result', $4) > 0)",
        )
        .bind(thread_id)
        .bind(tool_use_id)
        .bind(not_work())
        .bind(REFUSAL_MARKER)
        .fetch_one(pool)
        .await?;
    let (already_shown, input_type, answer, spoke, unreported_tool_calls, refused) = row;
    if already_shown {
        return Ok(None);
    }
    let last_input = match (input_type.as_deref(), answer) {
        (None, _) => LastInput::None,
        (Some("UserQuestionAnswered"), Some(answer)) => LastInput::from_answer(&answer),
        _ => LastInput::Message,
    };
    Ok(Some(SinceLastInput {
        last_input,
        spoke,
        unreported_tool_calls,
        refused,
    }))
}

#[cfg(test)]
#[path = "question_card_gate_tests.rs"]
mod tests;
