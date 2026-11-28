use std::collections::HashMap;

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::log::{self, StoredEvent, ThreadKind, ThreadLog};
use super::{NodeAddr, SummaryScope, HORIZON_SECS};
use crate::engine::thread_events::{EventMeta, ThreadEvent};

type StoreResult<T> = Result<T, sqlx::Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredNode {
    pub text: String,
    pub model: Option<String>,
    pub source_event_id: Option<Uuid>,
    pub source_thread_id: Option<Uuid>,
}

pub(super) const THREAD_IN_SCOPE_SQL: &str = "(s.source <> 'trigger' OR EXISTS ( \
       SELECT 1 FROM events m WHERE m.thread_id = s.thread_id \
       AND m.event_type = 'MessageReceived' AND m.payload->>'mode' = 'human'))";

#[derive(sqlx::FromRow)]
struct NodeRow {
    start: i64,
    span: i64,
    text: String,
    model: Option<String>,
    source_event_id: Option<Uuid>,
    source_thread_id: Option<Uuid>,
}

pub async fn load_nodes(
    pool: &PgPool,
    scope: SummaryScope,
) -> StoreResult<HashMap<NodeAddr, StoredNode>> {
    let rows: Vec<NodeRow> = sqlx::query_as(
        "SELECT start, span, text, model, source_event_id, source_thread_id \
         FROM summary_tree_nodes WHERE scope = $1",
    )
    .bind(scope.as_db())
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(NodeRow::into_entry).collect())
}

impl NodeRow {
    fn into_entry(self) -> (NodeAddr, StoredNode) {
        (
            NodeAddr {
                start: self.start as u64,
                span: self.span as u64,
            },
            StoredNode {
                text: self.text,
                model: self.model,
                source_event_id: self.source_event_id,
                source_thread_id: self.source_thread_id,
            },
        )
    }
}

/// INVARIANT: Store a built node. A node already at that address wins: two drains of one
/// scope never overlap, so a conflict is a replay of the same build.
pub async fn insert_node(
    pool: &PgPool,
    scope: SummaryScope,
    addr: NodeAddr,
    node: &StoredNode,
) -> StoreResult<()> {
    insert_with(pool, scope, addr, node).await
}

async fn insert_with<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    scope: SummaryScope,
    addr: NodeAddr,
    node: &StoredNode,
) -> StoreResult<()> {
    sqlx::query(
        "INSERT INTO summary_tree_nodes \
           (scope, start, span, text, model, source_event_id, source_thread_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (scope, span, start) DO NOTHING",
    )
    .bind(scope.as_db())
    .bind(addr.start as i64)
    .bind(addr.span as i64)
    .bind(&node.text)
    .bind(&node.model)
    .bind(node.source_event_id)
    .bind(node.source_thread_id)
    .execute(executor)
    .await?;
    Ok(())
}

