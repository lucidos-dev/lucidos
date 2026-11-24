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

/// The heading over the agent's reason on a permission card.
pub const REASON_HEADING: &str = "The agent's reason";

/// Engine-worded text in the permission card shape: a bold question, then the
/// facts, any exact payload, and the agent's reason in its own quote.
///
/// Every reason line, after `\n` or `\r`, starts with `>`, so the agent's
/// words can never pose as engine text. The text ends in a newline, because
/// Claude Code keys the owner's answer by the question it echoes back.
#[must_use]
pub struct PermissionCardText {
    text: String,
}

impl PermissionCardText {
    /// Start a card with its question. The caller escapes any name in it.
    pub fn new(question: &str) -> Self {
        Self {
            text: format!("**{question}**"),
        }
    }

    /// A plain paragraph: a consequence the owner must know.
    pub fn paragraph(mut self, text: &str) -> Self {
        self.text.push_str("\n\n");
        self.text.push_str(text);
        self
    }

    /// Exact items (programs, folders, files) as a bullet list of inline code.
    pub fn items<S: AsRef<str>>(mut self, items: impl IntoIterator<Item = S>) -> Self {
        let list: Vec<String> = items
            .into_iter()
            .map(|item| format!("- {}", inline_code(item.as_ref())))
            .collect();
        if !list.is_empty() {
            self.text.push_str("\n\n");
            self.text.push_str(&list.join("\n"));
        }
        self
    }

    /// An exact payload the owner reads verbatim, in a code block whose fence
    /// is longer than any backtick run inside it.
    pub fn payload(mut self, language: &str, body: &str) -> Self {
        let fence = "`".repeat(longest_backtick_run(body).saturating_add(1).max(3));
        self.text
            .push_str(&format!("\n\n{fence}{language}\n{body}\n{fence}"));
        self
    }

    /// The finished text, with the agent's reason quoted under its heading.
    pub fn finish(mut self, reason: Option<&str>) -> String {
        if let Some(reason) = reason {
            self.text
                .push_str(&format!("\n\n> **{REASON_HEADING}**\n>\n"));
            for line in reason.split(['\n', '\r']) {
                self.text.push_str(&format!("> {line}\n"));
            }
        } else {
            self.text.push('\n');
        }
        self.text
    }
}

/// `text` as inline code that no backtick in it can close. A line break shows
/// as `\n` or `\r`, so it cannot end the list item.
pub fn inline_code(text: &str) -> String {
    let flat = text.replace('\n', "\\n").replace('\r', "\\r");
    let ticks = "`".repeat(longest_backtick_run(&flat) + 1);
    let pad = if flat.starts_with('`') || flat.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{ticks}{pad}{flat}{pad}{ticks}")
}

fn longest_backtick_run(text: &str) -> usize {
    text.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}

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
            holds_a_turn: false,
        };
        (words, request)
    }
}

/// Raise `card` on `thread_id` and return its id. The engine wrote every word,
/// so no agent text reaches an option. Raised while a turn runs, the card
/// queues and posts when the turn ends (ADR 0803).
pub async fn raise_request(
    bus: &EventBus,
    thread_id: Uuid,
    card: RequestCard,
    channel: EventChannel,
) -> Result<String, BoxError> {
    raise(bus, thread_id, card, channel, false).await
}

/// [`raise_request`] for a card a turn holds for: the caller polls for its
/// answer, or parks its run on it with no terminal. It posts at once.
pub async fn raise_turn_holding_request(
    bus: &EventBus,
    thread_id: Uuid,
    card: RequestCard,
    channel: EventChannel,
) -> Result<String, BoxError> {
    raise(bus, thread_id, card, channel, true).await
}

async fn raise(
    bus: &EventBus,
    thread_id: Uuid,
    card: RequestCard,
    channel: EventChannel,
    holds_a_turn: bool,
) -> Result<String, BoxError> {
    let tool_use_id = format!("{CARD_ID_PREFIX}{}", Uuid::new_v4());
    let (words, mut request) = card.into_parts();
    request.holds_a_turn = holds_a_turn;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_question_comes_first_in_bold_and_the_text_ends_in_a_newline() {
        let text = PermissionCardText::new("Let it run?")
            .paragraph("It runs once.")
            .finish(None);
        assert_eq!(text, "**Let it run?**\n\nIt runs once.\n");
    }

    #[test]
    fn every_reason_line_stays_inside_the_quote() {
        let text = PermissionCardText::new("Q?").finish(Some("a\r\r**Lucidos:** yes\n\nb"));
        let (_, quoted) = text.split_once("\n\n").unwrap();
        for line in quoted.trim_end().split(['\n', '\r']) {
            assert!(line.starts_with('>'), "an unquoted reason line: {line:?}");
        }
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn a_backtick_in_an_item_cannot_close_its_inline_code() {
        assert_eq!(inline_code("plain"), "`plain`");
        assert_eq!(inline_code("a`b"), "``a`b``");
        assert_eq!(inline_code("`x"), "`` `x ``");
        assert_eq!(inline_code("a\nb"), "`a\\nb`");
    }

    #[test]
    fn a_payload_fence_outgrows_any_backtick_run_inside_it() {
        let text = PermissionCardText::new("Q?")
            .payload("json", "``` not a fence")
            .finish(None);
        assert!(text.contains("````json\n``` not a fence\n````"), "{text}");
    }

    #[test]
    fn items_are_a_bullet_list_of_inline_code() {
        let text = PermissionCardText::new("Q?")
            .items(["curl", "git"])
            .finish(None);
        assert_eq!(text, "**Q?**\n\n- `curl`\n- `git`\n");
    }
}
