//! Read [`TriageFacts`] for inbox root threads in one query.
//!
//! A root is an inbox thread whose parent is absent or not in the inbox, the
//! row the drawer shows at top level. Its inbox sub-threads roll up into it,
//! because an archive of the root takes them too.

use sqlx::PgPool;
use uuid::Uuid;

use super::{NeedFact, TriageFacts, TriggerRun};
use crate::core::store::HAS_DRAFT_SQL;
use crate::engine::thread_lifecycle::ThreadStatus;

/// One root as triage reports it: the facts, plus what a reader needs to find
/// it. The title rides beside the facts, never inside them.
#[derive(Debug, Clone)]
pub(crate) struct TriageRow {
    pub(crate) facts: TriageFacts,
    pub(crate) title: String,
    /// `inbox`, or `archived` for an id that left the inbox since it was read.
    pub(crate) section: String,
}

/// Which roots to read.
pub(crate) enum TriageScope<'a> {
    /// Every inbox root except the caller's own thread.
    Inbox { except: Option<Uuid> },
    /// Exactly these ids, wherever they sit now, for an apply-time re-check.
    Ids(&'a [Uuid]),
}

/// The row filter for a root, over `thread_summaries r`.
///
/// The home thread is never one: it is never archived, and no drawer section
/// lists it (ADR 0362). Its sub-threads count as roots instead, as the drawer
/// shows them.
const INBOX_ROOT_SQL: &str = "r.archive_state = 'inbox' AND r.state <> 'discarded' \
     AND NOT r.is_home \
     AND (r.parent_thread_id IS NULL OR NOT EXISTS ( \
         SELECT 1 FROM thread_summaries p \
         WHERE p.thread_id = r.parent_thread_id AND p.archive_state = 'inbox' \
           AND NOT p.is_home))";

/// The row filter for a root, over `thread_summaries t`. The drawer shows a
/// thread still being composed only when it holds a draft.
fn shown_root_sql() -> String {
    format!("(t.state <> 'composing' OR {HAS_DRAFT_SQL})")
}

/// A question card the thread asked and nobody answered, live or orphaned.
const PENDING_QUESTION_SQL: &str = "(t.status = 'waiting_for_user_answer' OR EXISTS ( \
     SELECT 1 FROM events q WHERE q.thread_id = t.thread_id \
       AND q.event_type = 'UserQuestionAsked' \
       AND NOT EXISTS (SELECT 1 FROM events a WHERE a.thread_id = q.thread_id \
         AND a.event_type = 'UserQuestionAnswered' \
         AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id')))";

/// One member's flags, read only while it is open: an archived member holds
/// nothing the user must act on, and the scans behind these are the costly part.
fn member_flags_sql() -> String {
    format!(
        "CASE WHEN t.archive_state = 'inbox' THEN {PENDING_QUESTION_SQL} ELSE FALSE END AS question, \
         CASE WHEN t.archive_state = 'inbox' THEN (t.coding_agent_proposed OR EXISTS ( \
           SELECT 1 FROM changes ch WHERE ch.thread_id = t.thread_id \
             AND ch.status IN ('pending', 'set_aside'))) ELSE FALSE END AS pending_change, \
         t.archive_state = 'inbox' AND t.coding_agent_has_diff \
           AND NOT t.coding_agent_proposed AS unproposed, \
         t.archive_state = 'inbox' AND {HAS_DRAFT_SQL} AS draft, \
         t.archive_state = 'inbox' AND t.status = 'failed' AS failed"
    )
}

