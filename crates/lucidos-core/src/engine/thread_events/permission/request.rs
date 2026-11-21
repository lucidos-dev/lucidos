//! Word and append an engine-owned permission card.

use uuid::Uuid;

use super::batch;
use super::{Action, PermissionChoice, PermissionRequest, PreparedCard, PreparedOption};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Every permission request's card id starts with this, so a log line names
/// what raised it.
pub const CARD_ID_PREFIX: &str = "permission-request:";

/// One option the engine words for a request.
pub struct RequestOption {
    pub label: String,
    pub description: String,
    pub choice: PermissionChoice,
}

/// What the engine asks: its words, and the typed request behind them.
pub struct RequestCard {
    pub question: String,
    pub header: &'static str,
    pub action: Action,
    pub breadth: Option<String>,
    pub options: Vec<RequestOption>,
    pub batch: Option<batch::Batch>,
}

/// The options the question parser makes of this card, with their ids.
pub fn parsed_options(card: &PreparedCard) -> Vec<crate::engine::thread_events::QuestionOption> {
    crate::engine::question_parsing::parse_ask_user_question_inputs(
        &serde_json::json!({ "questions": card.questions() }),
    )
    .remove(0)
    .options
}

impl RequestCard {
    /// Split the card into its words and its typed request. Each choice is
    /// keyed by the id the question parser gives its option.
    pub fn into_parts(self) -> (PreparedCard, PermissionRequest) {
        let words = PreparedCard {
            question: self.question,
            header: self.header.to_string(),
            options: self
                .options
                .iter()
                .map(|o| PreparedOption {
                    label: o.label.clone(),
                    description: o.description.clone(),
                })
                .collect(),
        };
        let choices = parsed_options(&words)
            .into_iter()
            .zip(self.options)
            .map(|(option, spec)| (option.id, spec.choice))
            .collect();
        let request = PermissionRequest {
            action: self.action,
            breadth: self.breadth,
            prepared: None,
            choices,
            batch: self.batch,
            lines: None,
        };
        (words, request)
    }
}

/// Raise `card` on `thread_id` and return its id. The engine wrote every word,
/// so no agent text reaches an option.
pub async fn raise_request(
    bus: &EventBus,
    thread_id: Uuid,
    card: RequestCard,
    channel: EventChannel,
) -> Result<String, BoxError> {
    let tool_use_id = format!("{CARD_ID_PREFIX}{}", Uuid::new_v4());
    let (words, request) = card.into_parts();
    let options = parsed_options(&words);
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.clone(),
            cc_session_id: String::new(),
            question: words.question,
            options,
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            permission_request: Some(request),
        },
        meta: EventMeta {
            channel: Some(channel),
            ..EventMeta::NONE
        },
    })
    .await?;
    Ok(tool_use_id)
}
