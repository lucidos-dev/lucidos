//! Contracts for submitting saved owner content (ADR 0651).
//!
//! A caller names a saved revision, never replacement content. The acceptance
//! path must parse the raw database row with `SavedDraftSnapshot::try_from`:
//! the tolerant compose reader is unsuitable for deciding what to send.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::store::{ComposeReport, ComposeReportRow};

mod chat_settings;
mod coding_settings;
pub use coding_settings::ResolvedCodingSelection;

/// The complete caller-controlled input. Unknown fields fail closed, so this
/// cannot grow into an arbitrary-message or device-impersonation API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitDraftRequest {
    pub thread_id: Uuid,
    pub expected_revision: i64,
    pub expected_epoch: i64,
    pub retry_key: Uuid,
}

impl SubmitDraftRequest {
    pub fn validate(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.expected_revision < 0 || self.expected_epoch < 0 {
            return Err("draft_snapshot_token_invalid".into());
        }
        if self.thread_id.is_nil() || self.retry_key.is_nil() {
            return Err("draft_submission_identity_invalid".into());
        }
        Ok(())
    }

    /// Compare against the raw row, before parsing it. A consumed draft is
    /// empty, so parsing first would hide a stale revision behind `draft_empty`.
    pub fn names_current(&self, row: &SavedDraftRow) -> bool {
        row.draft_revision == self.expected_revision && row.report.1 == self.expected_epoch
    }
}

/// The columns a [`SavedDraftRow`] is read from.
pub const SAVED_DRAFT_COLS: &str = concat!(
    crate::core::store::compose_report_cols!(),
    ", draft_revision"
);

/// A saved draft as read from `thread_summaries`: its compose report and its
/// *draft revision*. The revision moves only when the draft does, while the
/// report's `summary_version` moves with any change to the row.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedDraftRow {
    pub draft_revision: i64,
    pub report: ComposeReportRow,
}

impl SavedDraftRow {
    pub fn new(draft_revision: i64, report: ComposeReportRow) -> Self {
        Self {
            draft_revision,
            report,
        }
    }
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for SavedDraftRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            draft_revision: row.try_get("draft_revision")?,
            report: <ComposeReportRow as sqlx::FromRow<'r, sqlx::postgres::PgRow>>::from_row(row)?,
        })
    }
}

/// Stable identity returned only after durable acceptance. Retrying an accepted
/// request returns the original receipt; it does not create another send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftSubmissionReceipt {
    pub thread_id: Uuid,
    pub revision: i64,
    pub epoch: i64,
    pub retry_key: Uuid,
    pub event_id: Uuid,
    pub queue_entry_id: Uuid,
}

/// Evidence rechecked in the acceptance transaction, retained beside
/// the accepted payload so execution and recovery cannot invent a device send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DraftSubmissionAuthority {
    StandingInstruction {
        instruction_event_id: Uuid,
    },
    /// A once permission to send this draft. The acceptance spends it.
    Permission {
        permission_id: Uuid,
    },
    /// History only: an Allow once card from before permissions. Nothing
    /// builds it now, and acceptance refuses it.
    OwnerApproval {
        tool_use_id: String,
    },
}

/// Stored composer selections use the frontend's camel-case field names.
/// Keeping a strict typed shape prevents an unknown future pick from being
/// silently dropped while sending through an older engine.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedDraftSelection {
    pub scope: Option<SavedDraftScope>,
    pub coding_agent: Option<crate::runtime::CodingAgent>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub provider: Option<String>,
    pub cc_model: Option<String>,
    pub cc_reasoning_effort: Option<String>,
    pub side_question_mode: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SavedDraftScope {
    Lucidos {},
    External {
        #[serde(rename = "repoId")]
        repo_id: String,
    },
    App {
        #[serde(rename = "appId")]
        app_id: String,
    },
}

/// A structurally valid snapshot, before destination/model/blob validation.
/// Fields stay private so callers cannot accidentally substitute the tolerant
/// reader's coerced image array for the raw database value.
#[derive(Debug, Clone)]
pub struct SavedDraftSnapshot {
    draft_revision: i64,
    report: ComposeReport,
    selection: SavedDraftSelection,
}

/// Two snapshots are equal when they hold the same draft at the same compose
/// revision. The report's `summary_version` is left out: activity moves it.
impl PartialEq for SavedDraftSnapshot {
    fn eq(&self, other: &Self) -> bool {
        let (a, b) = (&self.report, &other.report);
        self.draft_revision == other.draft_revision
            && a.compose_epoch == b.compose_epoch
            && a.text == b.text
            && a.image_hashes == b.image_hashes
            && a.mode == b.mode
            && a.selection == b.selection
            && self.selection == other.selection
    }
}

impl SavedDraftSnapshot {
    pub fn draft_revision(&self) -> i64 {
        self.draft_revision
    }

    pub fn report(&self) -> &ComposeReport {
        &self.report
    }

    pub fn selection(&self) -> &SavedDraftSelection {
        &self.selection
    }