pub(crate) async fn realign(
    pool: &PgPool,
    scope: SummaryScope,
    from: Option<u64>,
    moved: &[(NodeAddr, StoredNode)],
    orphans: &[NodeAddr],
) -> StoreResult<()> {
    let mut tx = pool.begin().await?;
    if let Some(from) = from {
        sqlx::query(
            "DELETE FROM summary_tree_nodes WHERE scope = $1 \
             AND ((span = 1 AND start >= $2) OR (span > 1 AND start + span > $2))",
        )
        .bind(scope.as_db())
        .bind(from as i64)
        .execute(&mut *tx)
        .await?;
    }
    for (addr, node) in moved {
        insert_with(&mut *tx, scope, *addr, node).await?;
    }
    for addr in orphans {
        sqlx::query("DELETE FROM summary_tree_nodes WHERE scope = $1 AND span = $2 AND start = $3")
            .bind(scope.as_db())
            .bind(addr.span as i64)
            .bind(addr.start as i64)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

pub(crate) async fn rewrite_verbatim(
    pool: &PgPool,
    scope: SummaryScope,
    nodes: &[(NodeAddr, String)],
) -> StoreResult<()> {
    if nodes.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for (addr, text) in nodes {
        sqlx::query(
            "UPDATE summary_tree_nodes SET text = $4 \
             WHERE scope = $1 AND span = $2 AND start = $3 AND model IS NULL",
        )
        .bind(scope.as_db())
        .bind(addr.span as i64)
        .bind(addr.start as i64)
        .bind(text)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

pub(crate) async fn reflect(pool: &PgPool, scope: SummaryScope, through: i64) -> StoreResult<()> {
    sqlx::query(
        "INSERT INTO summary_tree_scopes (scope, reflected_through) VALUES ($1, $2) \
         ON CONFLICT (scope) DO UPDATE SET reflected_through = EXCLUDED.reflected_through",
    )
    .bind(scope.as_db())
    .bind(through)
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn mark_ready(pool: &PgPool) -> StoreResult<()> {
    sqlx::query(
        "INSERT INTO summary_tree_scopes (scope, backfilled_at) VALUES ($1, now()) \
         ON CONFLICT (scope) DO UPDATE \
         SET backfilled_at = COALESCE(summary_tree_scopes.backfilled_at, now())",
    )
    .bind(SummaryScope::Workspace.as_db())
    .execute(pool)
    .await?;
    Ok(())
}

/// INVARIANT: Clear the ready flag, and say whether it was set. A workspace leaving the
/// Tree module stops feeding its trees, so coming back must catch up before a
/// turn reads them.
pub(crate) async fn clear_ready(pool: &PgPool) -> StoreResult<bool> {
    let cleared = sqlx::query(
        "UPDATE summary_tree_scopes SET backfilled_at = NULL \
         WHERE scope = $1 AND backfilled_at IS NOT NULL",
    )
    .bind(SummaryScope::Workspace.as_db())
    .execute(pool)
    .await?
    .rows_affected();
    Ok(cleared > 0)
}

pub(crate) async fn is_ready(pool: &PgPool) -> StoreResult<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM summary_tree_scopes \
         WHERE scope = $1 AND backfilled_at IS NOT NULL)",
    )
    .bind(SummaryScope::Workspace.as_db())
    .fetch_one(pool)
    .await
}

pub(crate) async fn has_nodes(pool: &PgPool) -> StoreResult<bool> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM summary_tree_nodes)")
        .fetch_one(pool)
        .await
}

/// INVARIANT: Whether every tree reflects every log event written so far: the workspace
/// tree its leaf events, and each in-scope thread tree its entries. A turn
/// leaf never waits on its thread, so both halves are asked. The
/// context-handling benchmark waits on this between tasks, so a Tree arm is
/// scored on the trees it was designed to read.
pub async fn caught_up(pool: &PgPool) -> StoreResult<bool> {
    let thread_log_types: Vec<&str> = log::ENTRY_EVENT_TYPES
        .iter()
        .chain(log::TURN_END_EVENT_TYPES)
        .chain(crate::engine::consumption_claim::HELD_MESSAGE_RECEIPT_TYPES)
        .copied()
        .collect();
    sqlx::query_scalar(&format!(
        "SELECT COALESCE((SELECT reflected_through FROM summary_tree_scopes \
                          WHERE scope = $1), 0) \
             >= COALESCE((SELECT max(sequence) FROM events \
                          WHERE event_type IN ({})), 0) \
           AND NOT EXISTS ( \
             SELECT 1 FROM thread_summaries s \
             LEFT JOIN summary_tree_scopes t ON t.scope = s.thread_id::text \
             WHERE {THREAD_IN_SCOPE_SQL} AND EXISTS ( \
               SELECT 1 FROM events e WHERE e.thread_id = s.thread_id \
                 AND e.event_type IN ({}) \
                 AND e.sequence > COALESCE(t.reflected_through, 0)))",
        super::workspace_log::sql_list(super::workspace_log::WORKSPACE_LEAF_EVENT_TYPES),
        super::workspace_log::sql_list(&thread_log_types),
    ))
    .bind(SummaryScope::Workspace.as_db())
    .fetch_one(pool)
    .await
}

pub(super) fn ready_window_sql(param: usize) -> String {
    format!("s.last_activity > now() - make_interval(days => ${param})")
}

#[derive(sqlx::FromRow)]
pub(crate) struct DirtyThread {
    pub(crate) thread_id: Uuid,
    pub(crate) ready_window: bool,
    pub(crate) unreflected: bool,
}

