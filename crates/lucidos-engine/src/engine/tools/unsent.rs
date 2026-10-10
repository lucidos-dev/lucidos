//! The `threads` tool's read actions for what a thread holds unsent: its
//! *draft* (`drafts`) and its *held messages* (`held_messages`). Both are
//! read-only, and neither has a write counterpart on any agent surface.

use super::ToolOutcome;
use crate::engine::LucidosEngine;

/// The `list` row fields the reader fields replace for the model. The full
/// draft stays one `drafts` call away, by thread id.
const COMPOSE_FIELDS_THE_MODEL_READS_AS_A_PREVIEW: [&str; 3] =
    ["compose_text", "compose_images", "compose_selection"];

/// Serialise `list` rows for the model: every summary field, the reader fields,
/// and no raw compose fields.
pub(super) fn list_rows_for_the_model(
    summaries: &[crate::core::store::ThreadSummary],
) -> Result<String, String> {
    let mut rows = serde_json::to_value(summaries)
        .map_err(|e| format!("Error: failed to serialise thread summaries: {e}"))?;
    for row in rows.as_array_mut().into_iter().flatten() {
        if let Some(fields) = row.as_object_mut() {
            for field in COMPOSE_FIELDS_THE_MODEL_READS_AS_A_PREVIEW {
                fields.remove(field);
            }
        }
    }
    // Compact JSON, like every tool that returns a JSON array.
    Ok(rows.to_string())
}

impl LucidosEngine {
    /// `threads` action `drafts`: every thread holding an unsent draft, or one
    /// thread's draft with its whole text. Mirrors `GET /api/v1/threads/drafts`
    /// and `lucidos threads drafts`.
    pub(super) async fn execute_list_drafts(&self, args: &serde_json::Value) -> ToolOutcome {
        let workspace = self.workspace_name();
        let store = self.event_store();
        let thread_id = match args
            .get("thread_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
        {
            None | Some("") => None,
            Some(raw) => Some(uuid::Uuid::parse_str(raw).map_err(|_| {
                format!(
                    "Error: thread_id takes a thread's uuid, and '{raw}' is not one. \
                     Omit it to list every draft with its thread_id."
                )
            })?),
        };
        let Some(thread_id) = thread_id else {
            let limit = args
                .get("limit")
                .and_then(|v| v.as_i64())
                .unwrap_or(100)
                .clamp(1, 1000);
            let drafts = store
                .list_drafts(&workspace, limit)
                .await
                .map_err(|e| format!("Error: failed to list drafts: {e}"))?;
            return serde_json::to_string(&drafts)
                .map_err(|e| format!("Error: failed to serialise drafts: {e}"));
        };
        match store.get_draft(&workspace, thread_id).await {
            Ok(Some(draft)) => serde_json::to_string(&draft)
                .map_err(|e| format!("Error: failed to serialise the draft: {e}")),
            Ok(None) => Err(format!(
                "Error: thread {thread_id} holds no unsent draft. \
                 Call 'drafts' with no thread_id to see which threads do."
            )),
            Err(e) => Err(format!("Error: failed to read the draft: {e}")),
        }
    }

    /// `threads` action `held_messages`: every held message still waiting
    /// behind a human. Mirrors `GET /api/v1/threads/held-messages` and
    /// `lucidos threads held-messages`.
    pub(super) async fn execute_list_held_messages(&self, args: &serde_json::Value) -> ToolOutcome {
        let limit = args
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(100)
            .clamp(1, 1000);
        let held = self
            .list_held_messages(limit)
            .await
            .map_err(|e| format!("Error: failed to list held messages: {e}"))?;
        serde_json::to_string(&held).map_err(|e| format!("Error: failed to serialise: {e}"))
    }
}
