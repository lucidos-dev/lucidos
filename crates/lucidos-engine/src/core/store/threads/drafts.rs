//! A thread's *draft*, and the *reader fields* that say on every row what a
//! thread holds and how to link it.
//!
//! The draft is what the user typed into the composer and has not sent. It
//! lives on the thread's projection row (`compose_*` columns), and the compose
//! PUT is its only writer. Everything here only reads it. A draft has no
//! address of its own, so a reader links to its owning thread.

use super::*;

/// How many characters a preview of unsent text carries: a draft, or a held
/// message.
pub const PREVIEW_CHARS: usize = 200;

/// SQL truth of [`has_draft`] over `thread_summaries t`. A discarded thread
/// can hold neither: its discard clears the compose fields.
pub const HAS_DRAFT_SQL: &str = "(t.compose_text <> '' \
     OR (jsonb_typeof(t.compose_images) = 'array' AND jsonb_array_length(t.compose_images) > 0))";

/// Whether a thread holds a draft: any text, or at least one image. The one
/// definition, shared with [`HAS_DRAFT_SQL`] and the thread action guard.
pub fn has_draft(text: &str, images: &serde_json::Value) -> bool {
    !text.is_empty() || images.as_array().is_some_and(|a| !a.is_empty())
}

/// The first [`PREVIEW_CHARS`] characters of `text`. Counts characters,
/// so multibyte text is never cut inside a character.
pub fn text_preview(text: &str) -> String {
    text.chars().take(PREVIEW_CHARS).collect()
}

/// `text`'s whole length in characters, the unit [`text_preview`] cuts in.
pub fn char_length(text: &str) -> usize {
    text.chars().count()
}

/// The *thread link*: the markdown link target that opens a thread,
/// `thread:<workspace>/<thread_id>`. The workspace is always the serving
/// engine's own, never a caller's.
pub fn thread_link(workspace: &str, thread_id: impl std::fmt::Display) -> String {
    format!("thread:{workspace}/{thread_id}")
}

/// Fill the *reader fields* on rows an agent, the CLI or a script reads:
/// whether each holds a draft, its preview, and its thread link.
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

/// One thread's draft, as the `threads` tool's `drafts` action reports it.
#[derive(Debug, Clone, Serialize)]
pub struct DraftSummary {
    pub thread_id: String,
    pub title: String,
    pub channel: String,
    /// `composing` for a thread never sent, `active` for one with history.
    pub state: ThreadState,
    /// `inbox` or `archived`. A draft survives an archive.
    pub section: String,
    pub status: ThreadStatus,
    pub parent_thread_id: Option<String>,
    pub preview: String,
    /// The whole draft's length in characters.
    pub length: usize,
    pub image_count: usize,
    /// When the draft was last written. `None` for a draft written before
    /// the engine recorded edit times.
    pub last_edited: Option<chrono::DateTime<chrono::Utc>>,
    /// The whole draft, carried only when one thread's draft was asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub link: String,
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
    compose_text: String,
    compose_images: serde_json::Value,
    compose_updated_at: Option<chrono::DateTime<chrono::Utc>>,
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
            // A never-sent thread has no title and no first message yet. The
            // agent still needs a name to put on its link.
            title: Some(format_display_title(self.title, self.first_message))
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| UNTITLED_THREAD.to_string()),
            channel: self.source,
            state: ThreadState::from_db_str(&self.state)?,
            section: self.archive_state,
            status: ThreadStatus::parse(&self.status),
            parent_thread_id: self.parent_thread_id,
            preview: text_preview(&self.compose_text),
            length: char_length(&self.compose_text),
            image_count: self.compose_images.as_array().map_or(0, Vec::len),
            last_edited: self.compose_updated_at,
            text: with_text.then_some(self.compose_text),
        })
    }
}

impl EventStore {
    /// Every thread holding a draft, newest edit first. Threads whose edit
    /// time is unknown follow, by recency.
    pub async fn list_drafts(
        &self,
        workspace: &str,
        limit: i64,
    ) -> Result<Vec<DraftSummary>, Box<dyn std::error::Error + Send + Sync>> {
        self.query_drafts(workspace, None, limit).await
    }

    /// One thread's draft with its whole text. `None` when the thread holds
    /// no draft, or does not exist.
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
                    t.compose_text, t.compose_images, t.compose_updated_at \
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