/// INVARIANT: In-scope threads with events the tree does not yet reflect, the most
/// recently active first. A thread never drained has no progress row, so it
/// is always here.
pub(crate) async fn dirty_threads(pool: &PgPool, ready_days: i32) -> StoreResult<Vec<DirtyThread>> {
    sqlx::query_as(&format!(
        "SELECT s.thread_id, \
                {} AS ready_window, \
                t.scope IS NULL AS unreflected \
         FROM thread_summaries s \
         LEFT JOIN summary_tree_scopes t ON t.scope = s.thread_id::text \
         WHERE {THREAD_IN_SCOPE_SQL} \
           AND (t.scope IS NULL OR EXISTS ( \
             SELECT 1 FROM events e \
             WHERE e.thread_id = s.thread_id AND e.sequence > t.reflected_through)) \
         ORDER BY s.last_activity DESC",
        ready_window_sql(1)
    ))
    .bind(ready_days)
    .fetch_all(pool)
    .await
}

pub async fn in_scope_thread_count(pool: &PgPool) -> StoreResult<usize> {
    let count: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM thread_summaries s WHERE {THREAD_IN_SCOPE_SQL}"
    ))
    .fetch_one(pool)
    .await?;
    Ok(count as usize)
}

/// INVARIANT: Remove the trees of threads that no longer exist. A delete purges them in
/// its own transaction; this catches a node a drain wrote while it ran.
pub(crate) async fn sweep_deleted_threads(pool: &PgPool) -> StoreResult<u64> {
    let gone = "scope <> 'workspace' AND NOT EXISTS \
                (SELECT 1 FROM thread_summaries s WHERE s.thread_id::text = scope)";
    let nodes = sqlx::query(&format!("DELETE FROM summary_tree_nodes WHERE {gone}"))
        .execute(pool)
        .await?
        .rows_affected();
    sqlx::query(&format!("DELETE FROM summary_tree_scopes WHERE {gone}"))
        .execute(pool)
        .await?;
    Ok(nodes)
}

pub(crate) struct ThreadInfo {
    pub(crate) kind: ThreadKind,
    pub(crate) title: Option<String>,
}

pub(crate) async fn thread_in_scope(pool: &PgPool, id: Uuid) -> StoreResult<Option<ThreadInfo>> {
    let row: Option<(bool, Option<String>)> = sqlx::query_as(&format!(
        "SELECT s.is_coding_agent, s.title FROM thread_summaries s \
         WHERE s.thread_id = $1 AND {THREAD_IN_SCOPE_SQL}"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(is_coding_agent, title)| ThreadInfo {
        kind: ThreadKind::of(is_coding_agent),
        title,
    }))
}

pub(crate) struct LoadedThreadLog {
    pub(crate) log: ThreadLog,
    pub(crate) through: Option<i64>,
    pub(crate) has_newer: bool,
}

pub(crate) async fn load_thread_log(
    pool: &PgPool,
    id: Uuid,
    kind: ThreadKind,
) -> StoreResult<LoadedThreadLog> {
    let (through, newest): (Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT max(sequence) FILTER (WHERE created < now() - make_interval(secs => $2)), \
                max(sequence) \
         FROM events WHERE thread_id = $1",
    )
    .bind(id)
    .bind(HORIZON_SECS as f64)
    .fetch_one(pool)
    .await?;
    let events = thread_events(pool, id, through.unwrap_or(0)).await?;
    Ok(LoadedThreadLog {
        log: log::project(kind, &events),
        through,
        has_newer: newest > through,
    })
}

pub(crate) async fn load_thread_log_now(
    pool: &PgPool,
    id: Uuid,
    kind: ThreadKind,
) -> StoreResult<ThreadLog> {
    let events = thread_events(pool, id, i64::MAX).await?;
    Ok(log::project(kind, &events))
}

async fn thread_events(pool: &PgPool, id: Uuid, through: i64) -> StoreResult<Vec<StoredEvent>> {
    let types: Vec<&str> = log::ENTRY_EVENT_TYPES
        .iter()
        .chain(log::TURN_END_EVENT_TYPES)
        .copied()
        .collect();
    let rows =
        crate::core::store::EventStore::model_thread_events(pool, id, &types, through).await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let actor = EventMeta::from_payload(&row.payload).actor;
            parse_event(row.id, &row.event_type, row.payload).map(|event| StoredEvent {
                id: row.id,
                event,
                actor,
            })
        })
        .collect())
}

