use sqlx::PgPool;
use uuid::Uuid;

use crate::core::store::messages::CHILD_REPORT_EVENT_TYPES;
use crate::engine::agent_context::format_age;
use crate::engine::thread_events::WakeSource;
use crate::engine::thread_liveness::{liveness_for_threads, LivenessHold, ThreadLiveness};
use crate::engine::widget_guidance::truncate_on_word_boundary;

pub(super) const CHILD_STATUS_ROWS: usize = 8;

const WAIT_DETAIL_CHARS: usize = 120;

pub(super) const CHILD_STATUS_POINTER: &str = "\n\nCHILD THREADS:\n\
     The [CHILD THREADS] block at the END of the request gives each child \
     thread's state NOW and its last report to you. It beats any earlier report \
     or message of yours.";

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ChildStatus {
    pub title: Option<String>,
    pub liveness: ThreadLiveness,
    pub report_age_secs: Option<i64>,
}

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
            LivenessHold::OpenCard { .. } => "held by a card waiting for the owner".into(),
            LivenessHold::Hold { reason, .. } => {
                let reason = truncate_on_word_boundary(reason, WAIT_DETAIL_CHARS);
                format!("on hold ({reason:?})")
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
