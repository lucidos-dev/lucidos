//! Query the last terminal of a coding-agent turn.

use uuid::Uuid;

/// Return whether the latest Claude Code terminal event is `ResponseGenerated`.
///
/// Stale-session recovery uses this to withhold work from an unfinished turn
/// (ADR 0400). Tier-1 conflict resolution uses it to verify that the resolving
/// turn finished.
///
/// Restrict the query to the Claude Code channel. A later chat or trigger
/// response cannot establish whether the coding turn finished. A missing Claude
/// Code terminal event also means the turn did not finish cleanly.
pub async fn last_turn_ended_cleanly(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    last_turn_end(pool, thread_id).await == Some(TurnEnd::Finished)
}

/// How a thread's last coding-agent turn ended, for a recovery sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEnd {
    /// It finished, so its work is proposed.
    Finished,
    /// A Stop, a failure or an abort cut it short, so its work is withheld
    /// (ADR 0400).
    Unfinished,
}

/// How the thread's last coding-agent turn ended, mirroring the live idle
/// gate. `None` for a redirect, an Apply or Discard stop, no turn at all, or a
/// failed read: a sweep then settles nothing.
pub async fn last_turn_end(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<TurnEnd> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT event_type, payload->>'cause' FROM events \
         WHERE thread_id = $1 \
           AND event_type IN ('ResponseGenerated', 'ResponseAborted', 'ResponseCanceled', 'ResponseFailed') \
           AND payload->>'channel' = 'claude_code' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        log_legacy!(
            "[Recovery] last_turn_end({}): {}; settling nothing",
            thread_id,
            e
        );
        None
    });
    let (event_type, cause) = row?;
    match (event_type.as_str(), cause.as_deref()) {
        ("ResponseGenerated", _) => Some(TurnEnd::Finished),
        ("ResponseCanceled", Some("user_stop")) | ("ResponseFailed" | "ResponseAborted", _) => {
            Some(TurnEnd::Unfinished)
        }
        _ => None,
    }
}

/// Send `text` to a live session as a prompt the caller waits on. The receiver
/// is answered when the turn that reads this prompt ends.
pub fn send_awaited_prompt(
    msg_tx: &tokio::sync::mpsc::UnboundedSender<super::AgentUserInput>,
    text: String,
) -> Result<
    tokio::sync::oneshot::Receiver<super::InputAnswered>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    let (completion, answered) = tokio::sync::oneshot::channel();
    msg_tx
        .send(super::AgentUserInput {
            text,
            images: None,
            origin_event_id: None,
            kind: crate::engine::AgentInputKind::User,
            completion: Some(completion),
        })
        .map_err(|_| "Session channel closed")?;
    Ok(answered)
}
