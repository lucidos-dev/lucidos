use serde::{Deserialize, Serialize};

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
    /// INVARIANT: Whether a response terminator may still rewrite this item's status.
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

    pub fn as_str(self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
            TodoStatus::Waiting => "waiting",
            TodoStatus::Abandoned => "abandoned",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoItem {
    pub content: String,
    pub active_form: String,
    pub status: TodoStatus,
}
