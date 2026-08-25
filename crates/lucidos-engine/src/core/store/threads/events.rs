use super::*;

impl EventStore {
    /// When the newest event on a thread was recorded. `Ok(None)` for a thread
    /// with no events yet.
    ///
    /// The one caller is the chat turn resolving its clock
    /// (`engine::chat::process::turn_clock`). Postgres stamps `created` with
    /// `NOW()`, so this is the database's own reading, on the same clock as
    /// every row it will be compared against.
    ///
    /// `MAX(created)` rather than the newest row by `sequence`:
    /// `replay_historical_event` can insert a backdated row at a high
    /// sequence, and a turn's clock must never run backwards because a
    /// backfill landed.
    pub async fn thread_latest_event_created(
        &self,
        thread_id: uuid::Uuid,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>, sqlx::Error> {
        sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
            "SELECT MAX(created) FROM events WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_one(&self.pool)
        .await
    }

    /// Get all events for a specific thread, ordered chronologically.
    ///
    /// `sequence` breaks a `created` tie, matching `get_thread_events_by_seq`
    /// and `get_recent_messages`. A burst inside one timestamp is common, and
    /// last-writer-wins folds over this stream need insertion order.
    pub async fn get_thread_events(
        &self,
        thread_id: &str,
    ) -> Result<Vec<EventRow>, Box<dyn std::error::Error + Send + Sync>> {
        let thread_uuid = uuid::Uuid::parse_str(thread_id)?;
        let events = sqlx::query_as::<_, EventRow>(
            r#"
            SELECT id, event_type, payload, created, thread_id, sequence
            FROM events
            WHERE thread_id = $1
            ORDER BY created ASC, sequence ASC
            "#,
        )
        .bind(thread_uuid)
        .fetch_all(&self.pool)
        .await?;

        Ok(events)
    }

    /// Get the content of the first user message in a thread (for title generation).
    /// Returns (text, image_description, image_count) for the first user message in a thread.
    ///
    /// `image_description` resolves through the typed `ImageDescribed` event
    /// emitted by the agentic loop after iteration 1 (latest-wins per source).
    /// Falls back to the legacy `MessageReceived.image_description` payload
    /// field for pre-backfill rows in the deprecation window.
    pub async fn get_thread_first_message(
        &self,
        thread_id: &str,
    ) -> Result<Option<(String, Option<String>, usize)>, Box<dyn std::error::Error + Send + Sync>>
    {
        let thread_uuid = uuid::Uuid::parse_str(thread_id)?;
        let row = sqlx::query_as::<_, (uuid::Uuid, serde_json::Value)>(
            r#"
            SELECT id, payload
            FROM events
            WHERE event_type = 'MessageReceived'
              AND thread_id = $1
            ORDER BY created ASC
            LIMIT 1
            "#,
        )
        .bind(thread_uuid)
        .fetch_optional(&self.pool)
        .await?;

        let Some((message_id, payload)) = row else {
            return Ok(None);
        };
        let Some(text) = payload
            .get("text")
            .and_then(|v| v.as_str())
            .or_else(|| payload.get("content").and_then(|v| v.as_str()))
            .map(|s| s.to_string())
        else {
            return Ok(None);
        };
        let image_count = payload
            .get("user_image_hashes")
            .and_then(|v| v.as_array())
            .map_or(0, |a| a.len());
        // Latest ImageDescribed for this MessageReceived (multiple hashes
        // attached to the same message all carry the same description text;
        // pick any of them).
        let described: Option<(String,)> = sqlx::query_as(
            "SELECT payload->>'description' \
             FROM events \
             WHERE event_type = 'ImageDescribed' \
               AND payload->>'source_event_id' = $1 \
               AND payload->>'description' IS NOT NULL \
             ORDER BY created DESC \
             LIMIT 1",
        )
        .bind(message_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        let image_desc = described.map(|(d,)| d).or_else(|| {
            payload
                .get("image_description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });
        Ok(Some((text, image_desc, image_count)))
    }

    /// Returns recent `MessageReceived` / `ResponseGenerated` events from a thread,
    /// formatted as oldest-first labeled lines for use as Gemini extraction context.
    ///
    /// Each line is `"User: <text>"` or `"Assistant: <text>"`. Lines are capped at
    /// 500 chars to avoid blowing up the system prompt for long messages. Returns
    /// empty string if the thread has no such events. The `exclude_event_id` is
    /// for the live consumer / rebuild path: skip the event being extracted so it
    /// doesn't appear in its own context.
    pub async fn recent_thread_messages_for_extraction(
        &self,
        thread_id: uuid::Uuid,
        limit: i64,
        exclude_event_id: Option<uuid::Uuid>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let rows = sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
            r#"
            SELECT event_type,
                   payload->>'text' AS text,
                   payload->>'content' AS content
            FROM events
            WHERE thread_id = $1
              AND event_type IN ('MessageReceived', 'ResponseGenerated')
              AND ($3::uuid IS NULL OR id <> $3)
            ORDER BY created DESC
            LIMIT $2
            "#,
        )
        .bind(thread_id)
        .bind(limit)
        .bind(exclude_event_id)
        .fetch_all(&self.pool)
        .await?;

        let lines: Vec<String> = rows
            .into_iter()
            .rev()
            .filter_map(|(event_type, text, content)| {
                let body = text.or(content)?;
                if body.trim().is_empty() {
                    return None;
                }
                let trimmed: String = body.chars().take(500).collect();
                let role = match event_type.as_str() {
                    "MessageReceived" => "User",
                    "ResponseGenerated" => "Assistant",
                    _ => return None,
                };
                Some(format!("{}: {}", role, trimmed))
            })
            .collect();

        Ok(lines.join("\n"))
    }

    /// Get all messages for a specific thread, built from its events.
    pub async fn get_thread_messages(
        &self,
        thread_id: &str,
    ) -> Result<Vec<SessionMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let events = self.get_thread_events(thread_id).await?;
        Ok(build_session_messages(&events))
    }

    /// Get timeline events for a thread (session lifecycle + change actions).
    pub async fn get_thread_timeline_events(
        &self,
        thread_id: &str,
    ) -> Result<Vec<ThreadTimelineEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let thread_uuid = uuid::Uuid::parse_str(thread_id)?;
        let rows = sqlx::query_as::<_, (String, chrono::DateTime<chrono::Utc>, serde_json::Value)>(
            r#"
            SELECT event_type, created, payload
            FROM events
            WHERE thread_id = $1
              AND event_type IN ('SessionStarted', 'SessionEnded', 'ChangeApplied', 'ChangeDiscarded', 'ChangeReverted')
            ORDER BY created ASC
            "#,
        )
        .bind(thread_uuid)
        .fetch_all(&self.pool)
        .await?;

        let mut result = Vec::new();
        for (event_type, created, payload) in rows {
            let change_id = payload
                .get("change_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Look up the description from the originating ChangeProposed
            // event in the events table — same payload, no separate side table.
            let description = if let Some(ref cid) = change_id {
                sqlx::query_scalar::<_, Option<String>>(
                    "SELECT payload->>'description' FROM events \
                     WHERE event_type = 'ChangeProposed' AND payload->>'change_id' = $1 \
                     ORDER BY sequence DESC LIMIT 1",
                )
                .bind(cid)
                .fetch_optional(&self.pool)
                .await?
                .flatten()
                .map(|d| d.lines().next().unwrap_or(&d).to_string())
            } else {
                None
            };

            // Convert PascalCase to snake_case for frontend
            let snake_type = match event_type.as_str() {
                "SessionStarted" => "session_started",
                "SessionEnded" => "session_ended",
                "ChangeApplied" => "change_applied",
                "ChangeDiscarded" => "change_discarded",
                "ChangeReverted" => "change_reverted",
                _ => continue,
            };

            result.push(ThreadTimelineEvent {
                event_type: snake_type.to_string(),
                timestamp: created.to_rfc3339(),
                description,
                change_id,
            });
        }
        Ok(result)
    }
}
