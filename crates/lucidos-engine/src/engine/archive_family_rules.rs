use axum::http::StatusCode;
use uuid::Uuid;

use crate::engine::thread_lifecycle::{
    action_blocker, own_blocker, ArchiveState, Blocker, ChangeStateKind, ChangeWork, OwnBlocker,
    ThreadStatus, ThreadType,
};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FamilyRow {
    pub thread_id: Uuid,
    pub(crate) is_coding_agent: bool,
    pub(crate) status: String,
    pub(crate) archive_state: String,
    pub(crate) coding_agent_change_state: ChangeStateKind,
    pub(crate) coding_agent_is_external_repo: bool,
    pub(crate) holds_live_wait: bool,
    pub(crate) is_saved: bool,
    pub is_home: bool,
}

impl FamilyRow {
    pub(crate) fn is_proposed(&self) -> bool {
        self.coding_agent_change_state == ChangeStateKind::Proposed
    }

    fn thread_type(&self) -> ThreadType {
        if self.is_coding_agent {
            ThreadType::CodingAgent
        } else {
            ThreadType::Chat
        }
    }

    fn status_enum(&self) -> ThreadStatus {
        ThreadStatus::parse(&self.status)
    }

    pub fn own_blocker(&self, archive_state: ArchiveState) -> Option<OwnBlocker> {
        own_blocker(
            self.thread_type(),
            self.status_enum(),
            archive_state,
            ChangeWork::of(self.coding_agent_change_state, self.holds_live_wait),
            self.coding_agent_is_external_repo,
        )
    }

    pub fn archive_state_enum(&self) -> ArchiveState {
        ArchiveState::parse(&self.archive_state)
    }
}

pub const HOME_THREAD: &str = "home_thread";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FamilyVerb {
    Archive,
    Delete,
}

impl FamilyVerb {
    fn parent_blocked_reason(self) -> &'static str {
        match self {
            Self::Archive => "parent_not_archivable",
            Self::Delete => "parent_not_deletable",
        }
    }
}

pub enum FamilyDecision {
    Proceed,
    Reject {
        status: StatusCode,
        body: serde_json::Value,
    },
}

/// INVARIANT: Lock parent plus every descendant inside a transaction, so no state can flip
/// between validation and the cascade. The caller owns the transaction, so the
/// lock is released by its own commit or rollback.
///
/// The lock names the base table (`FOR UPDATE OF t`). Postgres skips a WITH
/// query that a bare `FOR UPDATE` reaches, so locking the recursive CTE itself
/// locks no row at all.
///
/// Rows are locked deepest first, the order `lock_edge_for_detach` uses, so the
/// two cannot deadlock. They are returned parent first, in family order.
pub async fn load_family(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_uuid: Uuid,
) -> Result<Vec<FamilyRow>, sqlx::Error> {
    sqlx::query_as::<_, FamilyRow>(
        "WITH RECURSIVE family AS (
            SELECT thread_id, 0 AS level
            FROM thread_summaries
            WHERE thread_id = $1
            UNION ALL
            SELECT t.thread_id, f.level + 1
            FROM thread_summaries t
            JOIN family f ON t.parent_thread_id = f.thread_id
        ),
        locked AS MATERIALIZED (
            SELECT t.thread_id, t.is_coding_agent, t.status, t.archive_state,
                   t.coding_agent_change_state, t.coding_agent_is_external_repo,
                   t.live_event_wait_count > 0 AS holds_live_wait,
                   t.is_saved, t.is_home, f.level
            FROM thread_summaries t
            JOIN family f ON f.thread_id = t.thread_id
            ORDER BY t.depth DESC, t.thread_id
            FOR UPDATE OF t
        )
        SELECT thread_id, is_coding_agent, status, archive_state,
               coding_agent_change_state, coding_agent_is_external_repo,
               holds_live_wait, is_saved, is_home
        FROM locked
        ORDER BY level, thread_id",
    )
    .bind(thread_uuid)
    .fetch_all(&mut **tx)
    .await
}

