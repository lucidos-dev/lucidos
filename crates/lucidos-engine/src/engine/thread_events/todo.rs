use serde::{Deserialize, Serialize};

/// Status of one *Todo item* (see `system-knowhow/glossary.md`). Item shape
/// mirrors CC's `TodoWrite` so users see the same mental model across agents.
///
/// `Pending` / `InProgress` / `Completed` are LLM-writable via `todo_write`.
/// `Waiting` and `Abandoned` are engine-only, both written by the
/// response-termination subscriber (`todo_consumer`), and `todo_write` rejects
/// both to keep that semantic honest: the engine is asserting something about
/// the thread that the model would only be guessing at.
///
/// Which of the two an open item settles to is the whole distinction.
/// `Abandoned` means the agent walked away. `Waiting` means it parked on
/// purpose, because the thread still holds a live *event wait*: per ADR 0049
/// `await_event` does not hold the turn, so a subscribed thread terminates its
/// response like any other and then sleeps until the event arrives. Same split
/// the thread's own status dot makes, where a live wait resolves to the
/// `waiting` VisualStatus rather than reading as finished.
///
/// [`TodoStatus::is_open`] is the shared predicate: `Completed` and
/// `Abandoned` are terminal, everything else is still settleable.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Waiting,
    Abandoned,
}

impl TodoStatus {
    /// Whether a response terminator may still rewrite this item's status.
    ///
    /// `Waiting` is open, not terminal: the wait it describes resolves, and the
    /// next terminator has to be able to settle the item to `Abandoned` so a
    /// parked list cannot read as parked forever. `Abandoned` is terminal in
    /// the other direction, since a later subscription on the same thread does
    /// not un-abandon an item the agent already walked away from.
    pub fn is_open(self) -> bool {
        match self {
            TodoStatus::Pending | TodoStatus::InProgress | TodoStatus::Waiting => true,
            TodoStatus::Completed | TodoStatus::Abandoned => false,
        }
    }
}

/// One row of a *Todo list*. `content` is the imperative form ("Run tests");
/// `active_form` is the present-continuous form ("Running tests") shown only
/// while the item is `InProgress`. Every other status renders `content`,
/// including `Waiting`: a parked item is not being worked, so the
/// present-continuous form would claim activity that stopped.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoItem {
    pub content: String,
    pub active_form: String,
    pub status: TodoStatus,
}
