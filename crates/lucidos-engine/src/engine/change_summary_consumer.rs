//! EventBus consumer that writes the *change summary*: one line saying what a
//! change of several commits does.
//!
//! Subscribes to aggregate `ChangeProposed` and answers with
//! `ChangeSummarized`. The summary is a courtesy to the reader, so nothing
//! waits on it: proposing and applying never see this module, and a failed
//! call leaves the change without a summary rather than without an Apply.
//!
//! A single-commit change needs no call, since its subject already is the line.
//! The projection keeps a summary only while its commit list is current, so a
//! slow answer racing a newer proposal is dropped there
//! (`ChangesProjection::write_summary`).
//!
//! On start it also summarizes the changes the Changes panel shows that still
//! lack one. That covers a proposal made before this consumer subscribed, which
//! startup recovery does, and every change proposed before summaries existed.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use super::event_bus::{BusEvent, EventBus};
use super::thread_events::{EventMeta, ThreadEvent};
use super::LucidosEngine;
use crate::core::changes_projection::ChangesProjection;
use crate::llm::provider::LlmProvider;

const SYSTEM_PROMPT: &str = "You summarize one code change for the person deciding whether to \
     apply it. You get the change's commit subjects, oldest first. Write ONE line of at most \
     12 words saying what the change does as a whole. Lead with the main piece of work, not \
     the latest fix or cleanup. Use plain words in sentence case. No conventional-commit \
     prefix such as 'feat:' or 'fix(engine):', no quotes, no trailing period. Return only \
     the line.";

/// The longest summary kept. A reply past this is an explanation, not a line.
const MAX_SUMMARY_CHARS: usize = 100;

/// How much of the commit list the model reads, so a long branch still fits
/// the title-sized model this runs on.
const MAX_INPUT_CHARS: usize = 4000;

/// Smaller models sometimes answer with a paragraph. One resample usually
/// returns a real line, the way titling does.
const MAX_ATTEMPTS: usize = 2;

/// Changes being summarized. One task per change at a time: it re-reads the row
/// after each call, so a proposal skipped here is still summarized.
type InFlight = Arc<Mutex<HashSet<Uuid>>>;

/// Spawn the consumer. Returns the `JoinHandle` so a caller can observe panics.
pub fn spawn(engine: Arc<LucidosEngine>) -> tokio::task::JoinHandle<()> {
    // Subscribe before the backfill reads, so nothing proposed in between is
    // missed by both.
    let rx = engine.event_bus.subscribe();
    let in_flight: InFlight = Arc::default();
    tokio::spawn(backfill(engine.clone(), in_flight.clone()));
    tokio::spawn(async move {
        let stream = BroadcastStream::new(rx);
        tokio::pin!(stream);
        // `while let Some(Ok(_))` would end the loop on a lag and silently stop
        // summarizing for the rest of the engine's life.
        while let Some(result) = stream.next().await {
            let emitted = match result {
                Ok(e) => e,
                Err(BroadcastStreamRecvError::Lagged(n)) => {
                    log!(
                        "[ChangeSummary] Broadcast lagged by {} events, their summaries are skipped",
                        n
                    );
                    continue;
                }
            };
            if emitted.seq.is_none() {
                continue;
            }
            let BusEvent::Thread {
                thread_id,
                event:
                    ThreadEvent::ChangeProposed {
                        change_id,
                        commit_sha: None,
                        ..
                    },
                ..
            } = &emitted.typed
            else {
                continue;
            };
            let Ok(change_id) = Uuid::parse_str(change_id) else {
                continue;
            };
            let (engine, in_flight, thread_id) = (engine.clone(), in_flight.clone(), *thread_id);
            tokio::spawn(async move {
                summarize_guarded(&engine, &in_flight, thread_id, change_id).await
            });
        }
    })
}

