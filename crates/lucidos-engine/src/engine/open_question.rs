use uuid::Uuid;

use super::permission::PermissionRequest;
use super::question_park_sql::open_posted_card_sql;
use super::thread_events::QuestionOption;

#[derive(Debug, Clone, PartialEq)]
pub struct OpenQuestion {
    pub tool_use_id: String,
    pub question: String,
    pub options: Vec<QuestionOption>,
    pub multi_select: bool,
    pub permission_request: Option<OpenRequest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenRequest {
    pub awaited: bool,
    pub words_answer: bool,
}

impl OpenRequest {
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

pub async fn open_question(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<OpenQuestion> {
    let sql = format!(
        "SELECT uqa.payload FROM events uqa WHERE {} \
         ORDER BY uqa.sequence LIMIT 1",
        open_posted_card_sql("uqa", "$1"),
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
