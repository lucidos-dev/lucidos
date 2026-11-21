//! Per-session synchronization between question requests and persisted output.

/// Starts every question id the MCP ask tool mints (`mcp_permission_server.rs`
/// in the CLI). The stream never carries such an id.
pub const MCP_ASK_ID_PREFIX: &str = "codex-q-";

/// Which question tool use a card waits for in [`QuestionFlushes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardAsk {
    /// A Claude Code hook names the tool use the agent's stream carries.
    ToolUse(String),
    /// The session's n-th MCP ask, counting from 1. Each MCP question tool
    /// use makes exactly one ask, so it pairs with the n-th one flushed.
    McpAsk(u64),
}

/// The question tool uses a coding-agent session loop has handled, each after
/// it persisted the text written before it. A card's gate waits here for its
/// own tool use before the second look. One per session, ephemeral: a restart
/// ends the loop and every waiter with it.
#[derive(Debug, Default)]
pub struct QuestionFlushes {
    state: std::sync::Mutex<Flushed>,
    changed: tokio::sync::Notify,
}

#[derive(Debug, Default)]
struct Flushed {
    /// Claude Code's own question tool uses, by id.
    hook_tool_uses: std::collections::HashSet<String>,
    /// MCP question tool uses flushed, and MCP asks arrived.
    mcp_flushed: u64,
    mcp_asked: u64,
    /// The loop ended, so nothing more will be persisted.
    loop_ended: bool,
}

impl QuestionFlushes {
    /// The loop persisted the text before question tool use `tool_use_id`,
    /// a call of tool `name`.
    pub fn flushed(&self, tool_use_id: &str, name: &str, input: &serde_json::Value) {
        self.update(|state| {
            if name == crate::runtime::CC_NATIVE_ASK_USER_QUESTION_TOOL {
                state.hook_tool_uses.insert(tool_use_id.to_string());
            } else if mcp_ask_is_sent(input) {
                state.mcp_flushed += 1;
            }
        });
    }

    /// The loop ended. Every waiter's second look may run now.
    pub fn loop_ended(&self) {
        self.update(|state| state.loop_ended = true);
    }

    fn update(&self, change: impl FnOnce(&mut Flushed)) {
        change(&mut self.lock());
        self.changed.notify_waiters();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Flushed> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The card a request for `tool_use_id` asks. Called once per request
    /// from this session's agent, since an MCP ask takes the next ordinal.
    pub fn ask(&self, tool_use_id: &str) -> CardAsk {
        if !tool_use_id.starts_with(MCP_ASK_ID_PREFIX) {
            return CardAsk::ToolUse(tool_use_id.to_string());
        }
        let mut state = self.lock();
        state.mcp_asked += 1;
        CardAsk::McpAsk(state.mcp_asked)
    }

    /// Resolves once `ask`'s tool use is flushed, or the loop ended.
    pub async fn wait_for(&self, ask: CardAsk) {
        loop {
            // Registered before the check, so a flush between the two wakes it.
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.is_flushed(&ask) {
                return;
            }
            changed.await;
        }
    }

    fn is_flushed(&self, ask: &CardAsk) -> bool {
        let state = self.lock();
        state.loop_ended
            || match ask {
                CardAsk::ToolUse(id) => state.hook_tool_uses.contains(id),
                CardAsk::McpAsk(n) => state.mcp_flushed >= *n,
            }
    }
}

/// Whether the MCP ask tool posts this call to the engine. It refuses one with
/// no question before asking, so that call takes no ordinal.
fn mcp_ask_is_sent(input: &serde_json::Value) -> bool {
    input
        .get("question")
        .and_then(|q| q.as_str())
        .is_some_and(|q| !q.trim().is_empty())
}

/// Marks the loop ended when the session loop holding it returns, by any path.
pub struct LoopEndsFlushes(pub std::sync::Arc<QuestionFlushes>);

impl Drop for LoopEndsFlushes {
    fn drop(&mut self) {
        self.0.loop_ended();
    }
}