pub fn classify_family(
    family: &[FamilyRow],
    thread_uuid: Uuid,
    verb: FamilyVerb,
) -> FamilyDecision {
    let Some(parent_row) = family.iter().find(|r| r.thread_id == thread_uuid) else {
        return FamilyDecision::Reject {
            status: StatusCode::NOT_FOUND,
            body: serde_json::json!({ "reason": "thread_not_found" }),
        };
    };

    // INVARIANT: Strongest first, so `blocking[0]` is a member holding the named
    // blocker: the client's Show sub-thread opens it.
    let mut blockers: Vec<(&FamilyRow, OwnBlocker)> = family
        .iter()
        .filter(|r| r.thread_id != thread_uuid)
        .filter_map(|r| r.own_blocker(r.archive_state_enum()).map(|b| (r, b)))
        .collect();
    blockers.sort_by_key(|(_, b)| *b);
    let blocker = action_blocker(
        parent_row.own_blocker(ArchiveState::Inbox),
        family.iter().any(|r| r.is_home),
        blockers.iter().map(|(_, b)| *b).min(),
    );
    let body = match blocker {
        Blocker::None => return FamilyDecision::Proceed,
        Blocker::Home => serde_json::json!({ "reason": HOME_THREAD }),
        Blocker::Running | Blocker::Question => {
            parent_blocked_body(verb, &parent_row.status, parent_row.is_proposed())
        }
        Blocker::PendingChange => serde_json::json!({ "reason": "parent_has_pending_changes" }),
        Blocker::HeldProposal => serde_json::json!({ "reason": "parent_holds_held_proposal" }),
        Blocker::DescendantRunning
        | Blocker::DescendantQuestion
        | Blocker::DescendantPendingChange
        | Blocker::DescendantHeldProposal => serde_json::json!({
            "reason": "descendants_blocking",
            "blocking": blockers.iter().map(|(r, _)| serde_json::json!({
                "thread_id": r.thread_id,
                "status": r.status,
                "has_pending_changes": r.is_proposed(),
            })).collect::<Vec<_>>(),
        }),
    };
    FamilyDecision::Reject {
        status: StatusCode::CONFLICT,
        body: with_blocker(body, blocker),
    }
}

pub fn with_blocker(mut body: serde_json::Value, blocker: Blocker) -> serde_json::Value {
    body["blocker"] = blocker.as_str().into();
    body
}

pub(crate) fn parent_blocked_body(
    verb: FamilyVerb,
    parent_status: &str,
    has_pending_changes: bool,
) -> serde_json::Value {
    serde_json::json!({
        "reason": verb.parent_blocked_reason(),
        "parent_status": parent_status,
        "has_pending_changes": has_pending_changes,
    })
}

pub fn every_member(family: &[FamilyRow]) -> Vec<Uuid> {
    family.iter().map(|r| r.thread_id).collect()
}

pub(crate) fn not_yet_archived(family: &[FamilyRow]) -> Vec<Uuid> {
    family
        .iter()
        .filter(|r| r.archive_state_enum() != ArchiveState::Archived)
        .map(|r| r.thread_id)
        .collect()
}

/// INVARIANT: External-repo coding-agent members holding a pending change, among those not
/// already archived. Archive clears each with `ChangeApplied` before the
/// `ThreadArchived` emit, so the change row does not dangle.
///
/// Delete has no use for this. It removes the `changes` rows outright, and
/// emitting `ChangeApplied` would claim a merge that never happened.
pub(crate) fn external_repo_pending(family: &[FamilyRow]) -> Vec<Uuid> {
    family
        .iter()
        .filter(|r| {
            r.archive_state_enum() != ArchiveState::Archived
                && r.is_coding_agent
                && r.coding_agent_is_external_repo
                && r.is_proposed()
        })
        .map(|r| r.thread_id)
        .collect()
}

pub fn coding_agent_members(family: &[FamilyRow]) -> Vec<Uuid> {
    family
        .iter()
        .filter(|r| r.is_coding_agent)
        .map(|r| r.thread_id)
        .collect()
}

#[cfg(test)]
#[path = "family_tests.rs"]
mod tests;
