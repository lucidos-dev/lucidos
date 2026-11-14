use super::*;

fn idle_thread() -> TriageFacts {
    TriageFacts {
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
        idle_secs: 3 * 86_400,
        trigger: None,
    }
}

fn run_of(name: &str, is_newest: bool) -> Option<TriggerRun> {
    Some(TriggerRun {
        trigger_id: format!("{name}-id"),
        trigger_name: name.to_string(),
        is_newest,
    })
}

/// Each way a thread can need the user, applied to an otherwise idle thread.
fn each_need() -> Vec<(NeedFact, TriageFacts)> {
    let with = |f: fn(&mut TriageFacts)| {
        let mut facts = idle_thread();
        f(&mut facts);
        facts
    };
    vec![
        (NeedFact::Question, with(|f| f.has_pending_question = true)),
        (
            NeedFact::PendingChange,
            with(|f| f.has_pending_change = true),
        ),
        (
            NeedFact::UnproposedWork,
            with(|f| f.has_unproposed_work = true),
        ),
        (NeedFact::Draft, with(|f| f.has_draft = true)),
        (
            NeedFact::FailedRun,
            with(|f| f.status = ThreadStatus::Failed),
        ),
    ]
}

#[test]
fn an_idle_thread_with_output_and_nothing_pending_is_archived() {
    let v = classify(&idle_thread());
    assert_eq!(v.action, TriageAction::Archive);
    assert_eq!(v.reason, "idle for 3 days, nothing pending");
}

/// The safety rule: every need fact lands under follow-up, with its words.
#[test]
fn every_need_fact_is_a_follow_up_naming_the_fact() {
    for (fact, facts) in each_need() {
        let v = classify(&facts);
        assert_eq!(v.action, TriageAction::FollowUp, "{fact:?}");
        assert!(v.reason.contains(fact.words()), "{fact:?}: {}", v.reason);
    }
}

#[test]
fn a_sub_threads_need_makes_its_root_a_follow_up() {
    let mut facts = idle_thread();
    facts.sub_thread_count = 2;
    facts.sub_thread_needs = vec![NeedFact::PendingChange];
    let v = classify(&facts);
    assert_eq!(v.action, TriageAction::FollowUp);
    assert!(v.reason.starts_with("a sub-thread holds"), "{}", v.reason);
}

/// A need outranks the pin, the trigger grouping and the no-output rule. So
/// nothing that needs the user is kept quietly or proposed for removal.
#[test]
fn a_need_outranks_every_put_away_rule() {
    for (fact, mut facts) in each_need() {
        facts.is_pinned = true;
        facts.has_output = false;
        facts.trigger = run_of("Daily digest", true);
        let action = classify(&facts).action;
        assert_eq!(action, TriageAction::FollowUp, "{fact:?}");
    }
}

#[test]
fn busy_threads_are_kept_even_when_they_need_the_user() {
    let busy: [fn(&mut TriageFacts); 4] = [
        |f| f.status = ThreadStatus::Running,
        |f| f.status = ThreadStatus::Paused,
        |f| f.live_event_waits = 1,
        |f| f.sub_thread_busy = true,
    ];
    for make_busy in busy {
        let mut facts = idle_thread();
        facts.has_draft = true;
        make_busy(&mut facts);
        assert_eq!(classify(&facts).action, TriageAction::Keep);
        assert!(refuse_apply(TriageAction::Archive, &facts).is_some());
    }
}

#[test]
fn a_pinned_thread_with_nothing_pending_is_kept() {
    let mut facts = idle_thread();
    facts.is_pinned = true;
    let v = classify(&facts);
    assert_eq!(
        (v.action, v.reason.as_str()),
        (TriageAction::Keep, "pinned")
    );
}

#[test]
fn a_thread_that_never_produced_anything_is_a_delete_candidate() {
    let mut facts = idle_thread();
    facts.has_output = false;
    assert_eq!(classify(&facts).action, TriageAction::Delete);

    // Children or a change are output of their own, so it is archived instead.
    let mut parent = facts.clone();
    parent.sub_thread_count = 1;
    assert_eq!(classify(&parent).action, TriageAction::Archive);
    let mut proposer = facts;
    proposer.ever_proposed_change = true;
    assert_eq!(classify(&proposer).action, TriageAction::Archive);
}

