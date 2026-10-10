//! The workspace log: one leaf per settled thread turn and per artifact write,
//! in event sequence order.
//!
//! A turn leaf exists for a turn-end event of an in-scope thread whose turn
//! holds content. A trigger thread's turns count only after its first human
//! message, so joining appends to the log rather than shifting it. Like a
//! thread log, it is a pure function of events and grows only at the end. A
//! delete is the exception: it removes leaves, and the ones after move up.

use sqlx::PgPool;
use uuid::Uuid;

use super::log::{ENTRY_EVENT_TYPES, TURN_END_EVENT_TYPES};
use super::HORIZON_SECS;

/// Every event type a workspace leaf comes from. The partial index
/// `idx_events_summary_tree_workspace_leaves` lists the same names, and the
/// query spells them as literals so the planner can use it.
pub(crate) const WORKSPACE_LEAF_EVENT_TYPES: &[&str] = &[
    "ResponseGenerated",
    "ResponseCanceled",
    "ResponseAborted",
    "ResponseFailed",
    "CodingAgentIdled",
    "ArtifactCreated",
    "ArtifactUpdated",
    "ArtifactImported",
];

/// The events that announce an artifact write, each able to name its writer
/// thread.
pub(crate) const ARTIFACT_WRITE_EVENT_TYPES: &[&str] =
    &["ArtifactCreated", "ArtifactUpdated", "ArtifactImported"];

/// One workspace leaf, by the event it stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceLeaf {
    Turn {
        event_id: Uuid,
        thread_id: Uuid,
    },
    Artifact {
        event_id: Uuid,
        /// Relative to `data/artifacts/`.
        path: String,
        commit: String,
        verb: &'static str,
        /// The thread whose turn wrote it, while that thread still exists.
        writer_thread_id: Option<Uuid>,
    },
}

impl WorkspaceLeaf {
    pub(crate) fn event_id(&self) -> Uuid {
        match self {
            Self::Turn { event_id, .. } | Self::Artifact { event_id, .. } => *event_id,
        }
    }

    /// The thread this leaf belongs to. Deleting that thread removes it.
    pub(crate) fn thread_id(&self) -> Option<Uuid> {
        match self {
            Self::Turn { thread_id, .. } => Some(*thread_id),
            Self::Artifact { .. } => None,
        }
    }

    /// The thread a model call about this leaf is recorded on: a turn's own
    /// thread, or the thread that wrote an artifact.
    pub(crate) fn capture_thread(&self) -> Option<Uuid> {
        match self {
            Self::Turn { thread_id, .. } => Some(*thread_id),
            Self::Artifact {
                writer_thread_id, ..
            } => *writer_thread_id,
        }
    }

    /// Drop a writer that was deleted, so no call is recorded on it.
    pub(crate) fn forget_writer_in(&mut self, deleted: &std::collections::HashSet<Uuid>) {
        if let Self::Artifact {
            writer_thread_id, ..
        } = self
        {
            if writer_thread_id.is_some_and(|w| deleted.contains(&w)) {
                *writer_thread_id = None;
            }
        }
    }
}

/// The workspace log as far as it has been read.
#[derive(Debug, Default)]
pub(crate) struct WorkspaceLog {
    pub(crate) leaves: Vec<WorkspaceLeaf>,
    /// The newest event sequence read.
    pub(crate) through: i64,
}