pub(crate) async fn thread_info(pool: &PgPool, id: Uuid) -> StoreResult<Option<ThreadInfo>> {
    let row: Option<(bool, Option<String>)> =
        sqlx::query_as("SELECT is_coding_agent, title FROM thread_summaries WHERE thread_id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(is_coding_agent, title)| ThreadInfo {
        kind: ThreadKind::of(is_coding_agent),
        title,
    }))
}

pub struct NodeShape {
    pub bytes: usize,
    pub(crate) source_event_id: Option<Uuid>,
}

pub async fn node_shapes(
    pool: &PgPool,
    scope: SummaryScope,
) -> StoreResult<HashMap<NodeAddr, NodeShape>> {
    let rows: Vec<(i64, i64, i64, Option<Uuid>)> = sqlx::query_as(
        "SELECT start, span, \
           (octet_length(text) + 2 * (length(text) - length(replace(text, E'\\n', ''))))::bigint, \
           source_event_id \
         FROM summary_tree_nodes WHERE scope = $1",
    )
    .bind(scope.as_db())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(start, span, bytes, source_event_id)| {
            (
                NodeAddr {
                    start: start as u64,
                    span: span as u64,
                },
                NodeShape {
                    bytes: bytes as usize,
                    source_event_id,
                },
            )
        })
        .collect())
}

pub async fn nodes_at(
    pool: &PgPool,
    scope: SummaryScope,
    addrs: &[NodeAddr],
) -> StoreResult<HashMap<NodeAddr, StoredNode>> {
    let starts: Vec<i64> = addrs.iter().map(|a| a.start as i64).collect();
    let spans: Vec<i64> = addrs.iter().map(|a| a.span as i64).collect();
    let rows: Vec<NodeRow> = sqlx::query_as(
        "SELECT n.start, n.span, n.text, n.model, n.source_event_id, n.source_thread_id \
         FROM summary_tree_nodes n \
         JOIN unnest($2::bigint[], $3::bigint[]) AS a(start, span) \
           ON n.start = a.start AND n.span = a.span \
         WHERE n.scope = $1",
    )
    .bind(scope.as_db())
    .bind(&starts)
    .bind(&spans)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(NodeRow::into_entry).collect())
}

pub(crate) async fn leaf_event_span(
    pool: &PgPool,
    scope: SummaryScope,
    start: u64,
    end: u64,
) -> StoreResult<EventSpan> {
    sqlx::query_as(
        "SELECT min(e.created), max(e.created) FROM summary_tree_nodes n \
         JOIN events e ON e.id = n.source_event_id \
         WHERE n.scope = $1 AND n.span = 1 AND n.start >= $2 AND n.start < $3",
    )
    .bind(scope.as_db())
    .bind(start as i64)
    .bind(end as i64)
    .fetch_one(pool)
    .await
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, sqlx::FromRow)]
pub struct TreeThread {
    pub(crate) thread_id: Uuid,
    pub(crate) title: Option<String>,
    pub(crate) last_activity: chrono::DateTime<chrono::Utc>,
    pub(crate) summarised: i64,
}

pub async fn tree_threads(pool: &PgPool, limit: i64, offset: i64) -> StoreResult<Vec<TreeThread>> {
    sqlx::query_as(&format!(
        "WITH page AS ( \
           SELECT s.thread_id, s.title, s.last_activity FROM thread_summaries s \
           WHERE {THREAD_IN_SCOPE_SQL} \
           ORDER BY s.last_activity DESC, s.thread_id LIMIT $1 OFFSET $2) \
         SELECT p.thread_id, p.title, p.last_activity, \
                (SELECT count(*) FROM summary_tree_nodes n \
                 WHERE n.scope = p.thread_id::text AND n.span = 1) AS summarised \
         FROM page p ORDER BY p.last_activity DESC, p.thread_id"
    ))
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

pub(crate) async fn artifact_write(
    pool: &PgPool,
    event_id: Uuid,
) -> StoreResult<Option<(String, String)>> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT COALESCE(payload->'data', payload)->>'artifact_path', \
                COALESCE(COALESCE(payload->'data', payload)->>'commit', \
                         COALESCE(payload->'data', payload)->>'commit_hash') \
         FROM events WHERE id = $1",
    )
    .bind(event_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(path, commit)| Some((path?, commit.unwrap_or_default()))))
}

