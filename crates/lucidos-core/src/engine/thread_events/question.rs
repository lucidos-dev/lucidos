use serde::{Deserialize, Serialize};

use crate::engine::thread_events::permission::Action;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widget: Option<WidgetEmbed>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub quiet: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_reply: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WidgetEmbed {
    pub app_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum AnswerKind {
    Selected {
        option_id: String,
    },
    FreeText {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        image_hashes: Vec<String>,
    },
    MultiSelected {
        option_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        image_hashes: Vec<String>,
    },
    Canceled,
    /// INVARIANT: A follow-up arrived that could not be the answer, so it replaced the
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnerApproval {
    pub verb: Action,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_thread_id: Option<uuid::Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_handle: Option<String>,
}

impl AnswerKind {
    pub fn is_a_reply(&self) -> bool {
        !matches!(self, AnswerKind::Canceled | AnswerKind::Superseded)
    }

    pub fn image_hashes(&self) -> &[String] {
        match self {
            AnswerKind::FreeText { image_hashes, .. }
            | AnswerKind::MultiSelected { image_hashes, .. } => image_hashes,
            AnswerKind::Selected { .. } | AnswerKind::Canceled | AnswerKind::Superseded => &[],
        }
    }

    pub fn text(&self, options: &[QuestionOption]) -> Option<String> {
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

pub fn with_image_note(text: &str, images: usize) -> String {
    match images {
        0 => text.to_string(),
        1 => format!("{text} [1 image]").trim().to_string(),
        n => format!("{text} [{n} images]").trim().to_string(),
    }
}

pub fn composer_submission(answer: &AnswerKind) -> Option<(String, Vec<String>)> {
    let text = match answer {
        AnswerKind::FreeText { text, .. } => text.clone(),
        AnswerKind::MultiSelected { text, .. } => text.clone().unwrap_or_default(),
        _ => return None,
    };
    let image_hashes = answer.image_hashes().to_vec();
    (!text.trim().is_empty() || !image_hashes.is_empty()).then_some((text, image_hashes))
}
