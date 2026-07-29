use serde::{Deserialize, Serialize};

/// One option offered by CC's AskUserQuestion tool. Persisted inside
/// `UserQuestionAsked` and looked up when the user picks one to send the
/// matching `tool_result` back to CC.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// How the user answered a `UserQuestionAsked`. Tagged so the JSON payload
/// is `{ "kind": "Selected", "option_id": "..." }` etc.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum AnswerKind {
    Selected {
        option_id: String,
    },
    FreeText {
        text: String,
    },
    /// Multi-select answer. `text` carries optional freetext typed alongside
    /// the toggled options — the prompt textarea folds into the answer when a
    /// multi-select question is pending. Backend joins the resolved labels and
    /// the freetext together when relaying to CC. Either side may be empty
    /// (but not both — see `validate_answer`).
    MultiSelected {
        option_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    Canceled,
}
