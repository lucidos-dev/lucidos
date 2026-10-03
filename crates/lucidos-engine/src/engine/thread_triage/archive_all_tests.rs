use super::*;
use crate::engine::thread_lifecycle::ThreadStatus;
use crate::engine::thread_triage::TriageFacts;

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
        idle_secs: 60,
        trigger: None,
    };
    f(&mut facts);
    TriageRow {
        facts,
        title: "A thread".into(),
        section: "inbox".into(),
    }
}

/// The confirm archives the safe rows, counts the rest once each by their
/// first reason, and ignores pinned rows: they live in Pinned, not Current.
#[test]
fn the_preflight_counts_what_stays_and_skips_pinned_rows() {
    let rows = vec![
        row(|_| {}),
        row(|f| f.has_output = false),
        row(|f| f.has_pending_question = true),
        row(|f| {
            f.has_pending_question = true;
            f.has_draft = true;
        }),
        row(|f| f.sub_thread_needs = vec![NeedFact::PendingChange]),
        row(|f| f.has_draft = true),
        row(|f| f.status = ThreadStatus::Running),
        row(|f| f.is_pinned = true),
    ];
    let p = preflight(&rows);
    assert_eq!(p.safe.len(), 2, "idle and empty threads are both safe");
    assert_eq!(p.kept_count, 5);
    assert_eq!(p.kept.get("question"), Some(&2));
    assert_eq!(p.kept.get("pending_change"), Some(&1));
    assert_eq!(p.kept.get("draft"), Some(&1));
    assert_eq!(p.kept.get("busy"), Some(&1));
    assert_eq!(p.kept.values().sum::<usize>(), p.kept_count);
    let pinned = rows[7].facts.thread_id;
    assert!(p.safe.iter().all(|s| s.thread_id != pinned));
}

/// The confirm is the ceiling: an id is archived only if the user confirmed
/// it and it is still safe now.
#[test]
fn only_confirmed_ids_that_are_still_safe_are_archived() {
    let safe = row(|_| {});
    let now_asking = row(|f| f.has_pending_question = true);
    let mut archived = row(|_| {});
    archived.section = "archived".into();
    let unconfirmed = row(|_| {});
    let gone = Uuid::new_v4();
    let rows = vec![
        safe.clone(),
        now_asking.clone(),
        archived.clone(),
        unconfirmed.clone(),
    ];
    let confirmed = [
        safe.facts.thread_id,
        now_asking.facts.thread_id,
        archived.facts.thread_id,
        gone,
    ];

    let (to_archive, kept) = still_safe(&rows, &confirmed);
    assert_eq!(to_archive, vec![safe.facts.thread_id]);
    assert!(!to_archive.contains(&unconfirmed.facts.thread_id));
    let reason = |id: Uuid| {
        kept.iter()
            .find(|k| k.thread_id == id)
            .map(|k| k.reason.clone())
            .expect("kept with a reason")
    };
    assert!(reason(now_asking.facts.thread_id).contains("an unanswered question"));
    assert_eq!(reason(archived.facts.thread_id), "it is already archived");
    assert_eq!(reason(gone), "it no longer exists");
}
