//! Who switched an MCP server on or off for one thread (ADR 0430).

use serde::{Deserialize, Serialize};

/// Who enabled or disabled an MCP server in a thread.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum McpEnablementSource {
    /// The thread's own agent, through the `mcp` tool.
    Agent,
    /// A person, through the thread's HTTP route.
    User,
    /// The trigger that fired this thread lists the server.
    Trigger,
    /// The parent thread had the server enabled when it spawned this child.
    Parent,
}
