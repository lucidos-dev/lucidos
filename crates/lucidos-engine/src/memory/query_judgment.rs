//! Query classification as three typed yes/no questions.
//!
//! The Jev half of `MemoryExtractor::classify_query`. It answers the three
//! booleans and nothing else: `sub_queries` is generation, so it stays on the
//! chat model and the extractor runs it only when memory is wanted.
//!
//! Everything here is pure, so the questions, the state and the thresholding
//! are tested without a pool or a network.

use serde_json::{json, Value};

use crate::llm::judgment::{Answers, NoulCriteria, Question};
use crate::memory::QueryClassification;

pub(crate) const NEEDS_MEMORY: &str = "needs_memory";
pub(crate) const NEEDS_FILE_LIST: &str = "needs_file_list";
pub(crate) const NEEDS_CREDENTIALS: &str = "needs_credentials";

/// The probability at or above which an answer reads as yes.
///
/// Below 0.5 on purpose, because the two mistakes do not cost the same. A
/// missing retrieval costs the user an answer that ignores what they asked
/// about. A needless one costs some latency and some tokens. So a split
/// answer loads, and only a confident no skips.
///
/// A starting value, pinned by the tests rather than fitted to data. Tuning it
/// needs real traffic through a live key.
pub(crate) const YES_THRESHOLD: f64 = 0.35;

/// What the model reads. Named fields, because the message and the
/// conversation are two different things and the questions name them.
pub(crate) fn state(message: &str, conversation_context: Option<&str>) -> Value {
    match conversation_context.filter(|c| !c.is_empty()) {
        Some(context) => json!({ "message": message, "conversation": context }),
        None => json!({ "message": message }),
    }
}

/// The three questions, asked together over one state.
///
/// They are independent, so one request answers all three in parallel. Each
/// carries its whole meaning, because the id never reaches the model.
pub(crate) fn questions() -> Vec<(String, Question)> {
    vec![
        (
            NEEDS_MEMORY.to_string(),
            Question::Noul {
                instructions: "Does answering `message` need the user's long-term memory: \
                     past conversations, projects, preferences or personal facts? Read \
                     `conversation` for the topic under discussion, not just the latest \
                     message."
                    .to_string(),
                criteria: Some(NoulCriteria {
                    yes: "The message refers to past conversations, earlier today, what we \
                          discussed, the research, last time, or any prior interaction. It \
                          counts even when the request also asks for a tool action, such as \
                          saving that content to a file."
                        .to_string(),
                    no: "A greeting, a time query, general knowledge, or a pure tool command \
                         referring to no past content, such as creating an empty file."
                        .to_string(),
                }),
            },
        ),
        (
            NEEDS_FILE_LIST.to_string(),
            Question::Noul {
                instructions: "Does answering `message` need to know which files exist in the \
                     workspace?"
                    .to_string(),
                criteria: Some(NoulCriteria {
                    yes: "The answer depends on what is on disk: naming, finding, listing, \
                          opening or editing a workspace file."
                        .to_string(),
                    no: "A greeting, a time query, or a question answered from memory alone."
                        .to_string(),
                }),
            },
        ),
        (
            NEEDS_CREDENTIALS.to_string(),
            Question::Noul {
                instructions: "Does this conversation involve external APIs, tokens, \
                     authentication or services that might need credentials? Judge the whole \
                     topic in `conversation`, not only the latest message."
                    .to_string(),
                criteria: Some(NoulCriteria {
                    yes: "The topic touches an external service, an API call, a token or a \
                          sign-in. A follow-up like \"try again\" inside such a conversation \
                          still counts."
                        .to_string(),
                    no: "Nothing in the topic reaches outside the workspace.".to_string(),
                }),
            },
        ),
    ]
}

/// Read the three answers, leaving `sub_queries` for the caller to fill.
///
/// **An answer that did not arrive reads as yes.** That matches
/// [`QueryClassification::default`], which loads everything, and it keeps a
/// dropped answer from quietly starving the turn of context.
pub(crate) fn read(answers: &Answers) -> QueryClassification {
    let yes = |id: &str| answers.noul(id).is_none_or(|p| p >= YES_THRESHOLD);
    QueryClassification {
        needs_memory: yes(NEEDS_MEMORY),
        needs_file_list: yes(NEEDS_FILE_LIST),
        needs_credentials: yes(NEEDS_CREDENTIALS),
        sub_queries: vec![],
    }
}

#[cfg(test)]
#[path = "query_judgment_tests.rs"]
mod tests;
