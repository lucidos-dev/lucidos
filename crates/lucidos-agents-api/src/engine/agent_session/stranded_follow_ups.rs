use std::path::Path;
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

use crate::core::db_retry::retry_transient_read;
use crate::core::EventRow;
use crate::engine::replay_inputs::{rebuilt_inputs, RebuiltInput};
use crate::engine::thread_events::{ActorMode, CACHE_KEEP_ALIVE_REASON_KIND};
use crate::engine::types::ChatImage;
use crate::engine::types::{AgentSession, AgentUserInput, RedeliveredInput};
use crate::engine::AgentInputKind;

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

static OWED_HUMAN_MESSAGE: LazyLock<String> = LazyLock::new(|| {
    format!(
        "m.event_type = 'MessageReceived' \
         AND COALESCE(m.payload->>'mode', 'human') = 'human' \
         AND {UNSETTLED}"
    )
});

static OWED_KIND: LazyLock<String> = LazyLock::new(|| {
    format!(
        "(m.event_type IN ('MessageReceived', 'ChildThreadCompleted', 'ChildReportFiled', \
                           'CodingAgentPromptSent') \
          OR (m.event_type = 'PromptInjected' \
              AND COALESCE(m.payload->'origin'->'reason'->>'kind', '') \
                  <> '{CACHE_KEEP_ALIVE_REASON_KIND}'))"
    )
});

const DISCARDED: &str = "\
EXISTS ( \
         SELECT 1 FROM events d \
          WHERE d.thread_id = $1 \
            AND d.event_type = 'CodingAgentInputDiscarded' \
            AND d.payload->>'input_event_id' = m.id::text)";

static DISCARDED_AND_OWED: LazyLock<String> =
    LazyLock::new(|| format!("{} AND {UNSETTLED} AND {DISCARDED}", *OWED_KIND));

pub(crate) const OWED_ROW_COLUMNS: &str =
    "m.id, m.event_type, m.payload, m.created, m.thread_id, m.sequence";

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

#[derive(Debug, Clone)]
pub(crate) struct StrandedFollowUp {
    pub event_id: Uuid,
    pub input: RedeliveredInput,
    pub text: String,
    pub images: Option<Vec<ChatImage>>,
    pub sent_at: DateTime<Utc>,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputFate {
    Read,
    Stranded,
    NeverForwarded,
}

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

#[easy_ext::ext(StrandedFollowUpsExt)]
pub(crate) impl crate::engine::LucidosEngine {
    async fn redeliver_stranded_follow_ups(
        &self,
        thread_id: Uuid,
        msg_tx: &UnboundedSender<AgentUserInput>,
        msg_rx: &mut UnboundedReceiver<AgentUserInput>,
        stranded: Vec<StrandedFollowUp>,
    ) {
        if stranded.is_empty() {
            return;
        }
        // INVARIANT: Under the lock every sender takes, so nothing slips in between.
        let mut sessions = self.agent_sessions.lock().await;
        let Some(session) = sessions
            .get_mut(&thread_id)
            .filter(|s| s.msg_tx.same_channel(msg_tx))
        else {
            return;
        };
        log_legacy!(
            "[AgentSession] Redelivering {} follow-up(s) the previous session never read on thread {}",
            stranded.len(),
            thread_id
        );
        queue_ahead_of_startup_input(session, msg_rx, stranded);
    }
}

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

/// INVARIANT: Queue one stranded follow-up as its own input. It is marked unforwarded
/// first, so a withdraw waits for its forward rather than overtaking it.
fn queue_follow_up(session: &mut AgentSession, follow_up: StrandedFollowUp) {
    session.unforwarded_inputs.insert(follow_up.event_id);
    // INVARIANT: The caller holds the open receiver, so the send cannot fail.
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
