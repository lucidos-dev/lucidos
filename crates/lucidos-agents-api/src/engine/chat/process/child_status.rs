//! The *child-status block*: each direct child thread's liveness, read at the
//! start of the turn and carried in the turn tail (ADR 0679).
//!
//! A parent learns about a child from its reports (`CHILD_REPORT_EVENT_TYPES`). When
//! the user carries on in the child directly, no new report arrives, and the
//! parent's newest fact goes stale without looking stale. This block puts the
//! child's state now beside the age of its newest report, so a stale claim
//! contradicts something the agent can see.
//!
//! The guidance is [`CHILD_STATUS_POINTER`], a `const` for the cached system
//! block. The reading varies per thread and per turn, so it rides in the tail
//! like the rest of [`super::turn_tail`].

use sqlx::PgPool;
use uuid::Uuid;

use crate::core::store::messages::CHILD_REPORT_EVENT_TYPES;
use crate::engine::agent_context::format_age;
use crate::engine::thread_events::WakeSource;
use crate::engine::thread_liveness::{liveness_for_threads, LivenessHold, ThreadLiveness};
use crate::engine::widget_guidance::truncate_on_word_boundary;

/// The most children the block lists. The rest collapse into one count line.
pub(super) const CHILD_STATUS_ROWS: usize = 8;

/// The turn tail carries previews. A thread read gives the full evidence.
const WAIT_DETAIL_CHARS: usize = 120;

pub(super) const CHILD_STATUS_POINTER: &str = "\n\nCHILD THREADS:\n\
     The [CHILD THREADS] block at the END of the request gives each child \
     thread's state NOW and its last report to you. It beats any earlier report \
     or message of yours.";

/// One child, as the block renders it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ChildStatus {
    pub title: Option<String>,
    pub liveness: ThreadLiveness,
    /// Age of the child's newest report on this thread, woken or filed.
    pub report_age_secs: Option<i64>,
}

/// The listed children, newest activity first, and how many more there are.
#[derive(Debug, Default, PartialEq)]
pub(super) struct ChildStatuses {
    pub listed: Vec<ChildStatus>,
    pub unlisted: usize,
}

#[derive(sqlx::FromRow)]
struct ChildRow {
    thread_id: Uuid,
    title: Option<String>,
    report_age_secs: Option<i64>,
    total: i64,
}

/// The block for `thread_id`, empty when it has no unarchived children.
pub(super) async fn child_status_block(pool: &PgPool, thread_id: Uuid) -> String {
    let read = read_child_statuses(pool, thread_id).await;
    if let Err(e) = &read {
        crate::log!(
            "[Chat] child-status read failed for thread {}: {}",
            thread_id,
            lucidos_diagnostic_log::private(e)
        );
    }
    render_child_status(read.as_ref().map_err(|_| ()))
}

async fn read_child_statuses(pool: &PgPool, thread_id: Uuid) -> Result<ChildStatuses, sqlx::Error> {
    let rows: Vec<ChildRow> = sqlx::query_as(
        "SELECT ts.thread_id, ts.title, \
                EXTRACT(EPOCH FROM now() - report.created)::bigint AS report_age_secs, \
                COUNT(*) OVER () AS total \
           FROM thread_summaries ts \
           LEFT JOIN LATERAL ( \
                SELECT e.created FROM events e \
                 WHERE e.thread_id = $1 \
                   AND e.event_type = ANY($3) \
                   AND e.payload->>'child_thread_id' = ts.thread_id::text \
                 ORDER BY e.created DESC LIMIT 1 \
           ) report ON true \
          WHERE ts.parent_thread_id = $1 AND ts.archive_state <> 'archived' \
          ORDER BY ts.last_activity DESC \
          LIMIT $2",
    )
    .bind(thread_id)
    .bind(CHILD_STATUS_ROWS as i64)
    .bind(CHILD_REPORT_EVENT_TYPES)
    .fetch_all(pool)
    .await?;
    let total = rows.first().map_or(0, |row| row.total as usize);
    let ids: Vec<Uuid> = rows.iter().map(|row| row.thread_id).collect();
    let mut liveness = liveness_for_threads(pool, &ids).await?;
    let listed: Vec<ChildStatus> = rows
        .into_iter()
        .filter_map(|row| {
            Some(ChildStatus {
                title: row.title,
                liveness: liveness.remove(&row.thread_id)?,
                report_age_secs: row.report_age_secs,
            })
        })
        .collect();
    Ok(ChildStatuses {
        unlisted: total.saturating_sub(listed.len()),
        listed,
    })
}

/// The block text. A failed read says so, rather than vanishing, so the agent
/// knows it has no reading and must look before claiming a state.
pub(super) fn render_child_status(read: Result<&ChildStatuses, ()>) -> String {
    let body = match read {
        Err(()) => "Unreadable this turn. Read a child's liveness before stating it.".to_string(),
        Ok(statuses) if statuses.listed.is_empty() && statuses.unlisted == 0 => {
            return String::new();
        }
        Ok(statuses) => {
            let mut lines: Vec<String> = statuses.listed.iter().map(child_line).collect();
            if statuses.unlisted > 0 {
                lines.push(format!(
                    "…and {} more (the `threads` tool lists them).",
                    statuses.unlisted
                ));
            }
            lines.join("\n")
        }
    };
    format!("[CHILD THREADS]\n{body}\n[END CHILD THREADS]")
}

fn child_line(child: &ChildStatus) -> String {
    let title = child.title.as_deref().unwrap_or("Untitled thread");
    let live = &child.liveness;
    let state = live.state.as_str().replace('_', " ");
    let active = live.last_activity_age_secs.map_or_else(
        || "no activity yet".to_string(),
        |s| format!("active {}", format_age(s)),
    );
    let report = child.report_age_secs.map_or_else(
        || "has not reported to you".to_string(),
        |s| format!("last reported to you {}", format_age(s)),
    );
    let mut parts = vec![format!("- {title:?}: {state} now, {active}"), report];
    parts.extend(live.live_event_waits.iter().map(|wait| {
        let reason = truncate_on_word_boundary(&wait.reason, WAIT_DETAIL_CHARS);
        format!("waiting for {reason:?} ({} s left)", wait.expires_in_secs)
    }));
    parts.extend(live.awaited_children.iter().map(|awaited| {
        let title = truncate_on_word_boundary(&awaited.title, WAIT_DETAIL_CHARS);
        format!("awaiting child {title:?}")
    }));
    if let Some(hold) = &live.hold {
        parts.push(match hold {
            LivenessHold::Stop => "held by Stop".into(),
            LivenessHold::AutomationPause { reason } => {
                let reason = truncate_on_word_boundary(reason, WAIT_DETAIL_CHARS);
                format!("held by automation pause ({reason:?})")
            }
        });
    }
    if let Some(wake) = &live.expected_wake {
        let source = match wake.source {
            WakeSource::Agent => "agent",
            WakeSource::Decision => "decision",
        };
        let reason = truncate_on_word_boundary(&wake.reason, WAIT_DETAIL_CHARS);
        parts.push(format!(
            "expected wake {} ({source}: {reason:?})",
            wake.at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
        ));
    }
    parts.join("; ")
}

#[cfg(test)]
#[path = "child_status_tests.rs"]
mod tests;
