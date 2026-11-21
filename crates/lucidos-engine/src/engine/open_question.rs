//! Read the question a thread's agent is waiting on.

use uuid::Uuid;

use super::permission::PermissionRequest;
use super::question_park_sql::unanswered_question_predicate_sql;
use super::thread_events::QuestionOption;

/// A question a thread is parked on, as a reader of it needs it.
///
/// The event carries resume plumbing beside these four: a Claude Code session
/// id and a worktree path. Neither means anything to somebody being asked the
/// question, so neither is here.
///
/// `tool_use_id` is here because it is what ANSWERS the question:
/// `agent_question::answer_pending_question` takes it, and a reader that could
/// read a question aloud but not settle it would be half a route.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenQuestion {
    pub tool_use_id: String,
    pub question: String,
    pub options: Vec<QuestionOption>,
    pub multi_select: bool,
    /// A *permission request* (ADR 0664): read back, settled only by the
    /// caller's own words.
    pub permission_request: Option<OpenRequest>,
}

/// A permission request a thread is parked on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenRequest {
    /// An agent prepared it, so the agent's question tool waits on the answer.
    pub awaited: bool,
    /// The owner's words can answer it. See [`PermissionRequest::words_answer`].
    pub words_answer: bool,
}

impl OpenRequest {
    /// The request a `UserQuestionAsked` payload carries, if any.
    pub fn of(payload: &serde_json::Value) -> Option<Self> {
        let request = payload.get("permission_request").filter(|r| !r.is_null())?;
        let has = |key: &str| request.get(key).is_some_and(|v| !v.is_null());
        Some(Self {
            awaited: has("prepared"),
            words_answer: !has("prepared")
                && !has("unattested_act")
                && request["choices"]
                    .as_object()
                    .is_some_and(|choices| !choices.is_empty()),
        })
    }
}

impl From<&PermissionRequest> for OpenRequest {
    fn from(request: &PermissionRequest) -> Self {
        Self {
            awaited: request.prepared.is_some(),
            words_answer: request.words_answer(),
        }
    }
}

/// The question this thread is parked on, or `None` when it is not parked.
///
/// The row half of [`super::question_park_sql::thread_has_unanswered_question`], built from the same
/// predicate. The bool answers the restart preserve guard. This answers a
/// *voice session* opening on a thread whose agent is waiting on a person: the
/// talker holds no tools, so a question absent from its opening block is one
/// it cannot go and look up (ADR 0149).
///
/// Newest first, so a thread carrying more than one live card reads out the
/// one the reader is looking at.
///
/// A read error, a missing question or a blank one all yield `None`. A call
/// that opens knowing less beats a call that does not open. A question with no
/// `tool_use_id` yields `None` too: nothing can answer it, here or anywhere
/// else, so reading it aloud would only offer the caller a dead end.
pub async fn newest_open_question(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<OpenQuestion> {
    let sql = format!(
        "SELECT uqa.payload FROM events uqa WHERE {} \
         ORDER BY uqa.sequence DESC LIMIT 1",
        unanswered_question_predicate_sql("$1"),
    );
    let payload = match sqlx::query_scalar::<_, serde_json::Value>(&sql)
        .bind(thread_id.to_string())
        .fetch_optional(pool)
        .await
    {
        Ok(payload) => payload?,
        Err(e) => {
            log_legacy!(
                "[Recovery] Could not read {}'s open question: {}",
                thread_id,
                e
            );
            return None;
        }
    };

    let question = payload.get("question")?.as_str()?.trim();
    if question.is_empty() {
        return None;
    }
    let tool_use_id = payload.get("tool_use_id")?.as_str()?.trim();
    if tool_use_id.is_empty() {
        return None;
    }
    let options = payload
        .get("options")
        .cloned()
        .and_then(|v| serde_json::from_value::<Vec<QuestionOption>>(v).ok())
        .unwrap_or_default();
    Some(OpenQuestion {
        tool_use_id: tool_use_id.to_string(),
        question: question.to_string(),
        options,
        multi_select: payload
            .get("multi_select")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        permission_request: OpenRequest::of(&payload),
    })
}
