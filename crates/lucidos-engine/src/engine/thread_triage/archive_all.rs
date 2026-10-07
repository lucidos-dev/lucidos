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
use crate::engine::thread_lifecycle::Blocker;

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

/// A confirmed id Archive all did not archive, and why: `reason` in words for
/// an agent, `slug` for the client to word as the confirm does.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct KeptThread {
    pub(crate) thread_id: Uuid,
    pub(crate) reason: String,
    pub(crate) slug: &'static str,
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
            }),
        }
    }
    (safe, kept)
}

#[cfg(test)]
#[path = "archive_all_tests.rs"]
mod tests;
