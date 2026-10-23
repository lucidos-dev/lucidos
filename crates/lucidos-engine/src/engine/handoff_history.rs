//! The history the *handoff estimate* reads: how many main-session calls one
//! unit of work took at past runs of a checkpoint, and how large a fresh
//! successor's context gets (ADR 0505).
//!
//! A sample runs from a `HandoffCheckpointReached` to the first *clean turn
//! end* after it. When the thread hands off first, it follows into the
//! successor, up to the successor's own clean turn end.

use sqlx::PgPool;
use uuid::Uuid;

use super::agent_recovery::park_ending_event_types_sql;
use super::successor::{MAIN_SESSION_CAPTURE, MAX_HANDOFF_HOPS};
use super::thread_events::ThreadEvent;

/// Fewer samples than this, and a median widens or falls back.
const MIN_SAMPLES: usize = 3;

/// How many of the most recent samples a median reads.
const RECENT_SAMPLES: usize = 20;

/// How many recent checkpoint runs a median looks through for its samples.
/// Some are still open, and only some fall in the size band.
const CANDIDATE_RUNS: i64 = 60;

/// Which call of a successor's session measures its fresh context. By then
/// it has read the narrative, the plan and the files that matter.
const FRESH_CALL: i64 = 10;

/// A SQL predicate: the `CodingAgentIdled` row aliased `idle` is a *clean
/// turn end*. Its turn finished on a `ResponseGenerated`, and at the idle no
/// question is open, no event wait is live and no message is unread. A
/// question stays open until a park-ending event, as the restart preserve
/// guard reads it. Unread follows the composer's `agentHoldsUnreadMessage`:
/// newer than the newest input read, and a thread with no reads holds none.
pub(crate) fn clean_turn_end_sql(idle: &str) -> String {
    let before_idle = |alias: &str| {
        format!("{alias}.thread_id = {idle}.thread_id AND {alias}.sequence < {idle}.sequence")
    };
    let terminators = ThreadEvent::TERMINATOR_EVENT_TYPES
        .iter()
        .map(|t| format!("'{t}'"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{idle}.event_type = 'CodingAgentIdled' AND {idle}.payload->>'reason' IS NULL \
         AND (SELECT t.event_type FROM events t WHERE {t_before} \
                AND t.event_type IN ({terminators}) \
              ORDER BY t.sequence DESC LIMIT 1) = 'ResponseGenerated' \
         AND NOT EXISTS (SELECT 1 FROM events q WHERE {q_before} \
               AND q.event_type = 'UserQuestionAsked' \
               AND NOT EXISTS (SELECT 1 FROM events p WHERE {p_before} \
                     AND p.sequence > q.sequence AND p.event_type IN ({park_ending}))) \
         AND NOT EXISTS (SELECT 1 FROM events w WHERE {w_before} \
               AND w.event_type = 'EventWaitStarted' \
               AND NOT EXISTS (SELECT 1 FROM events r WHERE {r_before} \
                     AND r.event_type IN ('EventWaitDelivered', 'EventWaitExpired', \
                                          'EventWaitCanceled') \
                     AND r.payload->>'wait_id' = w.payload->>'wait_id')) \
         AND NOT EXISTS (SELECT 1 FROM events m WHERE {m_before} \
               AND m.event_type = 'MessageReceived' \
               AND NOT EXISTS (SELECT 1 FROM events x WHERE {x_before} \
                     AND x.event_type = 'QueuedMessageRemoved' \
                     AND x.payload->>'removed_message_id' = m.id::text) \
               AND m.sequence > (SELECT max(src.sequence) FROM events h \
                     JOIN events src ON src.id = (h.payload->>'input_event_id')::uuid \
                    WHERE {h_before} AND h.event_type = 'CodingAgentInputRead'))",
        t_before = before_idle("t"),
        q_before = before_idle("q"),
        p_before = before_idle("p"),
        park_ending = park_ending_event_types_sql(),
        w_before = before_idle("w"),
        r_before = before_idle("r"),
        m_before = before_idle("m"),
        x_before = before_idle("x"),
        h_before = before_idle("h"),
    )
}

/// The repository a `thread_summaries` row aliased `alias` works in: its
/// registered repository id, else its folder. An app thread has only a folder.
pub(crate) fn thread_repo_sql(alias: &str) -> String {
    format!("COALESCE({alias}.cc_repo_id, {alias}.coding_agent_folder)")
}

/// What a median pools on: the repository, the checkpoint's name, the model
/// and the effort.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryKey {
    /// The repository, as [`thread_repo_sql`] reads it.
    pub repo: Option<String>,
    pub name: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
}

/// One finished sample: the context it entered at, and its calls per unit.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Sample {
    context_tokens: u64,
    calls_per_unit: f64,
}

/// The median calls per unit for `key` near `context_tokens`. It reads the
/// samples that entered at half to double this context, then all sizes when
/// those are too few. `None` when even all sizes are too few.
pub(crate) async fn calls_per_unit(
    pool: &PgPool,
    key: &HistoryKey,
    context_tokens: u64,
) -> Result<Option<f64>, sqlx::Error> {
    let samples = samples(pool, key).await?;
    let in_band = |s: &&Sample| {
        s.context_tokens.saturating_mul(2) >= context_tokens
            && s.context_tokens <= context_tokens.saturating_mul(2)
    };
    let band: Vec<f64> = samples
        .iter()
        .filter(in_band)
        .map(|s| s.calls_per_unit)
        .collect();
    let all: Vec<f64> = samples.iter().map(|s| s.calls_per_unit).collect();
    Ok(median_of_recent(&band).or_else(|| median_of_recent(&all)))
}

