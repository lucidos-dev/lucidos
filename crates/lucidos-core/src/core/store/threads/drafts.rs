use super::*;

pub const PREVIEW_CHARS: usize = 200;

/// INVARIANT: SQL truth of [`has_draft`] over `thread_summaries t`. A discarded thread
/// can hold neither: its discard clears the compose fields.
pub const HAS_DRAFT_SQL: &str = "(t.compose_text <> '' \
     OR (jsonb_typeof(t.compose_images) = 'array' AND jsonb_array_length(t.compose_images) > 0))";

pub fn has_draft(text: &str, images: &serde_json::Value) -> bool {
    !text.is_empty() || images.as_array().is_some_and(|a| !a.is_empty())
}

pub fn text_preview(text: &str) -> String {
    text.chars().take(PREVIEW_CHARS).collect()
}

pub fn char_length(text: &str) -> usize {
    text.chars().count()
}

pub fn thread_link(workspace: &str, thread_id: impl std::fmt::Display) -> String {
    format!("thread:{workspace}/{thread_id}")
}

pub fn attach_reader_fields<'a>(
    summaries: impl IntoIterator<Item = &'a mut ThreadSummary>,
    workspace: &str,
) {
    for summary in summaries {
        let holds = has_draft(&summary.compose_text, &summary.compose_images);
        summary.has_draft = Some(holds);
        summary.draft_preview = holds.then(|| text_preview(&summary.compose_text));
        summary.draft_length = holds.then(|| char_length(&summary.compose_text));
        summary.link = Some(thread_link(workspace, &summary.thread_id));
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DraftSummary {
    pub thread_id: String,
    pub title: String,
    pub channel: String,
    pub state: ThreadState,
    pub section: String,
    pub status: ThreadStatus,
    pub parent_thread_id: Option<String>,
    pub drafted_by: Option<String>,
    pub preview: String,
    pub length: usize,
    pub image_count: usize,
    pub last_edited: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// INVARIANT: Exact compose report for a single-draft read. Its revision and fields
    /// come from one SELECT; list reads carry previews only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<DraftSnapshot>,
    pub link: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DraftSnapshot {
    pub draft_revision: i64,
    #[serde(flatten)]
    pub report: ComposeReport,
}

#[derive(sqlx::FromRow)]
struct DraftRow {
    thread_id: String,
    title: Option<String>,
    first_message: Option<String>,
    source: String,
    state: String,
    archive_state: String,
    status: String,
    parent_thread_id: Option<String>,
    drafted_by_thread_id: Option<String>,
    compose_text: String,
    compose_images: serde_json::Value,
    compose_updated_at: Option<chrono::DateTime<chrono::Utc>>,
    summary_version: i64,
    draft_revision: i64,
    compose_epoch: i64,
    compose_restorations: serde_json::Value,
    compose_mode: Option<String>,
    compose_selection: Option<serde_json::Value>,
}

impl DraftRow {
    fn into_summary(
        self,
        workspace: &str,
        with_text: bool,
    ) -> Result<DraftSummary, Box<dyn std::error::Error + Send + Sync>> {
        Ok(DraftSummary {
            link: thread_link(workspace, &self.thread_id),
            thread_id: self.thread_id,
            title: Some(format_display_title(self.title, self.first_message))
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| UNTITLED_THREAD.to_string()),
            channel: self.source,
            state: ThreadState::from_db_str(&self.state)?,
            section: self.archive_state,
            status: ThreadStatus::parse(&self.status),
            parent_thread_id: self.parent_thread_id,
            drafted_by: self.drafted_by_thread_id,
            preview: text_preview(&self.compose_text),
            length: char_length(&self.compose_text),
            image_count: self.compose_images.as_array().map_or(0, Vec::len),
            last_edited: self.compose_updated_at,
            snapshot: with_text.then(|| DraftSnapshot {
                draft_revision: self.draft_revision,
                report: ComposeReport::from((
                    self.summary_version,
                    self.compose_epoch,
                    self.compose_text.clone(),
                    self.compose_images,
                    self.compose_mode,
                    self.compose_selection,
                    self.compose_restorations,
                )),
            }),
            text: with_text.then_some(self.compose_text),
        })
    }
}

impl EventStore {
    pub async fn list_drafts(
        &self,
        workspace: &str,
        limit: i64,
    ) -> Result<Vec<DraftSummary>, Box<dyn std::error::Error + Send + Sync>> {
        self.query_drafts(workspace, None, limit).await
    }

    pub async fn get_draft(
        &self,
        workspace: &str,
        thread_id: uuid::Uuid,
    ) -> Result<Option<DraftSummary>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self
            .query_drafts(workspace, Some(thread_id), 1)
            .await?
            .pop())
    }

    async fn query_drafts(
        &self,
        workspace: &str,
        thread_id: Option<uuid::Uuid>,
        limit: i64,
    ) -> Result<Vec<DraftSummary>, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            "SELECT t.thread_id::text AS thread_id, t.title, t.first_message, t.source, \
                    t.state, t.archive_state, t.status, \
                    t.parent_thread_id::text AS parent_thread_id, \
                    t.drafted_by_thread_id::text AS drafted_by_thread_id, \
                    t.compose_text, t.compose_images, t.compose_updated_at, \
                    t.summary_version, t.draft_revision, t.compose_epoch, t.compose_mode, t.compose_selection, t.compose_restorations \
             FROM thread_summaries t \
             WHERE {HAS_DRAFT_SQL} AND ($1::uuid IS NULL OR t.thread_id = $1) \
             ORDER BY t.compose_updated_at DESC NULLS LAST, t.last_activity DESC \
             LIMIT $2"
        );
        let rows = sqlx::query_as::<_, DraftRow>(&sql)
            .bind(thread_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| row.into_summary(workspace, thread_id.is_some()))
            .collect()
    }
}