    /// Resolve every attachment before accepting any of the draft. A blob's
    /// pathname alone is insufficient: its bytes must still match the saved
    /// content address and the image type the blob store serves.
    pub fn resolve_images(
        &self,
        workspace: &std::path::Path,
    ) -> Result<Vec<crate::core::blobs::ResolvedBlob>, Box<dyn std::error::Error + Send + Sync>>
    {
        use crate::core::blobs::{compute_hash, resolve_blob, sniff_image_mime};
        self.report
            .image_hashes
            .iter()
            .map(|hash| {
                let blob = resolve_blob(workspace, hash).ok_or("draft_image_missing")?;
                let bytes = std::fs::read(&blob.path)
                    .map_err(|e| format!("draft_image_unreadable: {e}"))?;
                if compute_hash(&bytes) != *hash
                    || sniff_image_mime(&bytes) != Some(blob.mime.as_str())
                {
                    return Err("draft_image_invalid".into());
                }
                Ok(blob)
            })
            .collect()
    }
}

impl TryFrom<SavedDraftRow> for SavedDraftSnapshot {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(
        SavedDraftRow {
            draft_revision,
            report: row,
        }: SavedDraftRow,
    ) -> Result<Self, Self::Error> {
        if draft_revision < 0 || row.1 < 0 {
            return Err("draft_snapshot_token_invalid".into());
        }
        let images = row.3.as_array().ok_or("draft_images_invalid")?;
        for image in images {
            let hash = image.as_str().ok_or("draft_images_invalid")?;
            if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err("draft_images_invalid".into());
            }
        }
        if row.2.trim_matches(composer_whitespace).is_empty() && images.is_empty() {
            return Err("draft_empty".into());
        }
        if !matches!(row.4.as_deref(), None | Some("lucidos" | "claude_code")) {
            return Err("draft_mode_unsupported".into());
        }
        let selection = match row.5.as_ref() {
            None => SavedDraftSelection::default(),
            Some(value) if value.is_object() => serde_json::from_value(value.clone())
                .map_err(|e| format!("draft_selection_invalid: {e}"))?,
            Some(_) => return Err("draft_selection_invalid".into()),
        };
        if selection.side_question_mode == Some(true) {
            return Err("draft_side_question_unsupported".into());
        }
        Ok(Self {
            draft_revision,
            report: ComposeReport::from(row),
            selection,
        })
    }
}

