//! The *compose report*: what the engine stores as a thread's draft, at one
//! *compose revision* (`docs/glossary.md`).
//!
//! Every writer reads one back from the statement that wrote it. So the
//! version always belongs to the fields beside it, and the client orders
//! reports by that version alone (ADR 0437).

use serde::Serialize;
use serde_json::Value as JsonValue;
use sqlx::{Executor, Postgres, Transaction};
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, SystemEvent};

/// Cap on `compose_text` size. Every compose update fans out to each SSE
/// subscriber. Without a cap, a runaway paste costs that bandwidth per device.
pub const MAX_COMPOSE_TEXT_BYTES: usize = 64 * 1024;
/// How many images one compose draft may carry. A COUNT, not a byte budget:
/// each entry is a blob hash of fixed length, checked by `is_blob_hash`, so
/// the two caps together bound the array at 32 x 64 bytes.
pub const MAX_COMPOSE_IMAGES: usize = 32;
/// Cap on the JSON-encoded `compose_selection` object. A real selection is a
/// few hundred bytes. The cap fences off a runaway client, since the selection
/// fans out over SSE like the other compose fields.
pub const MAX_COMPOSE_SELECTION_BYTES: usize = 8 * 1024;

/// [`COMPOSE_REPORT_COLS`] as a literal, so `concat!` can extend it.
macro_rules! compose_report_cols {
    () => {
        "summary_version, compose_epoch, compose_text, compose_images, compose_mode, compose_selection"
    };
}
pub(crate) use compose_report_cols;

/// The columns a compose report is read from, in [`ComposeReportRow`] order.
/// Spliced into a `RETURNING` or `SELECT` list.
pub const COMPOSE_REPORT_COLS: &str = compose_report_cols!();

/// The row shape [`COMPOSE_REPORT_COLS`] decodes into.
pub type ComposeReportRow = (
    i64,
    i64,
    String,
    JsonValue,
    Option<String>,
    Option<JsonValue>,
);

#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
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
///
/// Pass a transaction connection when accepting durable work, so consuming
/// the draft rolls back if recording that work fails. A pool preserves the
/// standalone consumption contract for existing callers.
pub async fn consume_compose_slot<'e>(
    executor: impl Executor<'e, Database = Postgres>,
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
    .fetch_optional(executor)
    .await?;
    Ok(row.map(ComposeReport::from))
}

/// Pin inherited agents while the caller holds the exclusive seed lock.
/// Return reports to broadcast after the preference transaction commits.
pub(crate) async fn pin_legacy_compose_agents(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<Vec<(Uuid, ComposeReport)>, sqlx::Error> {
    type PinnedRow = (
        Uuid,
        i64,
        i64,
        String,
        serde_json::Value,
        Option<String>,
        Option<serde_json::Value>,
    );
    let rows: Vec<PinnedRow> = sqlx::query_as(
            "UPDATE thread_summaries SET compose_selection = \
             COALESCE(compose_selection, '{}'::jsonb) || jsonb_build_object('codingAgent', \
                COALESCE((SELECT NULLIF(value, '') FROM preferences WHERE key = $1 AND device_id IS NULL), $2)) \
             WHERE state = 'composing' AND compose_selection->>'codingAgent' IS NULL \
             RETURNING thread_id, summary_version, compose_epoch, compose_text, compose_images, compose_mode, compose_selection",
        )
        .bind(crate::core::prefs::CODING_AGENT_DEFAULT.key())
        .bind(crate::core::prefs::CODING_AGENT_DEFAULT.default_text())
        .fetch_all(&mut **tx).await?;
    Ok(rows
        .into_iter()
        .map(|(id, version, epoch, text, images, mode, selection)| {
            (
                id,
                ComposeReport::from((version, epoch, text, images, mode, selection)),
            )
        })
        .collect())
}
