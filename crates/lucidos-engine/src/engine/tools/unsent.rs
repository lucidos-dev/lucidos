//! The `threads` tool's actions for what a thread holds unsent: its *draft*
//! (`drafts`, `submit_draft`) and its *held messages* (`held_messages`). An
//! agent reads both. It sends a saved draft exactly as saved, and only with
//! the owner's authority (ADR 0651). Held messages stay read-only.

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

    /// `threads` action `submit_draft`: send a saved draft by revision and
    /// epoch. Mirrors `POST /api/v1/threads/:thread_id/submit-draft` and
    /// `lucidos threads submit-draft`.
    pub(super) async fn execute_submit_draft(
        &self,
        args: &serde_json::Value,
        caller_thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        use crate::engine::draft_submission::{refusal_report, RefusalAudience};
        let raw = args
            .get("thread_id")
            .and_then(|v| v.as_str())
            .ok_or("Error: submit_draft needs the draft's thread_id.")?;
        let thread_id = crate::api::resolve_thread_id_arg(raw, Some(caller_thread_id))
            .map_err(|e| format!("Error: {e}"))?;
        let token = |field: &str| {
            args.get(field).and_then(|v| v.as_i64()).ok_or_else(|| {
                format!(
                    "Error: submit_draft needs {field}. Read it from the draft's snapshot \
                     with the 'drafts' action."
                )
            })
        };
        let retry_key = match args.get("retry_key").and_then(|v| v.as_str()) {
            None => uuid::Uuid::new_v4(),
            Some(raw) => uuid::Uuid::parse_str(raw.trim())
                .map_err(|_| format!("Error: retry_key takes a uuid, and '{raw}' is not one."))?,
        };
        let request = crate::core::draft_submission::SubmitDraftRequest {
            thread_id,
            expected_revision: token("expected_revision")?,
            expected_epoch: token("expected_epoch")?,
            retry_key,
        };
        match self
            .submit_saved_draft_as_agent(caller_thread_id, request)
            .await
        {
            Ok(receipt) => {
                let json = serde_json::to_string(&receipt)
                    .map_err(|e| format!("Error: failed to serialise the receipt: {e}"))?;
                Ok(format!(
                    "Accepted the saved draft on thread {thread_id}. It runs next. \
                     Retry with the same retry_key to read this receipt again.\n{json}"
                ))
            }
            Err(error) => {
                let report = refusal_report(&*error, thread_id, RefusalAudience::ChatAgent);
                Err(format!("Error: {}", report.message))
            }
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
