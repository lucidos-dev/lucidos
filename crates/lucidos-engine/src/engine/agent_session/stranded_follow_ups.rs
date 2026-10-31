//! Follow-ups a coding agent's process took into its input queue and died
//! holding. Plan: `docs/plans/2026-10-17-a-follow-up-a-dead-agent-never-read.md`.
//!
//! Claude Code keeps queued input in process memory. A process that dies
//! before it reads an input takes the input with it, and the user's bubble
//! stays "Queued" for good. The event store already says which inputs those
//! are (ADR 0236): a `CodingAgentPromptSent` names the message, and no
//! `CodingAgentInputRead` or `QueuedMessageRemoved` does. The next session
//! forwards each one again, and a Remove with no process to ask records its
//! tombstone from the same facts.

use std::path::Path;
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

use crate::api::ChatImage;
use crate::core::db_retry::retry_transient_read;
use crate::core::store::{
    format_child_report_filed_block, format_child_thread_completed_block, EventStore,
};
use crate::core::EventRow;
use crate::engine::thread_events::{ActorMode, CACHE_KEEP_ALIVE_REASON_KIND};
use crate::engine::types::{AgentSession, AgentUserInput, RedeliveredInput};
use crate::engine::AgentInputKind;

/// Nothing settled `m` on thread `$1`: no agent read it, and the user did not
/// take it back.
const UNSETTLED: &str = "\
NOT EXISTS ( \
         SELECT 1 FROM events r \
          WHERE r.thread_id = $1 \
            AND r.event_type = 'CodingAgentInputRead' \
            AND r.payload->>'input_event_id' = m.id::text) \
   AND NOT EXISTS ( \
         SELECT 1 FROM events t \
          WHERE t.thread_id = $1 \
            AND t.event_type = 'QueuedMessageRemoved' \
            AND t.payload->>'removed_message_id' = m.id::text)";

/// A human `MessageReceived` `m` on thread `$1` that nothing settled.
static OWED_HUMAN_MESSAGE: LazyLock<String> = LazyLock::new(|| {
    format!(
        "m.event_type = 'MessageReceived' \
         AND COALESCE(m.payload->>'mode', 'human') = 'human' \
         AND {UNSETTLED}"
    )
});

/// The input kinds a discard leaves owed, one per [`RedeliveredInput`]. A
/// continuation is not one: a later resume sends its own nudge. A cache
/// keep-alive only warms the cache.
static OWED_KIND: LazyLock<String> = LazyLock::new(|| {
    format!(
        "(m.event_type IN ('MessageReceived', 'ChildThreadCompleted', 'ChildReportFiled', \
                           'CodingAgentPromptSent') \
          OR (m.event_type = 'PromptInjected' \
              AND COALESCE(m.payload->'origin'->'reason'->>'kind', '') \
                  <> '{CACHE_KEEP_ALIVE_REASON_KIND}'))"
    )
});

/// An effort restart discarded `m` before the agent read it. Older threads
/// carry these; nothing writes one since the rung locks for a turn (ADR 0542).
const DISCARDED: &str = "\
EXISTS ( \
         SELECT 1 FROM events d \
          WHERE d.thread_id = $1 \
            AND d.event_type = 'CodingAgentInputDiscarded' \
            AND d.payload->>'input_event_id' = m.id::text)";

/// The one definition of a *discarded input* still owed, over alias `m` and
/// thread id `$1`.
static DISCARDED_AND_OWED: LazyLock<String> =
    LazyLock::new(|| format!("{} AND {UNSETTLED} AND {DISCARDED}", *OWED_KIND));

/// The columns of an [`EventRow`], over alias `m`.
pub(crate) const OWED_ROW_COLUMNS: &str =
    "m.id, m.event_type, m.payload, m.created, m.thread_id, m.sequence";

