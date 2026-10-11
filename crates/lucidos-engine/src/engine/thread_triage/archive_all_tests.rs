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
        family_root: facts.thread_id,
        family_pinned: false,
        facts,
        title: "A thread".into(),
        section: "inbox".into(),
        inbox_sub_threads: 0,
        pinned_sub_threads: 0,
    }
}

/// A family of `open` inbox sub-threads, `pinned` of them pinned.
fn family(open: usize, pinned: usize, f: impl FnOnce(&mut TriageFacts)) -> TriageRow {
    let mut row = row(f);
    row.facts.sub_thread_count = open as i64;
    row.inbox_sub_threads = open;
    row.pinned_sub_threads = pinned;
    row
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
    assert_eq!(p.safe_thread_count, 2);
    assert_eq!(p.kept_thread_count, 5);
    assert_eq!(p.kept.get("question"), Some(&2));
    assert_eq!(p.kept.get("pending_change"), Some(&1));
    assert_eq!(p.kept.get("draft"), Some(&1));
    assert_eq!(p.kept.get("busy"), Some(&1));
    assert_eq!(p.kept.values().sum::<usize>(), p.kept_thread_count);
    let pinned = rows[7].facts.thread_id;
    assert!(p.safe.iter().all(|s| s.thread_id != pinned));
}

/// The confirm counts inbox threads as the Current badge does, so what goes and
/// what stays add up to it. A kept family counts once under its reason and
/// its other threads beside it. A pinned sub-thread stays open.
#[test]
fn the_preflight_counts_every_inbox_thread_in_a_family() {
    let rows = vec![
        family(3, 0, |_| {}),
        family(2, 1, |_| {}),
        family(4, 0, |f| f.status = ThreadStatus::Running),
        family(1, 0, |f| f.sub_thread_needs = vec![NeedFact::Question]),
        family(5, 0, |f| f.is_pinned = true),
    ];
    let p = preflight(&rows);
    assert_eq!(p.safe.len(), 2, "the press sends family roots");
    assert_eq!(
        p.safe_thread_count,
        4 + 2,
        "each root and its unpinned subs"
    );
    assert_eq!(p.kept.get("busy"), Some(&1));
    assert_eq!(p.kept.get("question"), Some(&1));
    assert_eq!(p.kept.get(SAME_FAMILY), Some(&(4 + 1)));
    assert_eq!(p.kept.get(PINNED_SUB_THREAD), Some(&1));
    assert_eq!(p.kept_thread_count, 8);
    let current_badge = 4 + 3 + 5 + 2;
    assert_eq!(p.safe_thread_count + p.kept_thread_count, current_badge);
}

/// Archive all follows the drawer's families. A root under a pinned family is
/// in Pinned, so it is neither archived nor counted. A pinned root in a family
/// Current shows renders in Current and stays open, so it counts as kept.
#[test]
fn the_preflight_follows_the_drawers_families() {
    let top = Uuid::new_v4();
    let in_family = |f: fn(&mut TriageFacts), pinned_family: bool| {
        let mut row = family(1, 0, f);
        row.family_root = top;
        row.family_pinned = pinned_family;
        row.facts.is_pinned |= pinned_family;
        row
    };
    let under_pinned = in_family(|_| {}, true);
    let p = preflight(std::slice::from_ref(&under_pinned));
    assert!(
        p.safe.is_empty(),
        "a pinned family's threads stay in Pinned"
    );
    assert_eq!(p.kept_thread_count, 0);

    let pinned = in_family(|f| f.is_pinned = true, false);
    let plain = in_family(|_| {}, false);
    let p = preflight(&[pinned.clone(), plain]);
    assert_eq!(p.safe_thread_count, 2);
    assert_eq!(p.kept.get(PINNED_SUB_THREAD), Some(&1));
    assert_eq!(
        p.kept.get(SAME_FAMILY),
        Some(&1),
        "the pinned root's own sub"
    );
    assert_eq!(p.kept_thread_count, 2);

    let p = preflight(std::slice::from_ref(&pinned));
    assert_eq!(p.kept_thread_count, 0, "alone, its family sits in Pinned");
}

/// A kept entry carries how many inbox threads stayed with it, so the result
/// toast counts threads like the confirm did.
#[test]
fn a_kept_entry_counts_the_threads_that_stayed() {
    let asking = family(2, 0, |f| f.has_pending_question = true);
    let gone = Uuid::new_v4();
    let (_, kept) = still_safe(
        std::slice::from_ref(&asking),
        &[asking.facts.thread_id, gone],
    );
    let count = |id| {
        kept.iter()
            .find(|k| k.thread_id == id)
            .unwrap()
            .thread_count
    };
    assert_eq!(count(asking.facts.thread_id), 3);
    assert_eq!(count(gone), 1);
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

/// Each id that changed since the confirm carries a slug the client words as
/// the confirm does, naming the refusal that fired.
#[test]
fn a_changed_id_carries_the_slug_of_its_refusal() {
    let now_asking = row(|f| f.has_pending_question = true);
    let now_running = row(|f| f.status = ThreadStatus::Running);
    let now_pinned = row(|f| f.is_pinned = true);
    let mut archived = row(|_| {});
    archived.section = "archived".into();
    let gone = Uuid::new_v4();
    let rows = vec![
        now_asking.clone(),
        now_running.clone(),
        now_pinned.clone(),
        archived.clone(),
    ];
    let confirmed = [
        now_asking.facts.thread_id,
        now_running.facts.thread_id,
        now_pinned.facts.thread_id,
        archived.facts.thread_id,
        gone,
    ];

    let (_, kept) = still_safe(&rows, &confirmed);
    let slugs: Vec<&str> = kept.iter().map(|k| k.slug).collect();
    assert_eq!(slugs, ["question", "busy", "pinned", "archived", "gone"]);
}

/// A family the cascade refused is counted by its blocker, a sub-thread's
/// included; anything else it refuses is still working.
#[test]
fn a_refused_cascade_is_counted_by_its_blocker() {
    use crate::engine::thread_lifecycle::Blocker;
    let slug = |b: Blocker| refusal_kept_slug(&serde_json::json!({ "blocker": b.as_str() }));
    assert_eq!(slug(Blocker::DescendantQuestion), "question");
    assert_eq!(slug(Blocker::PendingChange), "pending_change");
    assert_eq!(slug(Blocker::DescendantRunning), "busy");
    let reason = |r: &str| refusal_kept_slug(&serde_json::json!({ "reason": r }));
    assert_eq!(reason("thread_not_found"), "gone");
    assert_eq!(
        reason(crate::api::threads::archive::THREAD_PINNED),
        "pinned"
    );
    assert_eq!(reason("apply_in_progress"), "busy");
}