/// Summarize the changes the Changes panel lists that still lack a summary:
/// every pending one, and the recently applied window. One at a time, so a
/// restart does not fire a burst of model calls.
async fn backfill(engine: Arc<LucidosEngine>, in_flight: InFlight) {
    let projection = ChangesProjection::new(engine.pool().clone());
    let listed = tokio::try_join!(
        projection.list_pending(),
        projection.list_recently_applied(crate::engine::change_ops::APPLIED_IN_BROADCAST, None),
    );
    let (pending, applied) = match listed {
        Ok(lists) => lists,
        Err(e) => {
            log!("[ChangeSummary] Backfill could not list changes: {}", e);
            return;
        }
    };
    let targets = backfill_targets(pending.iter().chain(&applied));
    if !targets.is_empty() {
        log!(
            "[ChangeSummary] Backfilling {} change summaries",
            targets.len()
        );
    }
    for (thread_id, change_id) in targets {
        summarize_guarded(&engine, &in_flight, thread_id, change_id).await;
    }
}

/// The changes a backfill summarizes: those with a thread to record the call
/// on that still need a summary.
pub(crate) fn backfill_targets<'a>(
    changes: impl Iterator<Item = &'a crate::core::changes::Change>,
) -> Vec<(Uuid, Uuid)> {
    changes
        .filter(|c| needs_summary(&c.description, c.summary.as_deref()))
        .filter_map(|c| c.thread_id.map(|thread_id| (thread_id, c.id)))
        .collect()
}

/// [`summarize_change`] behind the in-flight guard. A change already being
/// summarized is skipped, since that task re-reads the row when it finishes.
async fn summarize_guarded(
    engine: &LucidosEngine,
    in_flight: &InFlight,
    thread_id: Uuid,
    change_id: Uuid,
) {
    if !in_flight.lock().unwrap().insert(change_id) {
        return;
    }
    summarize_change(engine, thread_id, change_id).await;
    in_flight.lock().unwrap().remove(&change_id);
}

/// Everything one summary call runs under.
pub(crate) struct SummaryCall {
    provider: Arc<dyn LlmProvider>,
    effort: Option<String>,
    /// Covers both attempts, not each one.
    deadline: Duration,
}

impl SummaryCall {
    async fn resolve(
        pool: &sqlx::PgPool,
        extractor: &crate::memory::MemoryExtractor,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let call = crate::engine::aux_purpose::AuxCall::resolve(
            pool,
            crate::engine::ContextPurpose::ChangeSummary,
        )
        .await;
        Ok(Self {
            provider: extractor.provider_for_model(call.model(), call.attempt_timeout())?,
            effort: call.reasoning().map(str::to_string),
            deadline: call.deadline(),
        })
    }
}

/// Summarize until the change's commit list stops moving. The in-flight guard
/// skips a proposal that lands during a call. That call's answer is then
/// stale, so the loop reads the row again and goes on.
async fn summarize_change(engine: &LucidosEngine, thread_id: Uuid, change_id: Uuid) {
    // Bounds a branch that keeps gaining commits faster than a call returns.
    const MAX_ROUNDS: usize = 3;
    let mut attempted: Option<String> = None;
    for _ in 0..MAX_ROUNDS {
        let change = match ChangesProjection::new(engine.pool().clone())
            .get_by_id(change_id)
            .await
        {
            Ok(Some(change)) => change,
            Ok(None) => return,
            Err(e) => {
                log!("[ChangeSummary] Could not read change {}: {}", change_id, e);
                return;
            }
        };
        let already_tried = attempted.as_deref() == Some(change.description.as_str());
        if already_tried || !needs_summary(&change.description, change.summary.as_deref()) {
            return;
        }
        summarize_once(engine, thread_id, change_id, &change.description).await;
        attempted = Some(change.description);
    }
}

async fn summarize_once(
    engine: &LucidosEngine,
    thread_id: Uuid,
    change_id: Uuid,
    description: &str,
) {
    let Some(extractor) = engine.extractor() else {
        log!(
            "[ChangeSummary] No background model is configured, change {} keeps its commit subject",
            change_id
        );
        return;
    };
    let call = match SummaryCall::resolve(engine.pool(), extractor).await {
        Ok(call) => call,
        Err(e) => {
            log!(
                "[ChangeSummary] Could not build the model for {}: {}",
                change_id,
                e
            );
            return;
        }
    };
    let started = std::time::Instant::now();
    match write_summary(&engine.event_bus, &call, thread_id, change_id, description).await {
        Ok(summary) => {
            log!(
                "[ChangeSummary] Summarized {} in {:?}: {:?}",
                change_id,
                started.elapsed(),
                summary
            );
            // The Changes panel reads its lists from this frame.
            engine.broadcast_changes_updated().await;
        }
        Err(e) => log!(
            "[ChangeSummary] No summary for {} after {:?}: {}",
            change_id,
            started.elapsed(),
            e
        ),
    }
}