fn query_sql(root_filter: &str, shown_filter: &str) -> String {
    let member_flags = member_flags_sql();
    format!(
        "WITH RECURSIVE fam AS ( \
           SELECT r.thread_id AS root, r.thread_id AS member, 0 AS depth \
           FROM thread_summaries r WHERE {root_filter} \
           UNION ALL \
           SELECT fam.root, c.thread_id, fam.depth + 1 \
           FROM fam JOIN thread_summaries c ON c.parent_thread_id = fam.member \
           WHERE c.state <> 'discarded' AND fam.depth < 64 \
         ), m AS ( \
           SELECT fam.root, fam.depth, \
             t.archive_state = 'inbox' AND (t.status IN ('running', 'paused') \
               OR t.live_event_wait_count > 0) AS busy, \
             {member_flags}, \
             EXISTS (SELECT 1 FROM changes ch WHERE ch.thread_id = t.thread_id) AS ever_proposed \
           FROM fam JOIN thread_summaries t ON t.thread_id = fam.member \
         ), sub AS ( \
           SELECT root, \
             bool_or(question) FILTER (WHERE depth = 0) AS question, \
             bool_or(pending_change) FILTER (WHERE depth = 0) AS pending_change, \
             bool_or(unproposed) FILTER (WHERE depth = 0) AS unproposed, \
             bool_or(draft) FILTER (WHERE depth = 0) AS draft, \
             COUNT(*) FILTER (WHERE depth > 0) AS sub_count, \
             bool_or(busy) FILTER (WHERE depth > 0) AS sub_busy, \
             bool_or(question) FILTER (WHERE depth > 0) AS sub_question, \
             bool_or(pending_change) FILTER (WHERE depth > 0) AS sub_change, \
             bool_or(unproposed) FILTER (WHERE depth > 0) AS sub_unproposed, \
             bool_or(draft) FILTER (WHERE depth > 0) AS sub_draft, \
             bool_or(failed) FILTER (WHERE depth > 0) AS sub_failed, \
             bool_or(ever_proposed) AS ever_proposed \
           FROM m GROUP BY root \
         ) \
         SELECT t.thread_id, t.title, t.archive_state, t.status, t.is_saved, \
           sub.question, sub.pending_change, sub.unproposed, sub.draft, \
           t.has_response, \
           COALESCE(sub.ever_proposed, FALSE) AS ever_proposed, \
           COALESCE(sub.sub_count, 0) AS sub_count, \
           t.live_event_wait_count::bigint AS live_waits, \
           COALESCE(sub.sub_busy, FALSE) AS sub_busy, \
           COALESCE(sub.sub_question, FALSE) AS sub_question, \
           COALESCE(sub.sub_change, FALSE) AS sub_change, \
           COALESCE(sub.sub_unproposed, FALSE) AS sub_unproposed, \
           COALESCE(sub.sub_draft, FALSE) AS sub_draft, \
           COALESCE(sub.sub_failed, FALSE) AS sub_failed, \
           EXTRACT(EPOCH FROM now() - t.last_activity)::bigint AS idle_secs, \
           CASE WHEN t.source = 'trigger' AND t.trigger_id IS NOT NULL \
                     AND t.parent_thread_id IS NULL THEN t.trigger_id END AS run_of, \
           COALESCE(t.trigger_name, '') AS trigger_name, \
           NOT EXISTS (SELECT 1 FROM thread_summaries o \
             WHERE o.trigger_id = t.trigger_id AND o.source = 'trigger' \
               AND o.parent_thread_id IS NULL AND o.state <> 'discarded' \
               AND o.created_at > t.created_at) AS newest_run \
         FROM thread_summaries t JOIN sub ON sub.root = t.thread_id \
         WHERE {shown_filter} \
         ORDER BY t.last_activity DESC"
    )
}

/// Read the roots `scope` names, newest activity first.
pub(crate) async fn load(
    pool: &PgPool,
    scope: TriageScope<'_>,
) -> Result<Vec<TriageRow>, sqlx::Error> {
    let rows: Vec<sqlx::postgres::PgRow> = match scope {
        TriageScope::Inbox { except } => {
            let filter = format!("{INBOX_ROOT_SQL} AND r.thread_id IS DISTINCT FROM $1");
            sqlx::query(&query_sql(&filter, &shown_root_sql()))
                .bind(except)
                .fetch_all(pool)
                .await?
        }
        TriageScope::Ids(ids) => {
            sqlx::query(&query_sql("r.thread_id = ANY($1)", "TRUE"))
                .bind(ids)
                .fetch_all(pool)
                .await?
        }
    };
    rows.iter().map(row_from).collect()
}

fn row_from(row: &sqlx::postgres::PgRow) -> Result<TriageRow, sqlx::Error> {
    use sqlx::Row as _;
    let flag = |name: &str| row.try_get::<bool, _>(name);
    let sub_needs = [
        (flag("sub_question")?, NeedFact::Question),
        (flag("sub_change")?, NeedFact::PendingChange),
        (flag("sub_unproposed")?, NeedFact::UnproposedWork),
        (flag("sub_draft")?, NeedFact::Draft),
        (flag("sub_failed")?, NeedFact::FailedRun),
    ]
    .into_iter()
    .filter_map(|(holds, fact)| holds.then_some(fact))
    .collect();
    let status: String = row.try_get("status")?;
    let trigger = match row.try_get::<Option<String>, _>("run_of")? {
        Some(trigger_id) => Some(TriggerRun {
            trigger_id,
            trigger_name: row.try_get("trigger_name")?,
            is_newest: flag("newest_run")?,
        }),
        None => None,
    };
    Ok(TriageRow {
        facts: TriageFacts {
            thread_id: row.try_get("thread_id")?,
            status: ThreadStatus::parse(&status),
            is_pinned: flag("is_saved")?,
            has_pending_question: flag("question")?,
            has_pending_change: flag("pending_change")?,
            has_unproposed_work: flag("unproposed")?,
            has_draft: flag("draft")?,
            has_output: flag("has_response")?,
            ever_proposed_change: flag("ever_proposed")?,
            sub_thread_count: row.try_get("sub_count")?,
            live_event_waits: row.try_get("live_waits")?,
            sub_thread_needs: sub_needs,
            sub_thread_busy: flag("sub_busy")?,
            idle_secs: row.try_get("idle_secs")?,
            trigger,
        },
        title: row
            .try_get::<Option<String>, _>("title")?
            .unwrap_or_default(),
        section: row.try_get("archive_state")?,
    })
}

#[cfg(test)]
#[path = "facts_tests.rs"]
mod tests;