pub(crate) fn sql_list(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("'{n}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl WorkspaceLog {
    /// Read the leaves past `through`, up to the horizon. Returns whether
    /// leaf events newer than the horizon wait, so the caller comes back.
    pub(crate) async fn extend(&mut self, pool: &PgPool) -> Result<bool, sqlx::Error> {
        let leaf_types = sql_list(WORKSPACE_LEAF_EVENT_TYPES);
        // The cursor moves past every leaf-type event older than the horizon,
        // including those the filter below drops, so none is read twice.
        let (horizon, has_newer): (Option<i64>, Option<bool>) = sqlx::query_as(&format!(
            "SELECT max(sequence) FILTER (WHERE created < now() - make_interval(secs => $2)), \
                    bool_or(created >= now() - make_interval(secs => $2)) \
             FROM events WHERE event_type IN ({leaf_types}) AND sequence > $1"
        ))
        .bind(self.through)
        .bind(HORIZON_SECS as f64)
        .fetch_one(pool)
        .await?;
        let Some(horizon) = horizon else {
            return Ok(has_newer.unwrap_or(false));
        };
        let turn_ends = sql_list(TURN_END_EVENT_TYPES);
        let artifact_writes = sql_list(ARTIFACT_WRITE_EVENT_TYPES);
        // A system event keeps its fields under `data`; a few early artifact
        // rows were written flat. A writer that was since deleted reads as none.
        let sql = format!(
            "SELECT b.id, b.event_type, b.thread_id, \
                    COALESCE(b.payload->'data', b.payload)->>'artifact_path', \
                    COALESCE(COALESCE(b.payload->'data', b.payload)->>'commit', \
                             COALESCE(b.payload->'data', b.payload)->>'commit_hash'), \
                    w.thread_id \
             FROM events b \
             LEFT JOIN thread_summaries s ON s.thread_id = b.thread_id \
             LEFT JOIN thread_summaries w ON w.thread_id = \
               (COALESCE(b.payload->'data', b.payload)->>'writer_thread_id')::uuid \
             WHERE b.event_type IN ({leaf_types}) \
               AND b.sequence > $1 AND b.sequence <= $2 \
               AND (b.event_type IN ({artifact_writes}) \
                 OR (s.thread_id IS NOT NULL \
                   AND (s.source <> 'trigger' OR b.sequence > ( \
                     SELECT min(m.sequence) FROM events m \
                     WHERE m.thread_id = b.thread_id AND m.event_type = 'MessageReceived' \
                       AND m.payload->>'mode' = 'human')) \
                   AND (COALESCE(b.payload->>'text', '') <> '' \
                     OR b.event_type = 'ResponseFailed' \
                     OR EXISTS ( \
                       SELECT 1 FROM events x \
                       WHERE x.thread_id = b.thread_id AND x.sequence < b.sequence \
                         AND x.event_type = ANY($3) \
                         AND x.sequence > COALESCE(( \
                           SELECT max(p.sequence) FROM events p \
                           WHERE p.thread_id = b.thread_id AND p.sequence < b.sequence \
                             AND p.event_type IN ({turn_ends})), 0))))) \
             ORDER BY b.sequence",
        );
        type Row = (
            Uuid,
            String,
            Option<Uuid>,
            Option<String>,
            Option<String>,
            Option<Uuid>,
        );
        let rows: Vec<Row> = sqlx::query_as(&sql)
            .bind(self.through)
            .bind(horizon)
            .bind(ENTRY_EVENT_TYPES)
            .fetch_all(pool)
            .await?;
        for (event_id, event_type, thread_id, path, commit, writer_thread_id) in rows {
            if let Some(leaf) = leaf_for(
                event_id,
                &event_type,
                thread_id,
                path,
                commit,
                writer_thread_id,
            ) {
                self.leaves.push(leaf);
            }
        }
        self.through = horizon;
        Ok(has_newer.unwrap_or(false))
    }
}

fn leaf_for(
    event_id: Uuid,
    event_type: &str,
    thread_id: Option<Uuid>,
    path: Option<String>,
    commit: Option<String>,
    writer_thread_id: Option<Uuid>,
) -> Option<WorkspaceLeaf> {
    let verb = match event_type {
        "ArtifactCreated" => "created",
        "ArtifactUpdated" => "updated",
        "ArtifactImported" => "imported",
        _ => {
            return thread_id.map(|thread_id| WorkspaceLeaf::Turn {
                event_id,
                thread_id,
            })
        }
    };
    let path = crate::engine::LucidosEngine::canonicalize_artifact_path(&path?).to_string();
    // The memory indexer's filter: binaries and engine bookkeeping files say
    // nothing a summary line could use.
    if crate::engine::LucidosEngine::should_skip_artifact_for_memory(&path) {
        return None;
    }
    Some(WorkspaceLeaf::Artifact {
        event_id,
        path,
        commit: commit.unwrap_or_default(),
        verb,
        writer_thread_id,
    })
}
