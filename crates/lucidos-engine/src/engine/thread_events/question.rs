use serde::{Deserialize, Serialize};

use crate::api::thread_reach::ThreadReachVerb;

/// One option offered by CC's AskUserQuestion tool. Persisted inside
/// `UserQuestionAsked` and looked up when the user picks one to send the
/// matching `tool_result` back to CC.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Markdown the card shows under the option: a picture, or a short text
    /// sample. Claude Code's native tool names it `preview`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// The widget the option draws under its label row, lifted out of a
    /// *widget embed* in its description or preview (ADR 0415).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widget: Option<WidgetEmbed>,
    /// A *quiet option*: picking it records the answer and ends the turn
    /// without waking the agent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub quiet: bool,
    /// The *quiet reply*: the fixed acknowledgement the thread shows when the
    /// user picks this quiet option. Only a quiet option carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_reply: Option<String>,
}

/// A *widget embed*, `![label](app:<id>?params={...})`, as the engine reads
/// it: the widget, its params, and the alt text as its label.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WidgetEmbed {
    pub app_id: String,
    /// The instance's *widget params*. Absent means none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    /// The embed's alt text. Absent means the widget's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// How the user answered a `UserQuestionAsked`. Tagged so the JSON payload
/// is `{ "kind": "Selected", "option_id": "..." }` etc.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum AnswerKind {
    Selected {
        option_id: String,
    },
    /// A reply typed in the composer. `image_hashes` names the blobs attached
    /// to it, and `text` may be empty when the images are the answer.
    FreeText {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        image_hashes: Vec<String>,
    },
    /// Multi-select answer. `text` carries optional freetext typed alongside
    /// the toggled options. The prompt textarea folds into the answer when a
    /// multi-select question is pending, images included. Backend joins the
    /// resolved labels and the freetext together when relaying to CC. Some
    /// side must be present: see `validate_answer`.
    MultiSelected {
        option_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        image_hashes: Vec<String>,
    },
    Canceled,
    /// A follow-up arrived that could not be the answer, so it replaced the
    /// question instead. Two shapes reach here, both on coding-agent threads.
    /// An agent-driven message (a parent's instruction, a child-completion
    /// wake) is refused by the `mode == Human` guard. So is any message landing
    /// on a question the agent already overtook. Resolving it is what unblocks
    /// the parked agent, which cannot read the follow-up until its question
    /// call returns.
    ///
    /// Distinct from `Canceled`, which means the question was torn down and
    /// nothing is coming. Here the user did reply, just not to this question,
    /// and the reply drives the very next turn.
    Superseded,
}

/// The one clause-4 act an *owner approval card* proposes (ADR 0387).
///
/// Set on a `UserQuestionAsked` only by the internal ask route, when the agent
/// asks with an `OwnerApprovalRequested` id. The engine then writes the card's
/// question and options. Its presence is what makes the card's Allow spendable.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnerApproval {
    pub verb: ThreadReachVerb,
    /// The thread the act aims at. `None` only on a card asked for a
    /// top-thread before that needed no approval (ADR 0475).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_thread_id: Option<uuid::Uuid>,
}

impl AnswerKind {
    /// The blobs attached to this answer. Only a typed answer carries any.
    pub fn image_hashes(&self) -> &[String] {
        match self {
            AnswerKind::FreeText { image_hashes, .. }
            | AnswerKind::MultiSelected { image_hashes, .. } => image_hashes,
            AnswerKind::Selected { .. } | AnswerKind::Canceled | AnswerKind::Superseded => &[],
        }
    }

    /// What the user answered, as text: each picked option's label from
    /// `options`, then any typed text. `None` when no answer came.
    pub(crate) fn text(&self, options: &[QuestionOption]) -> Option<String> {
        let label = |option_id: &str| -> String {
            options
                .iter()
                .find(|o| o.id == option_id)
                .map_or_else(|| option_id.to_string(), |o| o.label.clone())
        };
        match self {
            AnswerKind::Selected { option_id } => Some(label(option_id)),
            AnswerKind::FreeText { text, image_hashes } => {
                Some(with_image_note(text, image_hashes.len()))
            }
            AnswerKind::MultiSelected {
                option_ids,
                text,
                image_hashes,
            } => {
                let mut parts: Vec<String> = option_ids.iter().map(|o| label(o)).collect();
                if let Some(text) = text.as_deref().filter(|t| !t.trim().is_empty()) {
                    parts.push(text.to_string());
                }
                Some(with_image_note(&parts.join("; "), image_hashes.len()))
            }
            AnswerKind::Canceled | AnswerKind::Superseded => None,
        }
    }
}

/// A message's text, noting attached images so an image-only message still
/// reads as something.
pub(crate) fn with_image_note(text: &str, images: usize) -> String {
    match images {
        0 => text.to_string(),
        1 => format!("{text} [1 image]").trim().to_string(),
        n => format!("{text} [{n} images]").trim().to_string(),
    }
}
