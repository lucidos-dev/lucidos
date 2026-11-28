use crate::engine::archive_policy::PINNED_SUB_THREAD;
use crate::engine::archive_policy::SAME_FAMILY;
use std::collections::{BTreeMap, HashSet};

use serde::Serialize;
use uuid::Uuid;

use super::facts::TriageRow;
use super::{refuse_apply, refuse_completed, refuse_fresh, NeedFact, TriageAction};
use crate::engine::thread_lifecycle::Blocker;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SafeThread {
    pub(crate) thread_id: Uuid,
    pub(crate) title: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ArchiveAllPreflight {
    pub(crate) safe: Vec<SafeThread>,
    pub(crate) safe_thread_count: usize,
    pub(crate) kept: BTreeMap<&'static str, usize>,
    pub(crate) kept_thread_count: usize,
}

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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct KeptThread {
    pub(crate) thread_id: Uuid,
    pub(crate) reason: String,
    pub(crate) slug: &'static str,
    pub(crate) thread_count: usize,
}

fn fresh_kept_slug(row: Option<&TriageRow>) -> &'static str {
    match row {
        None => "gone",
        Some(row) if row.section != "inbox" => "archived",
        Some(row) if row.facts.busy_reason().is_some() || row.facts.needs_user() => kept_slug(row),
        Some(_) => "pinned",
    }
}

pub(crate) fn refusal_kept_slug(body: &serde_json::Value) -> &'static str {
    let field = |key: &str| body.get(key).and_then(|v| v.as_str());
    match field("reason") {
        Some("thread_not_found") => return "gone",
        Some(crate::engine::archive_family::THREAD_PINNED) => return "pinned",
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

pub(crate) fn completed_preflight(rows: &[TriageRow]) -> ArchiveAllPreflight {
    let safe: Vec<_> = rows
        .iter()
        .filter(|r| refuse_completed(Some(r)).is_none())
        .map(|r| SafeThread {
            thread_id: r.facts.thread_id,
            title: r.title.clone(),
        })
        .collect();
    let safe_thread_count = safe.len();
    ArchiveAllPreflight {
        safe,
        safe_thread_count,
        kept: BTreeMap::new(),
        kept_thread_count: rows.len() - safe_thread_count,
    }
}

#[cfg(test)]
#[path = "archive_all_tests.rs"]
mod tests;