/// Trigger runs group per trigger: the newest is kept, older ones go.
#[test]
fn the_newest_trigger_run_is_kept_and_older_runs_are_archived() {
    let mut newest = idle_thread();
    newest.trigger = run_of("Watch the inbox", true);
    let v = classify(&newest);
    assert_eq!(v.action, TriageAction::Keep);
    assert_eq!(v.reason, "newest run of Watch the inbox");

    let mut older = idle_thread();
    older.trigger = run_of("Watch the inbox", false);
    older.has_output = false;
    let v = classify(&older);
    assert_eq!(
        v.action,
        TriageAction::Archive,
        "an empty older run is archived"
    );
    assert!(v.reason.starts_with("older run of Watch the inbox"));
}

/// An older run whose only open item is its own question is proposed for
/// dismissal. Anything more stays a follow-up, and the newest run's question
/// always does.
#[test]
fn an_older_runs_lone_question_is_proposed_for_dismissal() {
    let mut older = idle_thread();
    older.trigger = run_of("Watch the inbox", false);
    older.has_pending_question = true;
    assert_eq!(classify(&older).action, TriageAction::DismissQuestion);

    let mut with_draft = older.clone();
    with_draft.has_draft = true;
    assert_eq!(classify(&with_draft).action, TriageAction::FollowUp);

    let mut with_sub_need = older.clone();
    with_sub_need.sub_thread_needs = vec![NeedFact::Question];
    assert_eq!(classify(&with_sub_need).action, TriageAction::FollowUp);

    let mut newest = older;
    newest.trigger = run_of("Watch the inbox", true);
    assert_eq!(classify(&newest).action, TriageAction::FollowUp);
}

/// Apply re-checks fresh facts: a thread that needs the user is never
/// archived, whatever the proposal said.
#[test]
fn archive_is_refused_for_any_thread_that_needs_the_user() {
    for (fact, facts) in each_need() {
        let refusal = refuse_apply(TriageAction::Archive, &facts);
        assert!(refusal.is_some(), "{fact:?} must refuse archive");
    }
    let mut sub = idle_thread();
    sub.sub_thread_needs = vec![NeedFact::Question];
    assert!(refuse_apply(TriageAction::Archive, &sub).is_some());
    assert_eq!(refuse_apply(TriageAction::Archive, &idle_thread()), None);
}

#[test]
fn archive_is_refused_for_a_pinned_thread() {
    let mut facts = idle_thread();
    facts.is_pinned = true;
    assert!(refuse_apply(TriageAction::Archive, &facts).is_some());
}

#[test]
fn delete_is_never_applied_by_triage() {
    let mut facts = idle_thread();
    facts.has_output = false;
    let refusal = refuse_apply(TriageAction::Delete, &facts).expect("delete is owner-only");
    assert!(refusal.contains("only the user can delete"));
}

#[test]
fn dismiss_needs_a_pending_question_and_pin_always_applies() {
    let facts = idle_thread();
    assert!(refuse_apply(TriageAction::DismissQuestion, &facts).is_some());
    let mut asking = facts.clone();
    asking.has_pending_question = true;
    assert_eq!(refuse_apply(TriageAction::DismissQuestion, &asking), None);
    for (_, needing) in each_need() {
        assert_eq!(refuse_apply(TriageAction::Pin, &needing), None);
    }
}

#[test]
fn listing_actions_apply_nothing() {
    for action in [TriageAction::FollowUp, TriageAction::Keep] {
        assert!(refuse_apply(action, &idle_thread()).is_some());
    }
}

#[test]
fn actions_round_trip_through_their_wire_names() {
    for action in TriageAction::ALL {
        assert_eq!(TriageAction::parse(action.as_str()), Some(action));
        assert_eq!(
            serde_json::to_value(action).unwrap(),
            serde_json::json!(action.as_str())
        );
    }
    assert_eq!(TriageAction::parse("archive_all"), None);
}

#[test]
fn idle_words_use_the_largest_whole_unit() {
    assert_eq!(idle_words(5), "1 minute");
    assert_eq!(idle_words(7_200), "2 hours");
    assert_eq!(idle_words(86_400), "1 day");
}