/// The median of the first [`RECENT_SAMPLES`], newest first, or `None` under
/// [`MIN_SAMPLES`].
fn median_of_recent(values: &[f64]) -> Option<f64> {
    let mut recent: Vec<f64> = values.iter().take(RECENT_SAMPLES).copied().collect();
    if recent.len() < MIN_SAMPLES {
        return None;
    }
    recent.sort_by(f64::total_cmp);
    let mid = recent.len() / 2;
    Some(if recent.len().is_multiple_of(2) {
        (recent[mid - 1] + recent[mid]) / 2.0
    } else {
        recent[mid]
    })
}

/// One recorded checkpoint run a median may sample.
#[derive(sqlx::FromRow)]
struct CheckpointRow {
    thread_id: Uuid,
    sequence: i64,
    context_tokens: Option<i64>,
    done: Option<i32>,
    total: Option<i32>,
}

/// The finished samples for `key`, newest first. A run still open, or one
/// that left no units, gives none.
async fn samples(pool: &PgPool, key: &HistoryKey) -> Result<Vec<Sample>, sqlx::Error> {
    let sql = format!(
        "SELECT e.thread_id, e.sequence, \
                (e.payload->>'context_tokens')::bigint AS context_tokens, \
                (e.payload->'progress'->>'done')::int AS done, \
                (e.payload->'progress'->>'total')::int AS total \
           FROM events e JOIN thread_summaries t ON t.thread_id = e.thread_id \
          WHERE e.event_type = 'HandoffCheckpointReached' \
            AND {repo} IS NOT DISTINCT FROM $1 \
            AND e.payload->>'name' = $2 \
            AND split_part(e.payload->>'model', '[', 1) = split_part($3, '[', 1) \
            AND e.payload->>'reasoning_effort' IS NOT DISTINCT FROM $4 \
          ORDER BY e.sequence DESC LIMIT $5",
        repo = thread_repo_sql("t"),
    );
    let runs: Vec<CheckpointRow> = sqlx::query_as(&sql)
        .bind(&key.repo)
        .bind(&key.name)
        .bind(&key.model)
        .bind(&key.reasoning_effort)
        .bind(CANDIDATE_RUNS)
        .fetch_all(pool)
        .await?;
    let mut found = Vec::new();
    for run in runs {
        let units = match (run.done, run.total) {
            (Some(done), Some(total)) => total - done,
            _ => 1,
        };
        let tokens = run.context_tokens.and_then(|t| u64::try_from(t).ok());
        let (Some(tokens), true) = (tokens, units > 0) else {
            continue;
        };
        if let Some(calls) = sample_calls(pool, run.thread_id, run.sequence).await? {
            found.push(Sample {
                context_tokens: tokens,
                calls_per_unit: calls as f64 / f64::from(units),
            });
        }
    }
    Ok(found)
}

/// The main-session calls from `after` on `thread_id` to the first clean
/// turn end, following each handoff made before it. A leg that hands off
/// counts up to the spawn, never the old thread's later calls. `None` while
/// the work has not reached one.
pub(crate) async fn sample_calls(
    pool: &PgPool,
    thread_id: Uuid,
    after: i64,
) -> Result<Option<i64>, sqlx::Error> {
    let sql = format!(
        "WITH turn_end AS ( \
           SELECT min(idle.sequence) AS seq FROM events idle \
            WHERE idle.thread_id = $1 AND idle.sequence > $2 AND {clean} \
         ), \
         handoff AS ( \
           SELECT sequence, (payload->>'successor_thread_id')::uuid AS successor FROM events \
            WHERE thread_id = $1 AND event_type = 'SuccessorThreadSpawned' AND sequence > $2 \
              AND sequence < COALESCE((SELECT seq FROM turn_end), 9223372036854775807) \
            ORDER BY sequence LIMIT 1 \
         ) \
         SELECT (SELECT seq FROM turn_end), (SELECT successor FROM handoff), \
                (SELECT count(*) FROM events WHERE thread_id = $1 AND sequence > $2 \
                   AND sequence <= COALESCE((SELECT sequence FROM handoff), \
                                            (SELECT seq FROM turn_end), 9223372036854775807) \
                   AND {MAIN_SESSION_CAPTURE})",
        clean = clean_turn_end_sql("idle"),
    );
    let (mut thread, mut after, mut calls) = (thread_id, after, 0_i64);
    for _ in 0..MAX_HANDOFF_HOPS {
        let (end, successor, leg): (Option<i64>, Option<Uuid>, i64) = sqlx::query_as(&sql)
            .bind(thread)
            .bind(after)
            .fetch_one(pool)
            .await?;
        calls += leg;
        match (successor, end) {
            (Some(successor), _) => (thread, after) = (successor, 0),
            (None, Some(_)) => return Ok(Some(calls)),
            (None, None) => return Ok(None),
        }
    }
    Ok(None)
}

/// The median context of recent successors at their tenth main-session call.
pub(crate) async fn fresh_tokens_median(pool: &PgPool) -> Result<Option<f64>, sqlx::Error> {
    let sql = format!(
        "SELECT (SELECT (payload->'usage'->>'input_tokens')::bigint FROM events \
                  WHERE thread_id = t.thread_id AND {MAIN_SESSION_CAPTURE} \
                  ORDER BY sequence OFFSET {} LIMIT 1) \
           FROM thread_summaries t WHERE t.successor_of IS NOT NULL \
          ORDER BY t.created_at DESC LIMIT {RECENT_SAMPLES}",
        FRESH_CALL - 1
    );
    let tokens: Vec<Option<i64>> = sqlx::query_scalar(&sql).fetch_all(pool).await?;
    let tokens: Vec<f64> = tokens.into_iter().flatten().map(|t| t as f64).collect();
    Ok(median_of_recent(&tokens))
}

#[cfg(test)]
#[path = "handoff_history_tests.rs"]
mod tests;
