//! Resolving the workspace's [`ToolCapabilities`], the gate inputs the tool
//! registry reads (ADR 0088).
//!
//! One resolver, used by the chat turn and by the intent sub-loop, so the two
//! cannot disagree about what this workspace can do. It reads workspace
//! configuration and nothing thread-shaped, which is what keeps every thread
//! in the workspace on one byte-identical tools array.

use crate::core::{EmailStore, Intent, IntentStore};
use crate::engine::LucidosEngine;
use crate::llm::ToolCapabilities;

/// One turn's capability read: the gates the tool registry needs, and the
/// intent snapshot the `Intent` gate was derived from.
///
/// The snapshot travels with the gate deliberately. The prompt's "Available
/// Intents" listing and the `execute_intent` tool must agree, and two separate
/// scans of `data/` can disagree inside one turn. An intent created between
/// them leaves the prompt telling the model to call a tool it was not offered.
pub(crate) struct TurnCapabilities {
    pub gates: ToolCapabilities,
    pub intents: Vec<Intent>,
    /// When results leave, read beside the mode's own flag so the prompt, the
    /// panel and the sweep all quote one pair of numbers.
    pub schedule: crate::engine::SweepSchedule,
}

impl LucidosEngine {
    /// Read this workspace's tool gates. Called once per turn.
    ///
    /// An unreadable gate resolves OPEN, never closed. Closing one withdraws a
    /// capability the workspace has, and rewrites the whole tools cache tier
    /// for that turn. Opening it offers a tool that reports its own error if
    /// it is ever called. Same trade as the mandatory-preference read in
    /// `build_chat_system_prompt`.
    pub(crate) async fn read_turn_capabilities(&self) -> TurnCapabilities {
        let email_account = match EmailStore::list(&self.pool).await {
            Ok(accounts) => !accounts.is_empty(),
            Err(e) => {
                crate::log!(
                    "[Tools] email account read failed ({}); offering the email tools anyway",
                    e
                );
                true
            }
        };

        let data_dir = self.workspace_path.join(crate::core::DATA_DIR);
        let intents = IntentStore::load_all(&data_dir);

        // The self-curated context mode. Read here rather than beside its own
        // consumers so the tools array, the system prompt and the turn's
        // payload all answer from ONE read: a mid-turn Settings change cannot
        // then leave the prompt describing a mode the payload is not in.
        // Total, and an unreadable row resolves OFF.
        let context_mode =
            crate::core::PreferenceStore::self_curated_context_mode(&self.pool).await;
        // Two more rows, and only where they are read. An off workspace pays
        // no query for a schedule nothing consults.
        let schedule = if context_mode {
            crate::core::PreferenceStore::self_curated_context_schedule(&self.pool).await
        } else {
            crate::engine::SweepSchedule::default()
        };

        TurnCapabilities {
            gates: ToolCapabilities {
                email_account,
                intent: !intents.is_empty(),
                image_provider: self.current_image_provider().await.is_some(),
                context_mode,
            },
            intents,
            schedule,
        }
    }
}
