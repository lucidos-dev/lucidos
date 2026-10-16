//! The *compose report*: what the engine stores as a thread's draft, at one
//! *compose revision* (`docs/glossary.md`).
//!
//! Every writer reads one back from the statement that wrote it. So the
//! version always belongs to the fields beside it, and the client orders
//! reports by that version alone (ADR 0437).

use serde::Serialize;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, SystemEvent};

/// The columns a compose report is read from, in [`ComposeReportRow`] order.
/// Spliced into a `RETURNING` or `SELECT` list.
pub const COMPOSE_REPORT_COLS: &str =
    "summary_version, compose_epoch, compose_text, compose_images, compose_mode, compose_selection";

/// The row shape [`COMPOSE_REPORT_COLS`] decodes into.
pub type ComposeReportRow = (
    i64,
    i64,
    String,
    JsonValue,
    Option<String>,
    Option<JsonValue>,
);

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ComposeReport {
    /// The row's `summary_version` right after the statement that produced
    /// this report. It is the *compose revision*.
    pub summary_version: i64,
    pub compose_epoch: i64,
    pub text: String,
    pub image_hashes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<JsonValue>,
}

impl From<ComposeReportRow> for ComposeReport {
    fn from(
        (summary_version, compose_epoch, text, images, mode, selection): ComposeReportRow,
    ) -> Self {
        let image_hashes = images
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            summary_version,
            compose_epoch,
            text,
            image_hashes,
            mode,
            selection,
        }
    }
}

impl ComposeReport {
    /// The SSE frame that tells every device about this report.
    /// `origin_device_id` names the device whose write produced it, and is
    /// `None` for a change the engine made itself.
    pub fn broadcast(self, thread_id: Uuid, origin_device_id: Option<String>) -> BusEvent {
        BusEvent::System(SystemEvent::ThreadComposeChanged {
            id: thread_id,
            text: self.text,
            image_hashes: self.image_hashes,
            mode: self.mode,
            selection: self.selection,
            compose_epoch: self.compose_epoch,
            summary_version: self.summary_version,
            origin_device_id,
        })
    }
}

/// Whitespace `btrim` strips when matching a submission against the stored
/// draft, so the comparison agrees with the client's `String.prototype.trim()`.
/// Bound as a parameter because one-argument `btrim` strips spaces only, and a
/// textarea's trailing newline would otherwise defeat the match.
const COMPOSE_TRIM_CHARS: &str = " \t\n\r\u{000b}\u{000c}";

/// A submission from the composer was accepted: consume the thread's compose
/// slot (ADR 0450). Returns the report it left, or `None` when the thread has
/// no live compose slot (no row, or discarded).
///
/// The *compose epoch* always rises, so a write composed before the submission
/// is refused even if it is still in flight. The mode and the selection always
/// clear, because the send carried them. The stored text and images clear only
/// when they are the submitted content or an earlier state of it: trimmed text
/// the message starts with, and images it carries. That covers a send that beat
/// its last debounced write. A draft typed since survives. A caller passes only
/// a submission that carried composer content: an options-only answer consumes
/// nothing.
pub async fn consume_compose_slot(
    pool: &PgPool,
    thread_id: Uuid,
    submitted_text: &str,
    submitted_image_hashes: &[String],
) -> Result<Option<ComposeReport>, sqlx::Error> {
    // Every SET expression reads the row as it was before this statement, and
    // the row lock makes that the row a concurrent PUT left. So the match is
    // decided against the draft actually being replaced.
    const IS_DRAFT: &str = "starts_with(btrim($2, $3), btrim(compose_text, $3)) \
         AND COALESCE(compose_images, '[]'::jsonb) <@ $4";
    let row: Option<ComposeReportRow> = sqlx::query_as(&format!(
        "UPDATE thread_summaries SET \
             compose_text = CASE WHEN {IS_DRAFT} THEN '' ELSE compose_text END, \
             compose_images = CASE WHEN {IS_DRAFT} THEN '[]'::jsonb ELSE compose_images END, \
             compose_mode = NULL, \
             compose_selection = NULL, \
             compose_epoch = compose_epoch + 1 \
         WHERE thread_id = $1 AND state IN ('composing', 'active') \
         RETURNING {COMPOSE_REPORT_COLS}"
    ))
    .bind(thread_id)
    .bind(submitted_text)
    .bind(COMPOSE_TRIM_CHARS)
    .bind(serde_json::json!(submitted_image_hashes))
    .fetch_optional(pool)
    .await?;
    Ok(row.map(ComposeReport::from))
}