/// The stranded follow-ups of the thread, oldest first.
///
/// `$1` thread id, `$2` the events the starting session sends as its first
/// prompt, `$3` [`CACHE_KEEP_ALIVE_REASON_KIND`].
///
/// An input is stranded for one of two reasons:
///
/// * it is a human message nothing settled, and a `CodingAgentPromptSent`
///   names it after the latest `SessionStarted`: the latest session forwarded
///   it. The caller asks before it records its own `SessionStarted`, so
///   "latest" is the previous session. A cache keep-alive forwards nothing and
///   does not count, or its beat would hide the message. Older rows stay as
///   they are: replaying a stale message answers nobody.
/// * it is [`DISCARDED_AND_OWED`]: an older effort restart discarded it, in
///   whichever session. It stays owed until some session's agent reads it.
pub(crate) static STRANDED_FOLLOW_UPS_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "\
SELECT {OWED_ROW_COLUMNS} \
  FROM events m \
 WHERE m.thread_id = $1 \
   AND m.id <> ALL($2) \
   AND (({owed_human_message} \
         AND EXISTS ( \
         SELECT 1 FROM events p \
          WHERE p.thread_id = $1 \
            AND p.event_type = 'CodingAgentPromptSent' \
            AND p.payload->>'input_event_id' = m.id::text \
            AND p.sequence > ( \
                  SELECT COALESCE(MAX(s.sequence), 0) FROM events s \
                   WHERE s.thread_id = $1 AND s.event_type = 'SessionStarted' \
                     AND NOT EXISTS ( \
                           SELECT 1 FROM events k \
                            WHERE k.thread_id = $1 \
                              AND k.event_type = 'PromptInjected' \
                              AND k.id::text = s.payload->>'request_event_id' \
                              AND k.payload->'origin'->'reason'->>'kind' = $3)))) \
        OR {discarded_and_owed}) \
 ORDER BY m.sequence ASC",
        owed_human_message = *OWED_HUMAN_MESSAGE,
        discarded_and_owed = *DISCARDED_AND_OWED,
    )
});

/// An owed input's row, with its kind and the text to send the agent again.
pub(crate) struct RebuiltInput {
    pub row: EventRow,
    pub input: RedeliveredInput,
    pub text: String,
}

/// Rebuild what each owed row told the agent, oldest first as given.
///
/// A message or an engine prompt sends its payload `text`. A child report
/// sends its card again, with each change's status as it is now, so a change
/// that moved on since says so.
pub(crate) async fn rebuilt_inputs(pool: &sqlx::PgPool, rows: Vec<EventRow>) -> Vec<RebuiltInput> {
    let statuses = EventStore::new(pool.clone())
        .reported_change_statuses(&rows)
        .await;
    rows.into_iter()
        .filter_map(|row| {
            let input = RedeliveredInput::of_row(&row.event_type, &row.payload)?;
            let text = match (input, row.event_type.as_str()) {
                (RedeliveredInput::ChildReport, "ChildReportFiled") => {
                    format_child_report_filed_block(&row, &statuses)
                }
                (RedeliveredInput::ChildReport, _) => {
                    format_child_thread_completed_block(&row, &statuses)
                }
                _ => row
                    .payload
                    .get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .to_string(),
            };
            Some(RebuiltInput { row, input, text })
        })
        .collect()
}

/// One input a session dropped before its agent read it.
#[derive(Debug, Clone)]
pub(crate) struct StrandedFollowUp {
    /// Its event, which the redelivery's read names.
    pub event_id: Uuid,
    pub input: RedeliveredInput,
    pub text: String,
    pub images: Option<Vec<ChatImage>>,
    pub sent_at: DateTime<Utc>,
}

/// Run [`STRANDED_FOLLOW_UPS_SQL`] and rebuild each input with its images.
/// A dropped connection is retried, so a Postgres restart does not fail the
/// session that asks.
pub(crate) async fn stranded_follow_ups(
    pool: &sqlx::PgPool,
    workspace: &Path,
    thread_id: Uuid,
    first_prompt: &[Uuid],
) -> Result<Vec<StrandedFollowUp>, sqlx::Error> {
    let rows: Vec<EventRow> = retry_transient_read("The stranded follow-ups check", || {
        sqlx::query_as(STRANDED_FOLLOW_UPS_SQL.as_str())
            .bind(thread_id)
            .bind(first_prompt)
            .bind(CACHE_KEEP_ALIVE_REASON_KIND)
            .fetch_all(pool)
    })
    .await?;
    Ok(rebuilt_inputs(pool, rows)
        .await
        .into_iter()
        .map(|RebuiltInput { row, input, text }| StrandedFollowUp {
            event_id: row.id,
            input,
            text,
            images: crate::engine::chat::queued_recovery::images_for_row(workspace, &row.payload),
            sent_at: row.created,
        })
        .collect())
}

/// What became of a follow-up no live process holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputFate {
    /// The agent read it, in some session.
    Read,
    /// A session forwarded it and no agent read it: only a dead process held it.
    Stranded,
    /// No session forwarded it yet, so a spawn may still be on its way to it.
    NeverForwarded,
}

