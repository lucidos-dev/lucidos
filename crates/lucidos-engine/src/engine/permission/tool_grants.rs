use std::collections::BTreeSet;
use std::path::Path;

use tokio::sync::Mutex;
use uuid::Uuid;

use super::{action_wire, Holder, Lifetime, PickedBy};
use crate::core::grants::{self, GrantFile, GrantSource};
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::AllowScope;
use crate::engine::thread_events::{
    AnswerKind, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub(crate) static STANDING_LOCK: Mutex<()> = Mutex::const_new(());

const IMPORTED_UNIQUE_INDEX: &str = "events_grant_file_imported_unique";

pub use crate::engine::thread_events::permission::{lane_action, lane_of};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GrantedLines {
    pub standing: Vec<String>,
    pub thread: Vec<String>,
}

impl GrantedLines {
    pub fn contains(&self, pattern: &str) -> bool {
        self.standing
            .iter()
            .chain(&self.thread)
            .any(|p| p == pattern)
    }
}

pub async fn granted_lines(
    bus: &EventBus,
    file: GrantFile,
    thread_id: Option<Uuid>,
) -> GrantedLines {
    live_lines(bus.pool(), file, thread_id)
        .await
        .unwrap_or_else(|e| {
            crate::log_legacy!(
                "[ToolGrants] Could not read the {} grants: {e}. Treating as none.",
                file.file_name()
            );
            GrantedLines::default()
        })
}

async fn live_lines(
    pool: &sqlx::PgPool,
    file: GrantFile,
    thread_id: Option<Uuid>,
) -> Result<GrantedLines, BoxError> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT holder_kind, pattern FROM permissions \
         WHERE action = $1 AND pattern IS NOT NULL AND ended_at IS NULL \
           AND (holder_kind = 'workspace' OR (holder_kind = 'thread' AND holder_id = $2)) \
         ORDER BY granted_at, pattern COLLATE \"C\"",
    )
    .bind(action_wire(lane_action(file))?)
    .bind(thread_id.map(|t| t.to_string()))
    .fetch_all(pool)
    .await?;
    let mut lines = GrantedLines::default();
    for (holder_kind, pattern) in rows {
        match holder_kind.as_str() {
            "workspace" => lines.standing.push(pattern),
            _ => lines.thread.push(pattern),
        }
    }
    Ok(lines)
}

pub async fn standing_lines(pool: &sqlx::PgPool, file: GrantFile) -> Result<Vec<String>, BoxError> {
    Ok(live_lines(pool, file, None).await?.standing)
}

async fn is_imported(pool: &sqlx::PgPool, file: GrantFile) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events WHERE event_type = 'PermissionGrantsChanged' \
           AND payload->'data'->>'source' = 'legacy-import' \
           AND payload->'data'->>'grant_file' = $1)",
    )
    .bind(file.file_name())
    .fetch_one(pool)
    .await
}

/// INVARIANT: Consume the legacy inputs before recovery can resume a session.
pub async fn import_legacy_files(bus: &EventBus, dir: &Path) {
    for file in GrantFile::ALL {
        let _guard = STANDING_LOCK.lock().await;
        if let Err(e) = import_locked(bus, dir, file).await {
            crate::log_legacy!(
                "[ToolGrants] Could not finish importing {}: {e}",
                file.file_name()
            );
        }
    }
    if let Err(e) = retire_legacy_reviews(bus).await {
        crate::log_legacy!("[ToolGrants] Could not retire legacy grant reviews: {e}");
    }
}

/// INVARIANT: Close old review cards before the card queue starts. Every one was an
/// engine card on the chat channel, so there is no parked caller to resume.
async fn retire_legacy_reviews(bus: &EventBus) -> Result<(), BoxError> {
    let cards: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT DISTINCT q.thread_id, q.payload->>'tool_use_id' FROM events q \
         JOIN thread_summaries t ON t.thread_id = q.thread_id \
         WHERE q.event_type IN ('UserQuestionAsked', 'UserQuestionQueued') \
           AND jsonb_typeof(q.payload->'permission_request'->'lines') = 'object' \
           AND NOT EXISTS (SELECT 1 FROM events a WHERE a.thread_id = q.thread_id \
             AND a.event_type = 'UserQuestionAnswered' \
             AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id')",
    )
    .fetch_all(bus.pool())
    .await?;
    for (thread_id, tool_use_id) in cards {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::UserQuestionAnswered {
                compose: None,
                tool_use_id,
                answer: AnswerKind::Canceled,
                quiet: true,
            },
            meta: EventMeta {
                channel: Some(EventChannel::Chat),
                ..EventMeta::NONE
            },
        })
        .await?;
    }
    Ok(())
}

