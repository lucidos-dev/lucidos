use super::*;
use crate::engine::thread_lifecycle::ThreadStatus;
use crate::engine::thread_triage::{TriageFacts, TriggerRun};

fn row(f: impl FnOnce(&mut TriageFacts)) -> TriageRow {
    let mut facts = TriageFacts {
        thread_id: Uuid::new_v4(),
        status: ThreadStatus::Idle,
        is_pinned: false,
        has_pending_question: false,
        has_pending_change: false,
        has_unproposed_work: false,
        has_draft: false,
        has_output: true,
        ever_proposed_change: false,
        sub_thread_count: 0,
        live_event_waits: 0,
        sub_thread_needs: Vec::new(),
        sub_thread_busy: false,
        idle_secs: 7_200,
        trigger: None,
    };
    f(&mut facts);
    TriageRow {
        facts,
        title: "Title".into(),
        section: "inbox".into(),
    }
}

fn run(name: &str, is_newest: bool) -> Option<TriggerRun> {
    Some(TriggerRun {
        trigger_id: format!("{name}-id"),
        trigger_name: name.into(),
        is_newest,
    })
}

fn link(id: Uuid) -> String {
    format!("[thread](thread:ws/{id})")
}

/// Trigger runs form one group per trigger with the newest run first. Every
/// other thread lands in one closing group, and the counts add up.
#[test]
fn the_report_groups_trigger_runs_per_trigger_newest_first() {
    let older = row(|f| f.trigger = run("Watch", false));
    let newest = row(|f| f.trigger = run("Watch", true));
    let other = row(|_| {});
    let report = build_report(&[older.clone(), other.clone(), newest.clone()], link);

    assert_eq!(report.groups.len(), 2);
    let watch = &report.groups[0];
    assert_eq!(watch.group, "trigger runs: Watch");
    assert_eq!(watch.entries[0].thread_id, newest.facts.thread_id);
    assert_eq!(watch.entries[0].action, TriageAction::Keep);
    assert_eq!(watch.entries[1].action, TriageAction::Archive);
    assert_eq!(report.groups[1].group, "other threads");
    assert_eq!(report.groups[1].entries[0].last_activity, "2 hours ago");
    assert_eq!(report.counts.values().sum::<usize>(), 3);
    assert_eq!(report.proposal_entries().len(), 3);
}

#[test]
fn a_report_lists_at_most_the_cap_and_says_how_many_wait() {
    let rows: Vec<TriageRow> = (0..MAX_ENTRIES + 4).map(|_| row(|_| {})).collect();
    let report = build_report(&rows, link);
    assert_eq!(report.proposal_entries().len(), MAX_ENTRIES);
    assert_eq!(report.not_listed, 4);
}

fn proposal_of(rows: &[&TriageRow]) -> Vec<TriageProposalEntry> {
    rows.iter()
        .map(|r| TriageProposalEntry {
            thread_id: r.facts.thread_id,
            action: "archive".into(),
            reason: "idle".into(),
        })
        .collect()
}

fn request(row: &TriageRow, action: TriageAction) -> ApplyRequest {
    ApplyRequest {
        thread_id: row.facts.thread_id,
        action,
    }
}

/// Apply runs only proposed threads, once each, and re-judges each against
/// its fresh facts. A thread that gained a question since is not archived.
#[test]
fn apply_runs_only_proposed_threads_that_are_still_safe() {
    let idle = row(|_| {});
    let now_asking = row(|f| f.has_pending_question = true);
    let unproposed = row(|_| {});
    let mut archived = row(|_| {});
    archived.section = "archived".into();
    let proposal = proposal_of(&[&idle, &now_asking, &archived]);
    let fresh = vec![
        idle.clone(),
        now_asking.clone(),
        unproposed.clone(),
        archived.clone(),
    ];
    let requests = [
        request(&idle, TriageAction::Archive),
        request(&idle, TriageAction::Pin),
        request(&now_asking, TriageAction::Archive),
        request(&unproposed, TriageAction::Archive),
        request(&archived, TriageAction::Pin),
    ];

    let (run, refused) = plan_apply(&requests, &proposal, &fresh);
    assert_eq!(run, vec![request(&idle, TriageAction::Archive)]);
    let reason = |i: usize| refused[i].reason.as_str();
    assert_eq!(reason(0), "listed twice; only the first entry runs");
    assert!(
        reason(1).contains("an unanswered question"),
        "{}",
        reason(1)
    );
    assert_eq!(reason(2), "not in the triage the user saw");
    assert_eq!(reason(3), "it is already archived");
}

#[test]
fn apply_refuses_delete_and_runs_a_pin_or_a_dismissal() {
    let empty = row(|f| f.has_output = false);
    let asking = row(|f| f.has_pending_question = true);
    let pinned_candidate = row(|_| {});
    let proposal = proposal_of(&[&empty, &asking, &pinned_candidate]);
    let fresh = vec![empty.clone(), asking.clone(), pinned_candidate.clone()];
    let requests = [
        request(&empty, TriageAction::Delete),
        request(&asking, TriageAction::DismissQuestion),
        request(&pinned_candidate, TriageAction::Pin),
    ];

    let (run, refused) = plan_apply(&requests, &proposal, &fresh);
    assert_eq!(run, requests[1..].to_vec());
    assert_eq!(refused.len(), 1);
    assert!(refused[0].reason.contains("only the user can delete"));
}
