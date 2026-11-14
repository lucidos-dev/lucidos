//! The locked family snapshot, and the one gate archive and delete both ask.
//!
//! Both verbs cascade over a thread and every descendant, and both must refuse
//! while anything down there is live. Two copies of that rule would drift, and
//! the drift would be invisible: each verb's own tests would pass while the two
//! disagreed about the same family. So the recursive CTE, the row shape and the
//! decision live here, and the verb is a parameter.
//!
//! **They refuse the same families.** The verb picks only the refusal slug and
//! which members the caller then sweeps. Both refuse a parent waiting on the
//! user: it needs attention, and neither verb may hide it (ADR 0259).

use axum::http::StatusCode;
use uuid::Uuid;

use crate::engine::thread_lifecycle::{
    action_blocker, own_blocker, ArchiveState, Blocker, ChangeStateKind, ChangeWork, OwnBlocker,
    ThreadStatus, ThreadType,
};

/// Row shape pulled by the recursive CTE in [`load_family`]: the subset of
/// `thread_summaries` that feeds `is_blocking` plus the parent's own gate.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(in crate::api) struct FamilyRow {
    pub(in crate::api) thread_id: Uuid,
    pub(in crate::api) is_coding_agent: bool,
    pub(in crate::api) status: String,
    pub(in crate::api) archive_state: String,
    pub(in crate::api) coding_agent_change_state: ChangeStateKind,
    pub(in crate::api) coding_agent_is_external_repo: bool,
    pub(in crate::api) holds_live_wait: bool,
    /// The pin. An agent's archive leaves a pinned member open (ADR 0312).
    pub(in crate::api) is_saved: bool,
    /// The home thread, which neither verb may take (ADR 0362).
    pub(in crate::api) is_home: bool,
}

impl FamilyRow {
    pub(in crate::api) fn is_proposed(&self) -> bool {
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

    /// This member's own blocker, judged in `archive_state`.
    pub(in crate::api) fn own_blocker(&self, archive_state: ArchiveState) -> Option<OwnBlocker> {
        own_blocker(
            self.thread_type(),
            self.status_enum(),
            archive_state,
            ChangeWork::of(self.coding_agent_change_state, self.holds_live_wait),
            self.coding_agent_is_external_repo,
        )
    }

    pub(in crate::api) fn archive_state_enum(&self) -> ArchiveState {
        ArchiveState::parse(&self.archive_state)
    }
}

/// Slug for a cascade that would take the home thread (ADR 0362).
pub(in crate::api) const HOME_THREAD: &str = "home_thread";

/// Which cascade is asking. It names the refusal, so the frontend can render a
/// sentence per verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::api) enum FamilyVerb {
    Archive,
    Delete,
}

impl FamilyVerb {
    /// Machine-readable slug for a parent the verb cannot act on. Each verb
    /// names itself, because the frontend renders a different sentence.
    fn parent_blocked_reason(self) -> &'static str {
        match self {
            Self::Archive => "parent_not_archivable",
            Self::Delete => "parent_not_deletable",
        }
    }
}

/// May the cascade run over this locked family?
///
/// `Proceed` carries nothing. The caller already owns the `&[FamilyRow]` it
/// passed in, and each verb wants a different subset of it.
pub(in crate::api) enum FamilyDecision {
    Proceed,
    Reject {
        status: StatusCode,
        body: serde_json::Value,
    },
}

/// Lock parent plus every descendant inside a transaction, so no state can flip
/// between validation and the cascade. The caller owns the transaction, so the
/// lock is released by its own commit or rollback.
///
/// The lock names the base table (`FOR UPDATE OF t`). Postgres skips a WITH
/// query that a bare `FOR UPDATE` reaches, so locking the recursive CTE itself
/// locks no row at all.
///
/// Rows are locked deepest first, the order `lock_edge_for_detach` uses, so the
/// two cannot deadlock. They are returned parent first, in family order.
pub(in crate::api) async fn load_family(
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

/// The pure decision, given a locked family snapshot and the target. Split out
/// so tests drive it without a live HTTP stack.
///
/// The decision is [`action_blocker`] over the family, the function the
/// thread menu draws from, so the menu's reason and this refusal's `blocker`
/// slug always agree:
///
///   1. A family holding the home thread, whichever member it is (ADR 0362).
///   2. A parent that is running, waiting on the user (ADR 0259), or an
///      in-workspace coding-agent thread with a pending change. It is judged
///      as if in the inbox, so an archived parent cannot hide live work.
///      External-repo coding agents are exempt from the change clause, because
///      Apply cannot merge into a foreign repo and the cascade is their only
///      exit.
///   3. Any descendant that blocks in its own real `archive_state`, which is
///      what lets an archived descendant holding a pending change through.
///
/// An already-archived parent is NOT refused, and that is load-bearing for
/// archive: rejecting it produced a stuck button. A frontend whose
/// `meta.section` had desynced to inbox offered Archive, the 409 rolled its
/// optimistic flip back, and the button reappeared on every tap.
pub(in crate::api) fn classify_family(
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

    // Strongest first, so `blocking[0]` is a member holding the named
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

/// `body` with the `blocker` slug the client words the refusal from.
pub(in crate::api) fn with_blocker(
    mut body: serde_json::Value,
    blocker: Blocker,
) -> serde_json::Value {
    body["blocker"] = blocker.as_str().into();
    body
}

/// The refusal body for a parent the verb cannot act on: running, or waiting on
/// the user. One builder, so an agent's archive answers exactly what the
/// Archive route does (ADR 0310).
pub(in crate::api) fn parent_blocked_body(
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

/// Every member, in family order. What delete sweeps.
pub(in crate::api) fn every_member(family: &[FamilyRow]) -> Vec<Uuid> {
    family.iter().map(|r| r.thread_id).collect()
}

/// Members not already archived. What the archive cascade emits for, so a
/// second Archive is a no-op success rather than a duplicate emit.
pub(in crate::api) fn not_yet_archived(family: &[FamilyRow]) -> Vec<Uuid> {
    family
        .iter()
        .filter(|r| r.archive_state_enum() != ArchiveState::Archived)
        .map(|r| r.thread_id)
        .collect()
}

/// External-repo coding-agent members holding a pending change, among those not
/// already archived. Archive clears each with `ChangeApplied` before the
/// `ThreadArchived` emit, so the change row does not dangle.
///
/// Delete has no use for this. It removes the `changes` rows outright, and
/// emitting `ChangeApplied` would claim a merge that never happened.
pub(in crate::api) fn external_repo_pending(family: &[FamilyRow]) -> Vec<Uuid> {
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

/// Coding-agent members, whatever repo they belong to. Delete reclaims a
/// worktree and branch for each.
pub(in crate::api) fn coding_agent_members(family: &[FamilyRow]) -> Vec<Uuid> {
    family
        .iter()
        .filter(|r| r.is_coding_agent)
        .map(|r| r.thread_id)
        .collect()
}

#[cfg(test)]
#[path = "family_tests.rs"]
mod tests;
