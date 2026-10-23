//! What `triage` shows the agent, and which `apply_triage` entries may run.
//! Both are pure, so the grouping and every refusal are testable without an
//! engine.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;
use uuid::Uuid;

use super::facts::TriageRow;
use super::{classify, idle_words, refuse_fresh, TriageAction};
use crate::engine::thread_events::TriageProposalEntry;

/// The most threads one triage lists. A larger inbox is triaged in passes:
/// apply this one, then run `triage` again.
pub(crate) const MAX_ENTRIES: usize = 300;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ReportEntry {
    pub(crate) thread_id: Uuid,
    /// For the reader only. The action and reason never read it.
    pub(crate) title: String,
    pub(crate) link: String,
    pub(crate) action: TriageAction,
    pub(crate) reason: String,
    pub(crate) last_activity: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ReportGroup {
    /// `trigger runs: <name>`, or `other threads`.
    pub(crate) group: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) trigger_id: Option<String>,
    pub(crate) entries: Vec<ReportEntry>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) counts: BTreeMap<&'static str, usize>,
    pub(crate) groups: Vec<ReportGroup>,
    /// Roots beyond [`MAX_ENTRIES`], left for the next pass.
    pub(crate) not_listed: usize,
}

impl Report {
    /// The entries the proposal event records: every listed thread.
    pub(crate) fn proposal_entries(&self) -> Vec<TriageProposalEntry> {
        self.groups
            .iter()
            .flat_map(|g| &g.entries)
            .map(|e| TriageProposalEntry {
                thread_id: e.thread_id,
                action: e.action.as_str().to_string(),
                reason: e.reason.clone(),
            })
            .collect()
    }
}

/// Classify the rows and group them: one group per trigger, newest run
/// first, then every other thread in the order given.
pub(crate) fn build_report(rows: &[TriageRow], link: impl Fn(Uuid) -> String) -> Report {
    let listed = &rows[..rows.len().min(MAX_ENTRIES)];
    let mut counts = BTreeMap::new();
    let mut triggers: BTreeMap<(String, String), Vec<(bool, ReportEntry)>> = BTreeMap::new();
    let mut others = Vec::new();
    for row in listed {
        let verdict = classify(&row.facts);
        *counts.entry(verdict.action.as_str()).or_insert(0) += 1;
        let entry = ReportEntry {
            thread_id: row.facts.thread_id,
            title: row.title.clone(),
            link: link(row.facts.thread_id),
            action: verdict.action,
            reason: verdict.reason,
            last_activity: format!("{} ago", idle_words(row.facts.idle_secs)),
        };
        match &row.facts.trigger {
            Some(run) => triggers
                .entry((run.trigger_name.clone(), run.trigger_id.clone()))
                .or_default()
                .push((run.is_newest, entry)),
            None => others.push(entry),
        }
    }
    let mut groups: Vec<ReportGroup> = triggers
        .into_iter()
        .map(|((name, id), mut runs)| {
            runs.sort_by_key(|(is_newest, _)| !is_newest);
            ReportGroup {
                group: format!("trigger runs: {name}"),
                trigger_id: Some(id),
                entries: runs.into_iter().map(|(_, e)| e).collect(),
            }
        })
        .collect();
    if !others.is_empty() {
        groups.push(ReportGroup {
            group: "other threads".to_string(),
            trigger_id: None,
            entries: others,
        });
    }
    Report {
        counts,
        groups,
        not_listed: rows.len() - listed.len(),
    }
}

/// One entry the agent asked `apply_triage` to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ApplyRequest {
    pub(crate) thread_id: Uuid,
    pub(crate) action: TriageAction,
}

/// An entry `apply_triage` did not run, and why.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct Refused {
    pub(crate) thread_id: Uuid,
    pub(crate) action: TriageAction,
    pub(crate) reason: String,
}

/// Which requests may run. `proposal` is what the user saw. `fresh` is a new
/// read of the requested ids, so a thread that changed since is re-judged.
pub(crate) fn plan_apply(
    requests: &[ApplyRequest],
    proposal: &[TriageProposalEntry],
    fresh: &[TriageRow],
) -> (Vec<ApplyRequest>, Vec<Refused>) {
    let proposed: HashSet<Uuid> = proposal.iter().map(|e| e.thread_id).collect();
    let mut seen = HashSet::new();
    let mut run = Vec::new();
    let mut refused = Vec::new();
    for request in requests {
        let reason = if !seen.insert(request.thread_id) {
            Some("listed twice; only the first entry runs".to_string())
        } else if !proposed.contains(&request.thread_id) {
            Some("not in the triage the user saw".to_string())
        } else {
            let row = fresh
                .iter()
                .find(|r| r.facts.thread_id == request.thread_id);
            refuse_fresh(request.action, row)
        };
        match reason {
            None => run.push(*request),
            Some(reason) => refused.push(Refused {
                thread_id: request.thread_id,
                action: request.action,
                reason,
            }),
        }
    }
    (run, refused)
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
