//! Archive all's decisions (ADR 0349): what the confirm counts, and which of
//! the confirmed ids are still safe when the user presses it.
//!
//! Safe means exactly what triage's apply means for `archive`: nothing busy,
//! nothing the user still has to do, and no pin. One rule, two callers.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;
use uuid::Uuid;

use super::facts::TriageRow;
use super::{refuse_apply, refuse_fresh, NeedFact, TriageAction};
use crate::engine::thread_lifecycle::Blocker;

/// A thread the confirm will archive.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SafeThread {
    pub(crate) thread_id: Uuid,
    pub(crate) title: String,
}

/// What the confirm says. It counts inbox threads, sub-threads included, as
/// the Current badge does, so the two totals add up to that badge.
///
/// `safe` lists the family roots the press sends back; `safe_thread_count`
/// counts every inbox thread their archives take. `kept` counts every inbox
/// thread that stays, so its values add up to `kept_thread_count`. A kept
/// family counts once under its first reason, and its other inbox threads
/// under [`SAME_FAMILY`]. A pinned sub-thread of a safe family counts under
/// [`PINNED_SUB_THREAD`], because the archive leaves it open.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ArchiveAllPreflight {
    pub(crate) safe: Vec<SafeThread>,
    pub(crate) safe_thread_count: usize,
    pub(crate) kept: BTreeMap<&'static str, usize>,
    pub(crate) kept_thread_count: usize,
}

/// The other inbox threads of a kept family.
pub(crate) const SAME_FAMILY: &str = "same_family";
/// A pinned sub-thread the archive of its safe root leaves open.
pub(crate) const PINNED_SUB_THREAD: &str = "pinned_sub_thread";

impl TriageRow {
    /// The threads this family counts: the root, wherever it sits, and its
    /// inbox sub-threads. A root archived since the confirm still counts once,
    /// so the result names it under `archived`.
    pub(crate) fn counted_threads(&self) -> usize {
        1 + self.inbox_sub_threads
    }
}

/// The slug a kept family's root is counted under in the confirm: busy first,
/// then its first need. Its other threads count under [`SAME_FAMILY`]. Pinned rows never reach here.
fn kept_slug(row: &TriageRow) -> &'static str {
    let facts = &row.facts;
    if facts.busy_reason().is_some() {
        return "busy";
    }
    let first_need = facts
        .own_needs()
        .into_iter()
        .chain(facts.sub_thread_needs.iter().copied())
        .next();
    match first_need {
        Some(NeedFact::Question) => "question",
        Some(NeedFact::PendingChange) => "pending_change",
        Some(NeedFact::UnproposedWork) => "unproposed_work",
        Some(NeedFact::Draft) => "draft",
        Some(NeedFact::FailedRun) => "failed_run",
        None => "busy",
    }
}

/// The confirm for the Current section, family by family as the drawer draws
/// them. A family whose root is pinned sits in Pinned, so it is neither
/// archived nor counted. A pinned root in a family that also holds an unpinned
/// one renders in Current, so it is counted as kept.
pub(crate) fn preflight(rows: &[TriageRow]) -> ArchiveAllPreflight {
    let current_families: HashSet<Uuid> = rows
        .iter()
        .filter(|r| !r.facts.is_pinned)
        .map(|r| r.family_root)
        .collect();
    let mut safe = Vec::new();
    let mut safe_thread_count = 0;
    let mut kept = BTreeMap::new();
    let mut count = |slug: &'static str, n: usize| {
        if n > 0 {
            *kept.entry(slug).or_insert(0) += n;
        }
    };
    for row in rows {
        if row.facts.is_pinned {
            if !row.family_pinned && current_families.contains(&row.family_root) {
                count(PINNED_SUB_THREAD, 1);
                count(SAME_FAMILY, row.inbox_sub_threads);
            }
            continue;
        }
        if refuse_apply(TriageAction::Archive, &row.facts).is_none() {
            safe.push(SafeThread {
                thread_id: row.facts.thread_id,
                title: row.title.clone(),
            });
            safe_thread_count += row.counted_threads() - row.pinned_sub_threads;
            count(PINNED_SUB_THREAD, row.pinned_sub_threads);
        } else {
            count(kept_slug(row), 1);
            count(SAME_FAMILY, row.inbox_sub_threads);
        }
    }
    let kept_thread_count = kept.values().sum();
    ArchiveAllPreflight {
        safe,
        safe_thread_count,
        kept,
        kept_thread_count,
    }
}

/// A confirmed id Archive all did not archive, and why: `reason` in words for
/// an agent, `slug` for the client to word as the confirm does.
/// `thread_count` is the root plus its inbox sub-threads ([`TriageRow::counted_threads`]).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct KeptThread {
    pub(crate) thread_id: Uuid,
    pub(crate) reason: String,
    pub(crate) slug: &'static str,
    pub(crate) thread_count: usize,
}

/// The slug for a confirmed id that changed since the confirm: the preflight's
/// slugs, plus the three ways a row can leave the plan. Checked in
/// `refuse_fresh`'s order, so the slug names the refusal that fired.
fn fresh_kept_slug(row: Option<&TriageRow>) -> &'static str {
    match row {
        None => "gone",
        Some(row) if row.section != "inbox" => "archived",
        Some(row) if row.facts.busy_reason().is_some() || row.facts.needs_user() => kept_slug(row),
        Some(_) => "pinned",
    }
}

/// The slug for a family the archive cascade refused, from its refusal body: a
/// thread gone or pinned since the confirm, else its `blocker`. Any other
/// refusal, such as a change claim, is something still working.
pub(crate) fn refusal_kept_slug(body: &serde_json::Value) -> &'static str {
    let field = |key: &str| body.get(key).and_then(|v| v.as_str());
    match field("reason") {
        Some("thread_not_found") => return "gone",
        Some(crate::api::threads::archive::THREAD_PINNED) => return "pinned",
        _ => {}
    }
    let is = |b: Blocker| field("blocker") == Some(b.as_str());
    if is(Blocker::Question) || is(Blocker::DescendantQuestion) {
        "question"
    } else if is(Blocker::PendingChange) || is(Blocker::DescendantPendingChange) {
        "pending_change"
    } else {
        "busy"
    }
}

/// Split the confirmed ids into the ones still safe and the ones that changed.
/// `rows` is a fresh read of exactly `confirmed`. Nothing outside `confirmed`
/// is ever returned as safe: the confirm is the ceiling.
pub(crate) fn still_safe(rows: &[TriageRow], confirmed: &[Uuid]) -> (Vec<Uuid>, Vec<KeptThread>) {
    let mut safe = Vec::new();
    let mut kept = Vec::new();
    for id in confirmed {
        let row = rows.iter().find(|r| r.facts.thread_id == *id);
        match refuse_fresh(TriageAction::Archive, row) {
            None => safe.push(*id),
            Some(reason) => kept.push(KeptThread {
                thread_id: *id,
                reason,
                slug: fresh_kept_slug(row),
                thread_count: row.map_or(1, TriageRow::counted_threads),
            }),
        }
    }
    (safe, kept)
}

#[cfg(test)]
#[path = "archive_all_tests.rs"]
mod tests;