/// A change earns a summary when it has two or more commits and no summary of
/// its current commit list. The projection clears a stale one, so "has a
/// summary" means "has a current one".
pub(crate) fn needs_summary(description: &str, summary: Option<&str>) -> bool {
    summary.is_none() && commit_subjects(description).count() >= 2
}

fn commit_subjects(description: &str) -> impl DoubleEndedIterator<Item = &str> {
    description
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
}

/// The commit subjects the model reads: whole lines, oldest first, up to
/// [`MAX_INPUT_CHARS`]. The description is newest first, and on a long branch
/// the oldest subjects name the main work, so the cap drops the newest.
pub(crate) fn model_input(description: &str) -> String {
    let mut input = String::new();
    for subject in commit_subjects(description).rev() {
        let needed = subject.chars().count() + usize::from(!input.is_empty());
        if input.chars().count() + needed > MAX_INPUT_CHARS {
            break;
        }
        if !input.is_empty() {
            input.push('\n');
        }
        input.push_str(subject);
    }
    input
}

/// Ask the model, record every attempt's cost, and emit `ChangeSummarized`.
pub(crate) async fn write_summary(
    bus: &EventBus,
    call: &SummaryCall,
    thread_id: Uuid,
    change_id: Uuid,
    description: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let capture = crate::engine::AuxCapture::new(
        bus,
        thread_id,
        crate::engine::ContextPurpose::ChangeSummary,
    );
    let summary =
        match tokio::time::timeout(call.deadline, summary_attempts(call, description, &capture))
            .await
        {
            Ok(result) => result?,
            Err(_) => return Err(format!("timed out after {:?}", call.deadline).into()),
        };
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeSummarized {
            change_id: change_id.to_string(),
            summary: summary.clone(),
            description: description.to_string(),
        },
        meta: EventMeta::NONE,
    })
    .await?;
    Ok(summary)
}

async fn summary_attempts(
    call: &SummaryCall,
    description: &str,
    capture: &crate::engine::AuxCapture,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use crate::llm::provider::{Message, MessageContent};

    let input = model_input(description);
    let request_chars = SYSTEM_PROMPT.chars().count() + input.chars().count();
    let mut last_err: Box<dyn std::error::Error + Send + Sync> =
        "the model produced no candidate".into();
    for _ in 0..MAX_ATTEMPTS {
        let response = call
            .provider
            .chat(
                vec![Message {
                    role: "user".to_string(),
                    content: MessageContent::Text(input.clone()),
                }],
                vec![],
                crate::llm::ModelSelection::default().with_effort(call.effort.as_deref()),
                Some(SYSTEM_PROMPT),
                None,
            )
            .await?;
        capture
            .record(call.provider.default_model(), request_chars, &response)
            .await;
        match validate_summary(response.content.as_deref().unwrap_or_default()) {
            Ok(summary) => return Ok(summary),
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

/// One line, trimmed, unquoted, with no trailing period, and short enough to
/// be a headline. Anything else is the model explaining instead of summarizing.
pub(crate) fn validate_summary(
    reply: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let trimmed = reply.trim();
    if trimmed.contains('\n') {
        return Err(format!("the reply is more than one line: {:?}", trimmed).into());
    }
    let line = trimmed
        .trim_matches(|c| matches!(c, '"' | '\'' | '`' | '“' | '”'))
        .trim()
        .trim_end_matches('.')
        .trim();
    if line.is_empty() {
        return Err("the reply is empty".into());
    }
    if line.chars().count() > MAX_SUMMARY_CHARS {
        return Err(format!("the reply is too long for a headline: {:?}", line).into());
    }
    Ok(line.to_string())
}

#[cfg(test)]
#[path = "change_summary_consumer_tests.rs"]
mod tests;
