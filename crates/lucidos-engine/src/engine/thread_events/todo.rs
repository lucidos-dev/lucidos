use serde::{Deserialize, Serialize};

/// Status of one *Todo item* (see `system-knowhow/glossary.md`). Item shape
/// mirrors CC's `TodoWrite` so users see the same mental model across agents.
///
/// `Pending` / `InProgress` / `Completed` are LLM-writable via `todo_write`.
/// `Abandoned` is engine-only: the response-termination subscriber
/// (`todo_consumer`) flips any non-completed items to `Abandoned` so the user
/// can tell at a glance the agent walked away from them. `todo_write` rejects
/// `Abandoned` to keep that semantic honest.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Abandoned,
}

/// One row of a *Todo list*. `content` is the imperative form ("Run tests");
/// `active_form` is the present-continuous form ("Running tests") shown only
/// while the item is `InProgress`. Pending / Completed rows render `content`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoItem {
    pub content: String,
    pub active_form: String,
    pub status: TodoStatus,
}
