use serde::{Deserialize, Serialize};

use crate::api::thread_reach::ThreadReachVerb;

/// One option offered by CC's AskUserQuestion tool. Persisted inside
/// `UserQuestionAsked` and looked up when the user picks one to send the
/// matching `tool_result` back to CC.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Markdown the card shows under the option: a picture, or a short text
    /// sample. Claude Code's native tool names it `preview`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
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
    /// The thread the act aims at. `None` only for a verb that aims at the
    /// workspace root (`ThreadReachVerb::aims_at_root`).
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
}
