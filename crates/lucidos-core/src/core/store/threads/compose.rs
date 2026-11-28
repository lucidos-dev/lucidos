use serde::Serialize;
use serde_json::Value as JsonValue;
use sqlx::{Executor, Postgres, Transaction};
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, SystemEvent};

pub const MAX_COMPOSE_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_COMPOSE_IMAGES: usize = 32;
pub const MAX_COMPOSE_SELECTION_BYTES: usize = 8 * 1024;

macro_rules! compose_report_cols {
    () => {
        "summary_version, compose_epoch, compose_text, compose_images, compose_mode, compose_selection, compose_restorations"
    };
}
pub(crate) use compose_report_cols;

/// INVARIANT: The columns a compose report is read from, in [`ComposeReportRow`] order.
/// Spliced into a `RETURNING` or `SELECT` list.
pub const COMPOSE_REPORT_COLS: &str = compose_report_cols!();

pub type ComposeReportRow = (
    i64,
    i64,
    String,
    JsonValue,
    Option<String>,
    Option<JsonValue>,
    JsonValue,
);

#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct ComposeRestoration {
    pub epoch: i64,
    pub text: String,
    pub image_hashes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct ComposeReport {
    pub summary_version: i64,
    pub compose_epoch: i64,
    #[serde(default)]
    pub restorations: Vec<ComposeRestoration>,
    pub text: String,
    pub image_hashes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<JsonValue>,
}

impl From<ComposeReportRow> for ComposeReport {
    fn from(
        (summary_version, compose_epoch, text, images, mode, selection, restorations): ComposeReportRow,
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
            restorations: serde_json::from_value(restorations)
                .expect("stored compose restorations"),
            text,
            image_hashes,
            mode,
            selection,
        }
    }
}

impl ComposeReport {
    pub fn broadcast(self, thread_id: Uuid, origin_device_id: Option<String>) -> BusEvent {
        BusEvent::System(SystemEvent::ThreadComposeChanged {
            id: thread_id,
            text: self.text,
            image_hashes: self.image_hashes,
            mode: self.mode,
            selection: self.selection,
            compose_epoch: self.compose_epoch,
            restorations: self.restorations,
            summary_version: self.summary_version,
            origin_device_id,
        })
    }
}

/// INVARIANT: Whitespace `btrim` strips when matching a submission against the stored
/// draft, so the comparison agrees with the client's `String.prototype.trim()`.
/// Bound as a parameter because one-argument `btrim` strips spaces only, and a
/// textarea's trailing newline would otherwise defeat the match.
const COMPOSE_TRIM_CHARS: &str = " \t\n\r\u{000b}\u{000c}";

pub async fn consume_compose_slot<'e>(
    executor: impl Executor<'e, Database = Postgres>,
    thread_id: Uuid,
    submitted_text: &str,
    submitted_image_hashes: &[String],
) -> Result<Option<ComposeReport>, sqlx::Error> {
    // INVARIANT: Every SET expression reads the row as it was before this statement, and
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
             compose_restorations = '[]'::jsonb, \
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

/// INVARIANT: Pin inherited agents while the caller holds the exclusive seed lock.
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
        serde_json::Value,
    );
    let rows: Vec<PinnedRow> = sqlx::query_as(
            "UPDATE thread_summaries SET compose_selection = \
             COALESCE(compose_selection, '{}'::jsonb) || jsonb_build_object('codingAgent', \
                COALESCE((SELECT NULLIF(value, '') FROM preferences WHERE key = $1 AND device_id IS NULL), $2)) \
             WHERE state = 'composing' AND compose_selection->>'codingAgent' IS NULL \
             RETURNING thread_id, summary_version, compose_epoch, compose_text, compose_images, compose_mode, compose_selection, compose_restorations",
        )
        .bind(crate::core::prefs::CODING_AGENT_DEFAULT.key())
        .bind(crate::core::prefs::CODING_AGENT_DEFAULT.default_text())
        .fetch_all(&mut **tx).await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, version, epoch, text, images, mode, selection, restorations)| {
                (
                    id,
                    ComposeReport::from((
                        version,
                        epoch,
                        text,
                        images,
                        mode,
                        selection,
                        restorations,
                    )),
                )
            },
        )
        .collect())
}

pub(crate) async fn restore_compose_slot(
    tx: &mut Transaction<'_, Postgres>,
    thread_id: Uuid,
    text: &str,
    images: &[String],
) -> Result<ComposeReport, Box<dyn std::error::Error + Send + Sync>> {
    let row: Option<ComposeReportRow> = sqlx::query_as(&format!(
        "SELECT {COMPOSE_REPORT_COLS} FROM thread_summaries WHERE thread_id=$1 \
         AND state IN ('composing','active') FOR UPDATE"
    ))
    .bind(thread_id)
    .fetch_optional(&mut **tx)
    .await?;
    let mut draft = row
        .map(ComposeReport::from)
        .ok_or("Stop restoration has no live compose slot")?;
    if !text.is_empty() {
        if !draft.text.trim().is_empty() {
            draft.text.push_str("\n\n");
        } else {
            draft.text.clear();
        }
        draft.text.push_str(text);
    }
    for image in images {
        if !draft.image_hashes.contains(image) {
            draft.image_hashes.push(image.clone());
        }
    }
    if draft.text.len() > MAX_COMPOSE_TEXT_BYTES || draft.image_hashes.len() > MAX_COMPOSE_IMAGES {
        return Err(
            "Stop restoration exceeds the compose text or image limit; the input was not removed"
                .into(),
        );
    }
    draft.restorations.push(ComposeRestoration {
        epoch: draft.compose_epoch + 1,
        text: text.to_owned(),
        image_hashes: images.to_vec(),
    });
    let restored_bytes: usize = draft.restorations.iter().map(|r| r.text.len()).sum();
    let restored_images: usize = draft
        .restorations
        .iter()
        .map(|r| r.image_hashes.len())
        .sum();
    if restored_bytes > MAX_COMPOSE_TEXT_BYTES
        || restored_images > MAX_COMPOSE_IMAGES
        || serde_json::to_vec(&draft.restorations)?.len() > MAX_COMPOSE_TEXT_BYTES * 2
    {
        return Err(
            "Stop restoration journal exceeds the compose limit; the input was not removed".into(),
        );
    }
    let row: ComposeReportRow = sqlx::query_as(&format!(
        "UPDATE thread_summaries SET compose_text=$2,compose_images=$3,compose_restorations=$4,compose_epoch=compose_epoch+1 \
         WHERE thread_id=$1 RETURNING {COMPOSE_REPORT_COLS}"
    )).bind(thread_id).bind(draft.text).bind(serde_json::json!(draft.image_hashes)).bind(serde_json::json!(draft.restorations))
        .fetch_one(&mut **tx).await?;
    Ok(row.into())
}