pub(crate) async fn turn_leaf(pool: &PgPool, settle_event_id: Uuid) -> StoreResult<Option<u64>> {
    let start: Option<i64> = sqlx::query_scalar(
        "SELECT start FROM summary_tree_nodes \
         WHERE scope = 'workspace' AND span = 1 AND source_event_id = $1",
    )
    .bind(settle_event_id)
    .fetch_optional(pool)
    .await?;
    Ok(start.map(|s| s as u64))
}

pub(crate) async fn built_leaf_count(pool: &PgPool, scope: SummaryScope) -> StoreResult<u64> {
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM summary_tree_nodes WHERE scope = $1 AND span = 1")
            .bind(scope.as_db())
            .fetch_one(pool)
            .await?;
    Ok(count as u64)
}

pub(crate) async fn leaves_from(
    pool: &PgPool,
    scope: SummaryScope,
    from: u64,
) -> StoreResult<Vec<(NodeAddr, StoredNode)>> {
    let rows: Vec<NodeRow> = sqlx::query_as(
        "SELECT start, span, text, model, source_event_id, source_thread_id \
         FROM summary_tree_nodes WHERE scope = $1 AND span = 1 AND start >= $2 \
         ORDER BY start",
    )
    .bind(scope.as_db())
    .bind(from as i64)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(NodeRow::into_entry).collect())
}

pub(crate) type EventSpan = (
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
);

pub(crate) async fn event_span(pool: &PgPool, event_ids: &[Uuid]) -> StoreResult<EventSpan> {
    sqlx::query_as("SELECT min(created), max(created) FROM events WHERE id = ANY($1)")
        .bind(event_ids)
        .fetch_one(pool)
        .await
}

/// INVARIANT: A stored row back as its typed event. A row that no longer parses is
/// skipped the same way on every read, so the log stays deterministic.
fn parse_event(id: Uuid, event_type: &str, payload: serde_json::Value) -> Option<ThreadEvent> {
    match ThreadEvent::from_stored(event_type, payload) {
        Ok(event) => Some(event),
        Err(e) => {
            log_legacy!(
                "[SummaryTree] Event {} ({}) no longer parses, so the log skips it: {}",
                id,
                event_type,
                e
            );
            None
        }
    }
}

fn scope_texts(ids: &[Uuid]) -> Vec<String> {
    ids.iter()
        .map(|id| SummaryScope::Thread(*id).as_db())
        .collect()
}

const FIRST_FAMILY_LEAF_SQL: &str = "(SELECT min(start) FROM summary_tree_nodes \
       WHERE scope = 'workspace' AND span = 1 AND source_thread_id = ANY($1))";

/// INVARIANT: Purge a deleted family from every tree, inside the delete's transaction
/// (I9). Its own trees go. So do its workspace leaves and every workspace
/// merge from the first of them onward: positions after it shift, so each of
/// those merges is rebuilt. Resetting the workspace's progress makes the next
/// drain, even after a restart, realign and rebuild them.
pub async fn delete_family(tx: &mut Transaction<'_, Postgres>, ids: &[Uuid]) -> StoreResult<()> {
    let scopes = scope_texts(ids);
    sqlx::query("DELETE FROM summary_tree_nodes WHERE scope = ANY($1)")
        .bind(&scopes)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM summary_tree_scopes WHERE scope = ANY($1)")
        .bind(&scopes)
        .execute(&mut **tx)
        .await?;
    let removed = sqlx::query(&format!(
        "DELETE FROM summary_tree_nodes WHERE scope = 'workspace' AND ( \
           (span = 1 AND source_thread_id = ANY($1)) \
           OR (span > 1 AND start + span > {FIRST_FAMILY_LEAF_SQL}))"
    ))
    .bind(ids)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if removed > 0 {
        sqlx::query(
            "UPDATE summary_tree_scopes SET reflected_through = 0 WHERE scope = 'workspace'",
        )
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

pub async fn family_rebuild_count(
    tx: &mut Transaction<'_, Postgres>,
    ids: &[Uuid],
) -> StoreResult<i64> {
    sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM summary_tree_nodes \
         WHERE scope = 'workspace' AND span > 1 AND model IS NOT NULL \
           AND start + span > {FIRST_FAMILY_LEAF_SQL}"
    ))
    .bind(ids)
    .fetch_one(&mut **tx)
    .await
}