/// Ask the store what became of `input_event_id`'s input.
pub(crate) async fn input_fate(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    input_event_id: Uuid,
) -> Result<InputFate, sqlx::Error> {
    let (forwarded, read): (bool, bool) = sqlx::query_as(
        "SELECT \
           EXISTS (SELECT 1 FROM events WHERE thread_id = $1 \
                     AND event_type = 'CodingAgentPromptSent' \
                     AND payload->>'input_event_id' = $2), \
           EXISTS (SELECT 1 FROM events WHERE thread_id = $1 \
                     AND event_type = 'CodingAgentInputRead' \
                     AND payload->>'input_event_id' = $2)",
    )
    .bind(thread_id)
    .bind(input_event_id.to_string())
    .fetch_one(pool)
    .await?;
    Ok(match (read, forwarded) {
        (true, _) => InputFate::Read,
        (false, true) => InputFate::Stranded,
        (false, false) => InputFate::NeverForwarded,
    })
}

/// The note in front of a redelivered input. It names who sent it and when, so
/// the agent can weigh it against what happened since.
pub(crate) fn late_delivery_note(input: RedeliveredInput, sent_at: DateTime<Utc>) -> String {
    let sent_at = sent_at.format("%Y-%m-%d %H:%M");
    let (what, sender) = match input {
        RedeliveredInput::Message {
            sender: ActorMode::Human,
        } => {
            return format!(
                "[The previous session ended before you read the message below. \
                 The user sent it at {sent_at} UTC.]"
            )
        }
        RedeliveredInput::Message {
            sender: ActorMode::Engine,
        }
        | RedeliveredInput::EnginePrompt => ("message", "Lucidos"),
        RedeliveredInput::Message {
            sender: ActorMode::Agent,
        } => ("message", "Another thread"),
        RedeliveredInput::ChildReport => ("child thread's report", "The child thread"),
    };
    format!(
        "[The previous session ended before you read the {what} below. {sender} sent it at \
         {sent_at} UTC. Check that its request still applies before you act on it.]"
    )
}

impl crate::engine::LucidosEngine {
    /// Queue `stranded`, from [`stranded_follow_ups`], on the starting
    /// session's channel, behind its first prompt and ahead of anything sent
    /// during the startup.
    pub(crate) async fn redeliver_stranded_follow_ups(
        &self,
        thread_id: Uuid,
        msg_tx: &UnboundedSender<AgentUserInput>,
        msg_rx: &mut UnboundedReceiver<AgentUserInput>,
        stranded: Vec<StrandedFollowUp>,
    ) {
        if stranded.is_empty() {
            return;
        }
        // Under the lock every sender takes, so nothing slips in between.
        let mut sessions = self.agent_sessions.lock().await;
        let Some(session) = sessions
            .get_mut(&thread_id)
            .filter(|s| s.msg_tx.same_channel(msg_tx))
        else {
            return;
        };
        log!(
            "[AgentSession] Redelivering {} follow-up(s) the previous session never read on thread {}",
            stranded.len(),
            thread_id
        );
        queue_ahead_of_startup_input(session, msg_rx, stranded);
    }
}

/// Queue `stranded` oldest first, then put back what arrived during startup.
fn queue_ahead_of_startup_input(
    session: &mut AgentSession,
    msg_rx: &mut UnboundedReceiver<AgentUserInput>,
    stranded: Vec<StrandedFollowUp>,
) {
    let sent_during_startup: Vec<_> = std::iter::from_fn(|| msg_rx.try_recv().ok()).collect();
    for follow_up in stranded {
        queue_follow_up(session, follow_up);
    }
    for input in sent_during_startup {
        let _ = session.msg_tx.send(input);
    }
}

/// Queue one stranded follow-up as its own input. It is marked unforwarded
/// first, so a withdraw waits for its forward rather than overtaking it.
fn queue_follow_up(session: &mut AgentSession, follow_up: StrandedFollowUp) {
    session.unforwarded_inputs.insert(follow_up.event_id);
    // The caller holds the open receiver, so the send cannot fail.
    let _ = session.msg_tx.send(AgentUserInput {
        text: follow_up.text,
        images: follow_up.images,
        origin_event_id: Some(follow_up.event_id),
        kind: AgentInputKind::Redelivered {
            sent_at: follow_up.sent_at,
            input: follow_up.input,
        },
        completion: None,
    });
}

#[cfg(test)]
#[path = "stranded_follow_ups_tests.rs"]
mod tests;
