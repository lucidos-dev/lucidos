//! The locked family snapshot, and the one gate archive and delete both ask.
//!
//! Both verbs cascade over a thread and every descendant, and both must refuse
//! while anything down there is live. Two copies of that rule would drift, and
//! the drift would be invisible: each verb's own tests would pass while the two
//! disagreed about the same family. So the recursive CTE, the row shape and the
//! decision live here, and the verb is a parameter.
//!
//! **They differ in exactly one place.** Archive admits a parent in
//! `WaitingForUserAnswer` and cancel-stamps its question card. Delete refuses
//! it: there is nothing to stamp when the card is about to go, and a subprocess
//! parked on the question would be orphaned.

use axum::http::StatusCode;
use uuid::Uuid;

use crate::engine::thread_lifecycle::{is_blocking, ArchiveState, ThreadStatus, ThreadType};

/// Row shape pulled by the recursive CTE in [`load_family`]: the subset of
/// `thread_summaries` that feeds `is_blocking` plus the parent's own gate.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(in crate::api) struct FamilyRow {
    pub(in crate::api) thread_id: Uuid,
    pub(in crate::api) is_coding_agent: bool,
    pub(in crate::api) status: String,
    pub(in crate::api) archive_state: String,
    pub(in crate::api) coding_agent_proposed: bool,
    pub(in crate::api) coding_agent_is_external_repo: bool,
}

impl FamilyRow {
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

    pub(in crate::api) fn archive_state_enum(&self) -> ArchiveState {
        ArchiveState::parse(&self.archive_state)
    }
}

/// Which cascade is asking. The two verbs disagree on one state, so this is a
/// parameter rather than a second classifier.
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

    /// Does a parent in `WaitingForUserAnswer` block this verb?
    fn refuses_a_parked_parent(self) -> bool {
        match self {
            Self::Archive => false,
            Self::Delete => true,
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
pub(in crate::api) async fn load_family(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_uuid: Uuid,
) -> Result<Vec<FamilyRow>, sqlx::Error> {
    sqlx::query_as::<_, FamilyRow>(
        "WITH RECURSIVE family AS (
            SELECT thread_id, parent_thread_id, is_coding_agent, status,
                   archive_state, coding_agent_proposed,
                   coding_agent_is_external_repo
            FROM thread_summaries
            WHERE thread_id = $1
            UNION ALL
            SELECT t.thread_id, t.parent_thread_id, t.is_coding_agent, t.status,
                   t.archive_state, t.coding_agent_proposed,
                   t.coding_agent_is_external_repo
            FROM thread_summaries t
            JOIN family f ON t.parent_thread_id = f.thread_id
        )
        SELECT thread_id, is_coding_agent, status, archive_state,
               coding_agent_proposed, coding_agent_is_external_repo
        FROM family
        FOR UPDATE",
    )
    .bind(thread_uuid)
    .fetch_all(&mut **tx)
    .await
}

/// The pure decision, given a locked family snapshot and the target. Split out
/// so tests drive it without a live HTTP stack.
///
/// The parent gate refuses three states:
///
///   1. Running. Live work cannot be terminal, whatever `archive_state` says.
///   2. `WaitingForUserAnswer`, for [`FamilyVerb::Delete`] only.
///   3. An in-workspace coding-agent thread with a pending change. The user must
///      Apply or Discard first, which is what `resolve_actions` already offers
///      there. External-repo coding agents are exempt, because Apply cannot
///      merge into a foreign repo and the cascade is their only exit.
///
/// Descendants are judged by `is_blocking` with their own real `archive_state`,
/// which is what lets an archived descendant holding a pending change through.
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

    // An already-archived parent is NOT refused, and that is load-bearing for
    // archive: rejecting here produced a stuck button. A frontend whose
    // `meta.section` had desynced to inbox offered Archive, the 409 rolled its
    // optimistic flip back, and the button reappeared on every tap.
    let parked = parent_row.status_enum() == ThreadStatus::WaitingForUserAnswer;
    if parent_row.status_enum() == ThreadStatus::Running
        || (parked && verb.refuses_a_parked_parent())
    {
        return FamilyDecision::Reject {
            status: StatusCode::CONFLICT,
            body: serde_json::json!({
                "reason": verb.parent_blocked_reason(),
                "parent_status": parent_row.status,
                "has_pending_changes": parent_row.coding_agent_proposed,
            }),
        };
    }
    if parent_row.is_coding_agent
        && parent_row.coding_agent_proposed
        && !parent_row.coding_agent_is_external_repo
    {
        return FamilyDecision::Reject {
            status: StatusCode::CONFLICT,
            body: serde_json::json!({
                "reason": "parent_has_pending_changes",
            }),
        };
    }

    let blockers: Vec<&FamilyRow> = family
        .iter()
        .filter(|r| r.thread_id != thread_uuid)
        .filter(|r| {
            is_blocking(
                r.thread_type(),
                r.status_enum(),
                r.archive_state_enum(),
                r.coding_agent_proposed,
                r.coding_agent_is_external_repo,
            )
        })
        .collect();
    if !blockers.is_empty() {
        return FamilyDecision::Reject {
            status: StatusCode::CONFLICT,
            body: serde_json::json!({
                "reason": "descendants_blocking",
                "blocking": blockers.iter().map(|r| serde_json::json!({
                    "thread_id": r.thread_id,
                    "status": r.status,
                    "has_pending_changes": r.coding_agent_proposed,
                })).collect::<Vec<_>>(),
            }),
        };
    }

    FamilyDecision::Proceed
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
                && r.coding_agent_proposed
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
