//! Archive all's decisions (ADR 0349): what the confirm counts, and which of
//! the confirmed ids are still safe when the user presses it.
//!
//! Safe means exactly what triage's apply means for `archive`: nothing busy,
//! nothing the user still has to do, and no pin. One rule, two callers.

use std::collections::BTreeMap;

use serde::Serialize;
use uuid::Uuid;

use super::facts::TriageRow;
use super::{refuse_apply, refuse_fresh, NeedFact, TriageAction};

/// A thread the confirm will archive.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SafeThread {
    pub(crate) thread_id: Uuid,
    pub(crate) title: String,
}

/// What the confirm says. `kept` counts each kept thread once, under its first
/// reason, so the counts add up to `kept_count`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ArchiveAllPreflight {
    pub(crate) safe: Vec<SafeThread>,
    pub(crate) kept: BTreeMap<&'static str, usize>,
    pub(crate) kept_count: usize,
}

/// The slug a kept thread is counted under in the confirm: busy first, then
/// its first need. Pinned rows never reach here.
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

/// The confirm for the Current section. A pinned thread sits in Pinned, not
/// Current, so it is neither archived nor counted.
pub(crate) fn preflight(rows: &[TriageRow]) -> ArchiveAllPreflight {
    let mut safe = Vec::new();
    let mut kept = BTreeMap::new();
    let mut kept_count = 0;
    for row in rows.iter().filter(|r| !r.facts.is_pinned) {
        if refuse_apply(TriageAction::Archive, &row.facts).is_none() {
            safe.push(SafeThread {
                thread_id: row.facts.thread_id,
                title: row.title.clone(),
            });
        } else {
            *kept.entry(kept_slug(row)).or_insert(0) += 1;
            kept_count += 1;
        }
    }
    ArchiveAllPreflight {
        safe,
        kept,
        kept_count,
    }
}

/// A confirmed id Archive all did not archive, and why.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct KeptThread {
    pub(crate) thread_id: Uuid,
    pub(crate) reason: String,
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
            }),
        }
    }
    (safe, kept)
}

#[cfg(test)]
#[path = "archive_all_tests.rs"]
mod tests;