/// INVARIANT: Stamp each lane at its first boot, even when no legacy file exists.
/// Once stamped, a leftover or reappearing file needs only cleanup.
async fn import_locked(bus: &EventBus, dir: &Path, file: GrantFile) -> Result<(), BoxError> {
    let path = dir.join(file.file_name());
    if !is_imported(bus.pool(), file).await? {
        let body = read_legacy_file(&path)?;
        if file == GrantFile::CodingAgentTools {
            import_coding_agent_thread_allows(bus).await?;
        }
        let lines = body
            .as_deref()
            .map(grants::parse_patterns)
            .unwrap_or_default();
        let live = standing_lines(bus.pool(), file).await?;
        let by = ChangedBy {
            source: GrantSource::LegacyImport,
            thread_id: None,
            actor: None,
        };
        match emit_standing(bus, file, with_added(&live, &lines), by).await {
            Ok(()) => {}
            Err(e)
                if crate::engine::event_bus::violates_unique_index(&*e, IMPORTED_UNIQUE_INDEX) => {}
            Err(e) => return Err(e),
        }
    }

    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn read_legacy_file(path: &Path) -> Result<Option<String>, std::io::Error> {
    match std::fs::read_to_string(path) {
        Ok(body) => Ok(Some(body)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// INVARIANT: The coding-agent lane kept its "Allow for this thread" grants as resolution
/// events and rebuilt them per thread. Each becomes a thread-held row once.
///
/// `r.thread_id = q.thread_id` is a trust boundary: only events the engine wrote
/// on a thread carry one, so a forged domain event cannot become a grant. A
/// card already granted is skipped, so a second run adds nothing.
async fn import_coding_agent_thread_allows(bus: &EventBus) -> Result<(), BoxError> {
    let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT r.thread_id, r.payload->>'request_id', q.payload->'grant_offer'->>'session' \
         FROM events r \
         JOIN events q \
           ON q.event_type = 'CodingAgentPermissionRequest' \
          AND q.thread_id = r.thread_id \
          AND q.payload->>'request_id' = r.payload->>'request_id' \
         JOIN thread_summaries t ON t.thread_id = r.thread_id \
         WHERE r.event_type = 'CodingAgentPermissionResolved' \
           AND r.thread_id IS NOT NULL \
           AND (r.payload->>'allowed')::boolean IS TRUE \
           AND r.payload->>'persist_scope' = 'session' \
           AND q.payload->'grant_offer'->>'session' IS NOT NULL \
           AND NOT EXISTS (SELECT 1 FROM events g WHERE g.event_type = 'PermissionGranted' \
             AND g.thread_id = r.thread_id \
             AND g.payload->>'request_tool_use_id' = r.payload->>'request_id')",
    )
    .fetch_all(bus.pool())
    .await?;
    for (thread_id, request_id, pattern) in rows {
        // INVARIANT: Older cards stamped patterns unscreened. One that is refused now is
        // left out, so it cannot block the import of every other grant.
        if refuse_control_characters(std::slice::from_ref(&pattern)).is_err() {
            crate::log_legacy!(
                "[ToolGrants] Left out a thread grant with a control character: {pattern:?}"
            );
            continue;
        }
        let by = AnsweredBy::tap(None);
        grant_for_thread(
            bus,
            GrantFile::CodingAgentTools,
            thread_id,
            &request_id,
            pattern,
            by,
        )
        .await?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct AnsweredBy {
    pub actor: Option<MessageOrigin>,
    pub picked_by: PickedBy,
}

impl AnsweredBy {
    pub fn tap(actor: Option<MessageOrigin>) -> Self {
        Self {
            actor,
            picked_by: PickedBy::Tap,
        }
    }
}

pub async fn grant_for_thread(
    bus: &EventBus,
    file: GrantFile,
    thread_id: Uuid,
    request_id: &str,
    pattern: String,
    by: AnsweredBy,
) -> Result<(), BoxError> {
    refuse_control_characters(std::slice::from_ref(&pattern))?;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::PermissionGranted {
            permission_id: Uuid::new_v4(),
            action: lane_action(file),
            breadth: None,
            lifetime: Lifetime::ThisThread,
            holder: Holder::Thread { thread_id },
            request_tool_use_id: request_id.to_string(),
            picked_by: by.picked_by,
            pattern: Some(pattern),
        },
        meta: EventMeta::with_actor(by.actor),
    })
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn record_card_allow(
    bus: &EventBus,
    file: GrantFile,
    thread_id: Uuid,
    request_id: &str,
    pattern: String,
    scope: AllowScope,
    by: AnsweredBy,
) -> Result<(), BoxError> {
    match scope {
        AllowScope::Session => {
            grant_for_thread(bus, file, thread_id, request_id, pattern, by).await
        }
        AllowScope::Narrow | AllowScope::Broad => {
            let changed_by = ChangedBy {
                source: GrantSource::Card,
                thread_id: Some(thread_id),
                actor: by.actor,
            };
            change_standing(bus, file, StandingChange::Add(&[pattern]), changed_by)
                .await
                .map(|_| ())
        }
    }
}

pub enum StandingChange<'a> {
    Add(&'a [String]),
    Remove(&'a [String]),
    Replace(&'a [String]),
}

pub struct ChangedBy {
    pub source: GrantSource,
    pub thread_id: Option<Uuid>,
    pub actor: Option<MessageOrigin>,
}

pub async fn change_standing(
    bus: &EventBus,
    file: GrantFile,
    change: StandingChange<'_>,
    by: ChangedBy,
) -> Result<bool, BoxError> {
    let _guard = STANDING_LOCK.lock().await;
    let live = standing_lines(bus.pool(), file).await?;
    let next = match change {
        StandingChange::Add(lines) => {
            refuse_control_characters(lines)?;
            with_added(&live, lines)
        }
        StandingChange::Remove(lines) => live
            .iter()
            .filter(|l| !lines.contains(l))
            .cloned()
            .collect(),
        StandingChange::Replace(lines) => {
            refuse_control_characters(lines)?;
            let kept: Vec<String> = live.iter().filter(|l| lines.contains(l)).cloned().collect();
            with_added(&kept, lines)
        }
    };
    if file.denies()
        && next
            .iter()
            .any(|line| !live.contains(line) && !file.recognizes(line))
    {
        return Err("Invalid standing deny. Use Bash, Bash(program:*), or Python.".into());
    }
    if as_set(&next) == as_set(&live) {
        return Ok(false);
    }
    emit_standing(bus, file, next, by).await?;
    Ok(true)
}

async fn emit_standing(
    bus: &EventBus,
    file: GrantFile,
    patterns: Vec<String>,
    by: ChangedBy,
) -> Result<(), BoxError> {
    bus.emit(BusEvent::System(SystemEvent::PermissionGrantsChanged {
        grant_file: file,
        patterns,
        source: by.source,
        granted_in_thread_id: by.thread_id,
        actor: by.actor,
    }))
    .await?;
    Ok(())
}

fn with_added(live: &[String], lines: &[String]) -> Vec<String> {
    let added: BTreeSet<&String> = lines.iter().filter(|l| !live.contains(l)).collect();
    live.iter()
        .cloned()
        .chain(added.into_iter().cloned())
        .collect()
}

fn as_set(lines: &[String]) -> BTreeSet<&str> {
    lines.iter().map(String::as_str).collect()
}

fn refuse_control_characters(lines: &[String]) -> Result<(), BoxError> {
    match lines.iter().find(|l| l.chars().any(char::is_control)) {
        Some(line) => {
            Err(format!("refusing a grant pattern with a control character: {line:?}").into())
        }
        None => Ok(()),
    }
}

#[cfg(test)]
#[path = "tool_grants_tests.rs"]
mod tests;