/// ECMAScript's String.trim whitespace, used by the ordinary composer. Rust's
/// whitespace set includes NEXT LINE and excludes BOM, so `str::trim` differs.
fn composer_whitespace(ch: char) -> bool {
    matches!(ch, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
        | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> SubmitDraftRequest {
        SubmitDraftRequest {
            thread_id: Uuid::new_v4(),
            expected_revision: 0,
            expected_epoch: 0,
            retry_key: Uuid::new_v4(),
        }
    }

    fn row() -> ComposeReportRow {
        (0, 0, "saved text\n".into(), json!([]), None, None)
    }

    #[test]
    fn request_refuses_replacement_content_settings_and_identity() {
        let value = serde_json::to_value(request()).unwrap();
        for field in [
            "message",
            "text",
            "images",
            "image_hashes",
            "mode",
            "selection",
            "model",
            "provider",
            "device_id",
            "origin",
            "source_thread_id",
            "parent_thread_id",
        ] {
            let mut injected = value.clone();
            injected[field] = json!("override");
            assert!(
                serde_json::from_value::<SubmitDraftRequest>(injected).is_err(),
                "{field}"
            );
        }
        for field in [
            "thread_id",
            "expected_revision",
            "expected_epoch",
            "retry_key",
        ] {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<SubmitDraftRequest>(missing).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn request_allows_revision_zero_but_refuses_negative_tokens_and_nil_identity() {
        let good = request();
        good.validate().unwrap();
        for bad in [
            SubmitDraftRequest {
                expected_revision: -1,
                ..good.clone()
            },
            SubmitDraftRequest {
                expected_epoch: -1,
                ..good.clone()
            },
            SubmitDraftRequest {
                thread_id: Uuid::nil(),
                ..good.clone()
            },
            SubmitDraftRequest {
                retry_key: Uuid::nil(),
                ..good
            },
        ] {
            assert!(bad.validate().is_err());
        }
    }

    #[test]
    fn malformed_raw_images_are_refused_instead_of_filtered() {
        for images in [
            json!(null),
            json!({}),
            json!("image"),
            json!([1]),
            json!(["a".repeat(64), null]),
            json!(["../image"]),
        ] {
            let mut raw = row();
            raw.3 = images;
            assert!(SavedDraftSnapshot::try_from(SavedDraftRow::new(0, raw)).is_err());
        }
    }

    #[test]
    fn malformed_mode_and_selection_are_refused() {
        let mut raw = row();
        raw.4 = Some("unsupported".into());
        assert!(SavedDraftSnapshot::try_from(SavedDraftRow::new(0, raw)).is_err());
        for selection in [json!(null), json!([]), json!("settings")] {
            let mut raw = row();
            raw.5 = Some(selection);
            assert!(SavedDraftSnapshot::try_from(SavedDraftRow::new(0, raw)).is_err());
        }
        for selection in [
            json!({"unknownSetting": true}),
            json!({"model": 7}),
            json!({"codingAgent": "unsupported"}),
            json!({"scope": {"kind": "external"}}),
            json!({"scope": {"kind": "lucidos", "repoId": "unexpected"}}),
            json!({"sideQuestionMode": true}),
        ] {
            let mut raw = row();
            raw.5 = Some(selection.clone());
            assert!(
                SavedDraftSnapshot::try_from(SavedDraftRow::new(0, raw)).is_err(),
                "{selection}"
            );
        }
    }

    #[test]
    fn each_scope_preserves_its_destination_and_refuses_extra_fields() {
        for scope in [
            json!({"kind": "lucidos"}),
            json!({"kind": "external", "repoId": "repo"}),
            json!({"kind": "app", "appId": "app"}),
        ] {
            let parsed: SavedDraftScope = serde_json::from_value(scope.clone()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), scope);
            let mut extra = scope;
            extra["extra"] = json!(true);
            assert!(serde_json::from_value::<SavedDraftScope>(extra).is_err());
        }
    }

    #[test]
    fn image_only_snapshot_preserves_order_and_exact_tokens() {
        let mut raw = row();
        raw.2.clear();
        let hashes = vec!["b".repeat(64), "a".repeat(64)];
        raw.3 = json!(hashes);
        raw.4 = Some("claude_code".into());
        raw.5 = Some(json!({"ccModel": null}));
        let saved = SavedDraftSnapshot::try_from(SavedDraftRow::new(0, raw)).unwrap();
        assert_eq!(saved.report().image_hashes, hashes);
        assert_eq!(saved.report().selection, Some(json!({"ccModel": null})));
    }

    #[test]
    fn only_the_saved_revision_and_epoch_are_current() {
        let raw = SavedDraftRow::new(0, row());
        assert!(request().names_current(&raw));
        assert!(!SubmitDraftRequest {
            expected_revision: 1,
            ..request()
        }
        .names_current(&raw));
        assert!(!SubmitDraftRequest {
            expected_epoch: 1,
            ..request()
        }
        .names_current(&raw));
    }

    #[test]
    fn empty_and_whitespace_only_snapshots_are_refused() {
        for text in ["", " \n\t", "\u{feff}", "\u{00a0}\u{2028}"] {
            let mut raw = row();
            raw.2 = text.into();
            assert!(SavedDraftSnapshot::try_from(SavedDraftRow::new(0, raw)).is_err());
        }
    }

    #[test]
    fn text_validation_agrees_with_composer_without_changing_saved_text() {
        for text in ["\u{0085}", "  saved text\n", "\u{feff}text\u{feff}"] {
            let mut raw = row();
            raw.2 = text.into();
            assert_eq!(
                SavedDraftSnapshot::try_from(SavedDraftRow::new(0, raw))
                    .unwrap()
                    .report()
                    .text,
                text
            );
        }
    }

    fn png(color: u8) -> Vec<u8> {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([color, 0, 0, 255]),
        ));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn every_image_is_resolved_in_saved_order() {
        let workspace = tempfile::tempdir().unwrap();
        let first = crate::core::blobs::write_blob(workspace.path(), &png(1)).unwrap();
        let second = crate::core::blobs::write_blob(workspace.path(), &png(2)).unwrap();
        let mut raw = row();
        raw.3 = json!([second.hash, first.hash]);
        let snapshot = SavedDraftSnapshot::try_from(SavedDraftRow::new(0, raw)).unwrap();
        assert_eq!(
            snapshot.resolve_images(workspace.path()).unwrap(),
            vec![second, first]
        );
    }

    #[test]
    fn one_missing_image_refuses_the_whole_draft() {
        let workspace = tempfile::tempdir().unwrap();
        let first = crate::core::blobs::write_blob(workspace.path(), &png(1)).unwrap();
        let mut raw = row();
        raw.3 = json!([first.hash, "f".repeat(64)]);
        let snapshot = SavedDraftSnapshot::try_from(SavedDraftRow::new(0, raw)).unwrap();
        assert!(snapshot.resolve_images(workspace.path()).is_err());
        assert_eq!(snapshot.report().image_hashes.len(), 2);
        assert_eq!(snapshot.report().text, "saved text\n");
    }

    #[test]
    fn a_replaced_blob_is_refused_even_when_its_path_still_exists() {
        let workspace = tempfile::tempdir().unwrap();
        let first = crate::core::blobs::write_blob(workspace.path(), &png(1)).unwrap();
        let mut raw = row();
        raw.3 = json!([first.hash]);
        let snapshot = SavedDraftSnapshot::try_from(SavedDraftRow::new(0, raw)).unwrap();
        std::fs::write(first.path, png(2)).unwrap();
        assert!(snapshot.resolve_images(workspace.path()).is_err());
    }
}
