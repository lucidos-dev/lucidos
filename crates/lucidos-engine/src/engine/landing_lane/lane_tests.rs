//! The lane against the events table: what is queued, what recovery rebuilds,
//! and which apply path takes the lane.

use super::driver::load_lane_records;
use super::recovery::{fold, LaneRecord};
use super::state::Place;
use super::*;
use crate::engine::agent_session::CodingAgentKind;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent, UnqueueReason};
use crate::test_support::{setup_test_db, start_cc_session, teardown_test_db};

fn kind(kind: CodingAgentKind) -> ApplyKindContext {
    ApplyKindContext { kind, app_id: None }
}

/// Only Lucidos source in the engine's own repository takes the lane. An app
/// change keeps the Apply tiers, and so does any other repository.
#[test]
fn only_lucidos_source_in_the_engine_repo_takes_the_lane() {
    let engine_repo = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let lane = engine_repo.path();
    assert!(takes_landing_lane(
        &kind(CodingAgentKind::Lucidos),
        lane,
        lane
    ));
    assert!(!takes_landing_lane(&kind(CodingAgentKind::App), lane, lane));
    assert!(!takes_landing_lane(
        &kind(CodingAgentKind::External),
        lane,
        lane
    ));
    assert!(!takes_landing_lane(
        &kind(CodingAgentKind::Lucidos),
        other.path(),
        lane
    ));
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("emit");
}

async fn propose(bus: &EventBus, thread_id: Uuid, change_id: Uuid, branch: &str) {
    emit(
        bus,
        thread_id,
        ThreadEvent::ChangeProposed {
            change_id: change_id.to_string(),
            description: Some("work".into()),
            files: vec!["a.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: branch.into(),
            repo_root: "/repo".into(),
            hardened: true,
            incomplete: false,
            set_aside: false,
            path: String::new(),
            diff: String::new(),
        },
    )
    .await;
}

fn queued_event(change_id: Uuid, snapshot: &str) -> ThreadEvent {
    ThreadEvent::ChangeQueued {
        change_id: change_id.to_string(),
        snapshot_sha: snapshot.into(),
    }
}

fn applied(change_id: Uuid, landing: Option<&str>) -> ThreadEvent {
    ThreadEvent::ChangeApplied {
        change_id: change_id.to_string(),
        requires_restart: false,
        client_update: false,
        commits: vec![],
        thread_title: None,
        actor: None,
        pre_merge_sha: landing.map(|_| "pre".to_string()),
        post_merge_sha: landing.map(str::to_string),
        landing_merge_sha: landing.map(str::to_string),
        path: String::new(),
    }
}

/// A change is queued from its `ChangeQueued` until it lands or leaves.
#[tokio::test]
async fn a_change_is_queued_until_it_lands_or_leaves() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    start_cc_session(&bus, thread, "work", None).await;
    let change = Uuid::new_v4();
    propose(&bus, thread, change, "work").await;
    assert_eq!(queued_snapshot(&pool, thread, change).await.unwrap(), None);

    emit(&bus, thread, queued_event(change, "snap1")).await;
    assert_eq!(
        queued_snapshot(&pool, thread, change).await.unwrap(),
        Some("snap1".into())
    );

    emit(
        &bus,
        thread,
        ThreadEvent::ChangeUnqueued {
            change_id: change.to_string(),
            reason: UnqueueReason::BatchFailed,
        },
    )
    .await;
    assert_eq!(queued_snapshot(&pool, thread, change).await.unwrap(), None);

    emit(&bus, thread, queued_event(change, "snap2")).await;
    assert_eq!(
        queued_changes(&pool, thread).await.unwrap(),
        vec![LaneMember {
            change_id: change,
            snapshot_sha: "snap2".into()
        }]
    );
    emit(&bus, thread, applied(change, Some("landing"))).await;
    assert!(queued_changes(&pool, thread).await.unwrap().is_empty());
    assert_eq!(
        landing_merge_of(&pool, thread, change).await.unwrap(),
        Some("landing".into())
    );

    teardown_test_db(&db).await;
}

/// Apply All reads lane membership from events, for any thread: a queued
/// pending change is held, and one that was unqueued, landed or never queued
/// is not.
#[tokio::test]
async fn lane_held_changes_names_the_queued_pending_changes_only() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (thread_a, thread_b) = (Uuid::new_v4(), Uuid::new_v4());
    start_cc_session(&bus, thread_a, "work-a", None).await;
    start_cc_session(&bus, thread_b, "work-b", None).await;
    let (queued, unqueued, landed, idle) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    for (thread, change, branch) in [
        (thread_a, queued, "work-a"),
        (thread_a, unqueued, "work-a2"),
        (thread_b, landed, "work-b"),
        (thread_b, idle, "work-b2"),
    ] {
        propose(&bus, thread, change, branch).await;
    }
    for (thread, change) in [(thread_a, queued), (thread_a, unqueued), (thread_b, landed)] {
        emit(&bus, thread, queued_event(change, "snap")).await;
    }
    emit(
        &bus,
        thread_a,
        ThreadEvent::ChangeUnqueued {
            change_id: unqueued.to_string(),
            reason: UnqueueReason::Removed,
        },
    )
    .await;
    emit(&bus, thread_b, applied(landed, Some("landing"))).await;

    let held = lane_held_changes(&pool, &[queued, unqueued, landed, idle])
        .await
        .unwrap();
    assert_eq!(held, std::collections::HashSet::from([queued]));
    assert!(lane_held_changes(&pool, &[]).await.unwrap().is_empty());

    teardown_test_db(&db).await;
}

/// Remove takes a waiting change out with `ChangeUnqueued { removed }`. The
/// change is queued nowhere after it, and recovery does not rebuild it.
#[tokio::test]
async fn a_removed_change_leaves_the_queue_and_is_not_rebuilt() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    start_cc_session(&bus, thread, "work", None).await;
    let change = Uuid::new_v4();
    propose(&bus, thread, change, "work").await;
    emit(&bus, thread, queued_event(change, "snap1")).await;
    emit(
        &bus,
        thread,
        ThreadEvent::ChangeUnqueued {
            change_id: change.to_string(),
            reason: UnqueueReason::Removed,
        },
    )
    .await;

    assert!(queued_changes(&pool, thread).await.unwrap().is_empty());
    let lane = fold(&load_lane_records(&pool).await.unwrap(), |_| true);
    assert_eq!(lane.place(change), Place::Absent);
    let reason: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'reason' FROM events \
         WHERE thread_id = $1 AND event_type = 'ChangeUnqueued'",
    )
    .bind(thread)
    .fetch_one(&pool)
    .await
    .unwrap();
    // Pins the wire value `thread-event-wire.ts` carries.
    assert_eq!(reason.as_deref(), Some("removed"));
    teardown_test_db(&db).await;
}

/// An app change, and every apply before the lane, recorded no landing merge.
#[tokio::test]
async fn an_apply_without_a_landing_merge_reverts_as_before() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    start_cc_session(&bus, thread, "work", None).await;
    let change = Uuid::new_v4();
    propose(&bus, thread, change, "work").await;
    emit(&bus, thread, applied(change, None)).await;
    assert_eq!(landing_merge_of(&pool, thread, change).await.unwrap(), None);
    teardown_test_db(&db).await;
}

/// Recovery reads the lane back from the events the lane wrote: the queue,
/// the resolving batch and its members, in order.
#[tokio::test]
async fn recovery_rebuilds_the_lane_from_stored_events() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let [landed, resolving, waiting] = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    for (change, branch) in [(landed, "b1"), (resolving, "b2"), (waiting, "b3")] {
        start_cc_session(&bus, change, branch, None).await;
        propose(&bus, change, change, branch).await;
    }
    emit(&bus, landed, queued_event(landed, "s1")).await;
    emit(&bus, landed, applied(landed, Some("m1"))).await;
    emit(&bus, resolving, queued_event(resolving, "s2")).await;
    let batch_id = Uuid::new_v4();
    let lane_thread_id = Uuid::new_v4();
    let member = LaneMember {
        change_id: resolving,
        snapshot_sha: "s2".into(),
    };
    bus.emit(BusEvent::System(SystemEvent::LaneBatchStarted {
        batch_id,
        members: vec![member.clone()],
        base_sha: "base".into(),
        lane_thread_id,
    }))
    .await
    .expect("batch started");
    emit(&bus, waiting, queued_event(waiting, "s3")).await;

    let records = load_lane_records(&pool).await.unwrap();
    assert!(records.contains(&LaneRecord::BatchStarted {
        batch_id,
        base_sha: "base".into(),
        lane_thread_id,
        members: vec![member],
    }));
    let lane = fold(&records, |id| id != landed);
    assert_eq!(lane.place(landed), Place::Absent);
    assert_eq!(lane.place(resolving), Place::Resolving);
    assert_eq!(lane.place(waiting), Place::Waiting);

    bus.emit(BusEvent::System(SystemEvent::LaneBatchFailed {
        batch_id,
        reason: "aborted".into(),
        actor: None,
    }))
    .await
    .expect("batch failed");
    let lane = fold(&load_lane_records(&pool).await.unwrap(), |id| id != landed);
    assert!(lane.resolving().is_none(), "a failed batch stays closed");
    assert_eq!(lane.place(resolving), Place::Absent, "and requeues nothing");

    teardown_test_db(&db).await;
}

/// Only the lane moves `main` for Lucidos source. This test names each
/// production caller of a primitive that moves `main`, with its reason. A new
/// caller fails it until someone decides it may.
#[test]
fn only_the_lane_and_the_app_path_move_main() {
    use crate::test_support::source_scan::production_sources;
    const PRIMITIVES: &[&str] = &[
        "ff_main_to_locked(",
        "ff_main_to_branch(",
        "catchup_and_ff_to_main(",
        "ff_merge_to_main(",
        "move_local_main(",
        "follow_origin_main(",
    ];
    // (file, why it may move `main`)
    const ALLOWED: &[(&str, &str)] = &[
        ("engine/git_ops/merge.rs", "the primitives themselves"),
        ("engine/landing_lane/landing.rs", "the landing lane"),
        ("engine/landing_lane/integration.rs", "the landing lane"),
        (
            "engine/follow_main.rs",
            "following origin/main, a fast-forward only (ADR 0589)",
        ),
        ("engine/change_ops/apply.rs", "the app path's Tiers 1 to 3"),
        (
            "engine/agent_session/apply_now.rs",
            "the app path's live-session Apply Now and Tier-1 recovery",
        ),
        (
            "engine/agent_session/run_session/completion.rs",
            "the app path's Tier-2 and Tier-3 resolution completion",
        ),
    ];
    let callers: Vec<String> = production_sources()
        .into_iter()
        .filter(|(_, source)| PRIMITIVES.iter().any(|p| calls(source, p)))
        .map(|(rel, _)| rel)
        .collect();
    let mut allowed: Vec<String> = ALLOWED.iter().map(|(f, _)| f.to_string()).collect();
    allowed.sort();
    assert_eq!(callers, allowed, "a new caller moves main outside the lane");
}

/// A call, not a definition or a mention in a comment.
fn calls(source: &str, primitive: &str) -> bool {
    source.lines().any(|line| {
        let code = line.split("//").next().unwrap_or("");
        code.contains(primitive) && !code.contains(&format!("fn {primitive}"))
    })
}

async fn emit_system(bus: &EventBus, event: SystemEvent) {
    bus.emit(BusEvent::System(event)).await.expect("emit");
}

fn started(batch_id: Uuid, lane_thread_id: Uuid, members: Vec<LaneMember>) -> SystemEvent {
    SystemEvent::LaneBatchStarted {
        batch_id,
        members,
        base_sha: "base".into(),
        lane_thread_id,
    }
}

/// A failed batch, by a lane-thread abort or a Cancel, writes one
/// `LaneBatchFailed` and a `ChangeUnqueued { batch_failed }` per member.
/// After them every member is pending, apply-ready, and queued nowhere: the
/// lane rebuilds no batch and no queue from them.
#[tokio::test]
async fn a_failed_batch_leaves_every_member_pending_apply_ready_and_unqueued() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    for actor in [
        None,
        Some(crate::engine::thread_events::MessageOrigin::Device {
            device_id: "test-device".into(),
        }),
    ] {
        let members: Vec<(Uuid, Uuid)> = (0..2).map(|_| (Uuid::new_v4(), Uuid::new_v4())).collect();
        for (n, (thread, change)) in members.iter().enumerate() {
            let branch = format!("lucidos-b{n}-{thread}");
            start_cc_session(&bus, *thread, &branch, None).await;
            propose(&bus, *thread, *change, &branch).await;
            emit(&bus, *thread, queued_event(*change, &format!("snap{n}"))).await;
        }
        let batch_id = Uuid::new_v4();
        let lane_members = members
            .iter()
            .enumerate()
            .map(|(n, (_, change))| LaneMember {
                change_id: *change,
                snapshot_sha: format!("snap{n}"),
            })
            .collect();
        emit_system(&bus, started(batch_id, Uuid::new_v4(), lane_members)).await;
        emit_system(
            &bus,
            SystemEvent::LaneBatchFailed {
                batch_id,
                reason: "The lane thread was stopped.".into(),
                actor: actor.clone(),
            },
        )
        .await;
        for (thread, change) in &members {
            emit(
                &bus,
                *thread,
                ThreadEvent::ChangeUnqueued {
                    change_id: change.to_string(),
                    reason: UnqueueReason::BatchFailed,
                },
            )
            .await;
            sqlx::query("UPDATE thread_summaries SET status = 'idle' WHERE thread_id = $1")
                .bind(thread)
                .execute(&pool)
                .await
                .unwrap();
        }

        let lane = fold(&load_lane_records(&pool).await.unwrap(), |_| true);
        assert!(
            lane.resolving_batch(batch_id).is_none(),
            "the batch stays closed"
        );
        for (thread, change) in &members {
            assert!(queued_changes(&pool, *thread).await.unwrap().is_empty());
            assert_eq!(lane.place(*change), Place::Absent, "nothing re-queues it");
            let row = crate::core::changes_projection::ChangesProjection::new(pool.clone())
                .get_by_id(*change)
                .await
                .unwrap()
                .expect("the change row");
            assert!(row.is_pending(), "still pending");
            let ready = crate::core::changes::keep_apply_ready_changes(&pool, vec![row])
                .await
                .unwrap();
            assert_eq!(ready.len(), 1, "still apply-ready");
        }
        let stored: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT payload->'data'->'actor' FROM events \
             WHERE event_type = 'LaneBatchFailed' AND aggregate_id = $1",
        )
        .bind(batch_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            stored.filter(|v| !v.is_null()).is_some(),
            actor.is_some(),
            "a Cancel names who canceled"
        );
    }
    teardown_test_db(&db).await;
}

/// The body of the first `fn {name}(` in `source`, braces included, with its
/// whole-line comments dropped.
fn fn_body(source: &str, name: &str) -> String {
    let start = source
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("no fn {name}"));
    let open = start + source[start..].find('{').expect("a body");
    let mut depth = 0usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return source[open..=open + offset]
                        .lines()
                        .filter(|line| !line.trim_start().starts_with("//"))
                        .collect::<Vec<_>>()
                        .join("\n");
                }
            }
            _ => {}
        }
    }
    panic!("fn {name} never closes");
}

fn production(rel: &str) -> String {
    use crate::test_support::source_scan::{read_production_source, src_root};
    read_production_source(&src_root().join(rel))
}

/// The lane holds each resolving member's apply claim until the batch lands
/// or fails. Apply, Discard and Set aside each take the claim before anything
/// else, so the lane's claim refuses each with its reason before it acts.
///
/// No in-process harness builds a whole engine: its repo root is the source
/// checkout, so a test engine would land on the real `main`. So this pins the
/// order in each surface's source, and the claim itself below.
#[test]
fn a_landing_member_refuses_apply_discard_and_set_aside() {
    use crate::engine::change_ops::apply_claim::{ApplyClaims, APPLY_IN_PROGRESS_MESSAGE};
    for (file, surface) in [
        ("engine/change_ops/apply.rs", "apply_change_inner"),
        ("engine/change_ops/discard.rs", "discard_change"),
        ("engine/change_ops/discard.rs", "discard_change_quietly"),
        ("engine/change_ops/set_aside.rs", "set_aside_change"),
    ] {
        let body: String = fn_body(&production(file), surface)
            .split_whitespace()
            .collect();
        let first_act = body
            .find("self.")
            .unwrap_or_else(|| panic!("{surface} does nothing"));
        assert!(
            body[first_act..].starts_with("self.apply_claims.try_claim(change_id)"),
            "{surface} takes the claim before anything else"
        );
        assert!(
            body.contains("APPLY_IN_PROGRESS_MESSAGE"),
            "{surface} refuses with the claim's reason"
        );
    }
    assert!(!APPLY_IN_PROGRESS_MESSAGE.is_empty());
    let claims = std::sync::Arc::new(ApplyClaims::default());
    let member = Uuid::new_v4();
    let held_by_lane = claims
        .try_claim(member)
        .expect("the lane claims at batch start");
    assert!(
        claims.try_claim(member).is_none(),
        "every other surface is refused"
    );
    drop(held_by_lane);
    assert!(
        claims.try_claim(member).is_some(),
        "free once the batch lands or fails"
    );
}

/// Every Apply surface hands a Lucidos-source change to the lane before any
/// app-path tier can move `main`, and only the lane announces a queued change.
/// So an app change, which `takes_landing_lane` refuses, keeps the old tiers
/// and never reads `ChangeQueued`.
///
/// Pinned in the source for the reason the claim test gives: no test engine
/// can run these surfaces without landing on the real `main`.
#[test]
fn every_apply_surface_queues_lucidos_source_before_any_tier() {
    use crate::test_support::source_scan::production_sources;
    const MAIN_MOVERS: &[&str] = &[
        "catchup_and_ff_to_main(",
        "ff_main_to_branch(",
        "ff_main_to_locked(",
        "ff_merge_to_main(",
    ];
    // (file, function, the call that queues, the tier calls it must precede)
    let routes: [(&str, &str, &str, &[&str]); 2] = [
        (
            "engine/change_ops/apply.rs",
            "apply_change_inner",
            "return self.queue_in_landing_lane(",
            &[
                "begin_in_place_merge(",
                "hold_thread_for_worktree_reset(",
                "apply_change_tier3(",
            ],
        ),
        (
            "engine/agent_session/apply_now.rs",
            "apply_now_inner",
            "self.apply_now_through_lane(",
            &["merge_via_cc_session("],
        ),
    ];
    for (file, surface, queue, tiers) in routes {
        let body = fn_body(&production(file), surface);
        let route = body
            .find("takes_landing_lane(")
            .unwrap_or_else(|| panic!("{surface} never asks whether the lane takes the change"));
        let queued = body
            .find(queue)
            .unwrap_or_else(|| panic!("{surface} never queues"));
        assert!(route < queued, "{surface} queues only behind the check");
        for tier in tiers {
            let at = body
                .find(tier)
                .unwrap_or_else(|| panic!("{surface} has no {tier}"));
            assert!(queued < at, "{surface} reaches {tier} before the lane");
        }
        for mover in MAIN_MOVERS {
            assert!(
                !body[..route].contains(mover),
                "{surface} moves main with {mover} before the lane check"
            );
        }
    }

    // Apply Now with no live session, and Apply All, go through `apply_change`.
    assert!(fn_body(
        &production("engine/agent_session/apply_now.rs"),
        "apply_pending_now"
    )
    .contains(".apply_change("));
    assert!(production("engine/apply_all_driver.rs").contains(".apply_change("));

    let files = |matches: &dyn Fn(&str) -> bool| -> Vec<String> {
        production_sources()
            .into_iter()
            .filter(|(_, source)| matches(source))
            .map(|(rel, _)| rel)
            .collect()
    };
    let builds_change_queued = |source: &str| {
        source.lines().any(|line| {
            let code = line.split("//").next().unwrap_or("").trim_end();
            code.ends_with("ThreadEvent::ChangeQueued {")
        })
    };
    assert_eq!(
        files(&builds_change_queued),
        vec!["engine/landing_lane/driver.rs".to_string()],
        "only the lane announces a queued change"
    );
    assert_eq!(
        files(&|source| calls(source, "queue_in_landing_lane(")),
        vec![
            "engine/agent_session/apply_now.rs".to_string(),
            "engine/change_ops/apply.rs".to_string()
        ],
        "only the two routes above reach the lane"
    );
}

/// Every arrival tries `main` in a clean pass, whatever batch resolves. Only
/// the pass sets a change aside: nothing queues one before it (ADR 0480).
#[test]
fn every_arrival_gets_a_clean_pass() {
    let arrive = fn_body(&production("engine/landing_lane/driver.rs"), "arrive");
    assert!(
        arrive.contains("self.pass_answer("),
        "the arrival runs a pass"
    );
    assert!(
        !arrive.contains("self.lane.wait("),
        "no arrival waits before its pass"
    );
}

/// Apply All opens its press in the lane before the first member applies,
/// and submits it wherever the press can start nothing more: an advance that
/// leaves it submitted, and Cancel. A press left open would hold every
/// conflict back for good (ADR 0480).
#[test]
fn apply_all_opens_its_press_first_and_always_submits_it() {
    let driver = production("engine/apply_all_driver.rs");
    let seed = fn_body(&driver, "start_apply_all_batch");
    assert!(seed.contains("self.open_lane_press(batch_id)"));
    let run = fn_body(&driver, "run_apply_all");
    let seeded = run.find(".start_apply_all_batch(").expect("seeds");
    let first = run.find(".apply_batch_member(").expect("applies");
    assert!(seeded < first, "the press opens before its first member");
    let filtered = run
        .find(".drop_changes_already_applying(")
        .expect("filters out what the lane holds");
    assert!(filtered < seeded, "the filter runs before the batch seeds");

    let advance = fn_body(&driver, "advance_apply_all_batch");
    let submitted = advance
        .find("batch.is_submitted()")
        .expect("advance asks whether the press is in");
    let told = advance
        .find("self.submit_lane_press(batch_id)")
        .expect("advance tells the lane");
    assert!(submitted < told);

    let cancel = fn_body(&driver, "cancel_apply_all_batches");
    let left = cancel.find(".cancel_lane_members(").expect("cancel");
    let told = cancel
        .find("self.submit_lane_press(")
        .expect("cancel submits the press");
    assert!(
        left < told,
        "its members leave the lane before the queue passes"
    );
}

/// A restart never hands the lane's member back to `apply_change`: recovery
/// asks the lane before it drives anything, and a laned member only waits.
/// Applying it would take the claim the restored lane needs, and fail the
/// lane batch with "being applied elsewhere".
#[test]
fn recovery_leaves_a_laned_member_to_the_lane() {
    let driver = production("engine/apply_all_driver.rs");
    let recover = fn_body(&driver, "recover_apply_all_batches");
    let asked = recover.find("lane_held_changes(").expect("asks the lane");
    let restored = recover
        .find("progress.restore_laned(")
        .expect("restores a laned member beside the queue");
    let owner = recover.find("self.recovered_owner(").expect("owners");
    let driven = recover.find("self.spawn_batch_member(").expect("drives");
    assert!(asked < restored && restored < owner && owner < driven);
}

/// A change the lane queues tells its Apply All press, whoever handed it
/// over. The auto-apply after hardening is such a caller. Without it the press
/// would wait for the member to land, and the lane would wait for the press
/// (ADR 0480).
#[test]
fn a_queued_answer_tells_apply_all_whoever_queued_the_change() {
    let submit = fn_body(
        &production("engine/landing_lane/driver.rs"),
        "submit_to_landing_lane",
    );
    let queued = submit
        .find("ApplyStatus::Queued")
        .expect("the hand-off reads a Queued answer");
    let laned = submit
        .find("ApplyAllDriveMsg::Laned")
        .expect("the hand-off tells Apply All");
    assert!(queued < laned, "it tells Apply All only of a Queued answer");

    let member = fn_body(
        &production("engine/apply_all_driver.rs"),
        "apply_batch_member",
    );
    assert!(
        !member.contains("ApplyAllDriveMsg::Laned"),
        "the hand-off reports it, once"
    );
}

/// A run on the lane thread carries its batch while the batch is open. Once
/// it closes, a run is refused until the integration branch is gone, then
/// runs as an ordinary thread.
#[tokio::test]
async fn a_run_on_the_lane_thread_carries_the_open_batch() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let repo = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let git = |args: &'static [&'static str]| {
        let dir = repo.path().to_path_buf();
        async move {
            let out = crate::engine::git_ops::git_cmd(args, &dir).await.unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    };
    git(&["init", "-b", "main"]).await;
    git(&[
        "-c",
        "user.email=t@example.com",
        "-c",
        "user.name=T",
        "commit",
        "--allow-empty",
        "-m",
        "init",
    ])
    .await;

    let lane_thread = Uuid::new_v4();
    let batch_id = Uuid::new_v4();
    // A follow-up the user typed, not a turn the lane sent.
    let user_turn = Uuid::new_v4();
    let duty_for_run =
        || lane_duty_for_run(&pool, workspace.path(), repo.path(), lane_thread, user_turn);
    assert_eq!(duty_for_run().await.unwrap(), None, "not a lane thread yet");
    emit_system(&bus, started(batch_id, lane_thread, vec![])).await;
    let duty = duty_for_run().await.unwrap().expect("the open batch");
    assert_eq!(duty.batch_id, batch_id);
    assert_eq!(duty.base_sha, "base");
    assert_eq!(
        duty.worktree.branch,
        crate::engine::git_ops::lane_branch(batch_id)
    );

    emit_system(
        &bus,
        SystemEvent::LaneBatchLanded {
            batch_id,
            pre_sha: "a".into(),
            post_sha: "b".into(),
        },
    )
    .await;
    let branch = crate::engine::git_ops::lane_branch(batch_id);
    let branch_exists = || {
        let refname = format!("refs/heads/{branch}");
        let dir = repo.path().to_path_buf();
        async move {
            crate::engine::git_ops::git_answer(
                &["rev-parse", "--verify", "--quiet", &refname],
                &dir,
            )
            .await
        }
    };
    // Another tree holds the branch, so git refuses to delete it.
    let holder = workspace.path().join("holder");
    let holder_str = holder.to_string_lossy().to_string();
    let out = crate::engine::git_ops::git_cmd(
        &["worktree", "add", "-b", &branch, &holder_str],
        repo.path(),
    )
    .await
    .unwrap();
    assert!(out.status.success());
    let refused = duty_for_run().await;
    assert!(
        refused.is_err_and(|e| e.to_string() == super::lane_thread::LANE_THREAD_ENDED),
        "no run on a closed batch's branch while it stays"
    );
    assert_eq!(
        branch_exists().await,
        crate::engine::git_ops::GitAnswer::Yes
    );

    // Each run tries the removal again, so a refusal cannot last.
    let out = crate::engine::git_ops::git_cmd(
        &["worktree", "remove", "--force", &holder_str],
        repo.path(),
    )
    .await
    .unwrap();
    assert!(out.status.success());
    assert_eq!(
        duty_for_run().await.unwrap(),
        None,
        "the run retires the branch, then runs as an ordinary thread"
    );
    assert_eq!(branch_exists().await, crate::engine::git_ops::GitAnswer::No);
    teardown_test_db(&db).await;
}

/// A run that carries a batch answers the lane however it ends. These are
/// the exits before its completion, which answers for itself.
#[test]
fn every_lane_run_exit_answers_the_lane() {
    use super::lane_thread::{unanswered_run_end, LaneThreadEnd};
    use crate::engine::claude_code::STALE_RESUME_ERROR;
    assert_eq!(
        unanswered_run_end(Ok(()), false),
        Some(LaneThreadEnd::Stopped),
        "a Stop during startup keeps the batch open"
    );
    assert!(
        matches!(
            unanswered_run_end(Err("Failed to start the coding agent: no binary"), false),
            Some(LaneThreadEnd::Failed(reason)) if reason.contains("no binary")
        ),
        "a failure to start fails the batch, and names why"
    );
    assert!(matches!(
        unanswered_run_end(
            Err("The lane worktree could not be prepared: disk full"),
            false
        ),
        Some(LaneThreadEnd::Failed(_))
    ));
    assert_eq!(
        unanswered_run_end(Err(STALE_RESUME_ERROR), false),
        None,
        "the caller's retry answers instead"
    );
    for outcome in [Ok(()), Err("anything")] {
        assert_eq!(
            unanswered_run_end(outcome, true),
            None,
            "a shutdown leaves the batch to the restart"
        );
    }
}

/// A waiting change something else resolved leaves the lane with the reason
/// its status gives, so no restart queues it again. A landed one already left.
#[test]
fn a_resolved_waiting_change_leaves_with_its_reason() {
    use super::driver::unqueue_reason_for;
    use crate::core::changes::ChangeStatus;
    assert_eq!(
        unqueue_reason_for(Some(ChangeStatus::SetAside)),
        Some(UnqueueReason::SetAside)
    );
    for gone in [
        Some(ChangeStatus::Discarded),
        Some(ChangeStatus::Withdrawn),
        None,
    ] {
        assert_eq!(unqueue_reason_for(gone), Some(UnqueueReason::Discarded));
    }
    for stays_silent in [
        ChangeStatus::Pending,
        ChangeStatus::Applied,
        ChangeStatus::Reverted,
    ] {
        assert_eq!(unqueue_reason_for(Some(stays_silent)), None);
    }
}

/// Every member a clean pass takes out of the lane settles its source
/// thread, including one already on `main` and one whose landing failed.
#[test]
fn every_member_leaving_a_pass_settles_its_source_thread() {
    use super::driver::PassOutcome;
    use super::landing::{LandingMerge, LandingStep};
    let merge = LandingMerge {
        first_parent: "p".into(),
        merge_sha: "m".into(),
    };
    let landed = LandingStep::Landed(merge.clone());
    let landed_earlier = LandingStep::AlreadyOnMain(Some(merge.clone()));
    let cases = [
        (&landed, None, Some(true)),
        (&landed, Some("update-ref failed"), Some(false)),
        (&LandingStep::AlreadyOnMain(None), None, Some(true)),
        (&landed_earlier, Some("update-ref failed"), Some(true)),
        (&LandingStep::SetAside, None, None),
    ];
    for (step, failed, settles) in cases {
        assert_eq!(
            PassOutcome::of(step, failed).settles_source(),
            settles,
            "{step:?} with {failed:?}"
        );
    }
}

/// The lane branch prefix has one definition, in `git_ops`. The lane reads
/// it rather than spelling the prefix again.
#[test]
fn the_lane_branch_prefix_is_written_once() {
    use crate::test_support::source_scan::production_sources;
    let respelled: Vec<String> = production_sources()
        .into_iter()
        .filter(|(rel, source)| {
            rel != "engine/git_ops/branch_name.rs"
                && (source.contains("\"lane/") || source.contains("refs/heads/lane/"))
        })
        .map(|(rel, _)| rel)
        .collect();
    assert!(respelled.is_empty(), "{respelled:?}");
}

/// Recovery reads where the lane thread stood. Killed at batch open, it was
/// never prompted. Killed mid-resolution, or by a shutdown boundary, its turn
/// is unfinished. Killed after a clean finish, it is finished and lands.
#[tokio::test]
async fn recovery_reads_where_the_lane_thread_stood() {
    use super::recovery::{batch_recovery, BatchRecovery, LaneThreadState};
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let state = |thread: Uuid, branch: String| {
        let pool = pool.clone();
        async move {
            super::lane_thread::lane_thread_state(&pool, thread, &branch)
                .await
                .unwrap()
        }
    };
    let cc = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    let terminal = |event: ThreadEvent, thread: Uuid| BusEvent::Thread {
        thread_id: thread,
        event,
        meta: cc.clone(),
    };

    let not_opened = Uuid::new_v4();
    let branch = crate::engine::git_ops::lane_branch(Uuid::new_v4());
    assert_eq!(state(not_opened, branch).await, LaneThreadState::NotOpened);
    assert_eq!(
        batch_recovery(LaneThreadState::NotOpened),
        BatchRecovery::OpenLaneThread
    );

    let mid = Uuid::new_v4();
    let mid_branch = crate::engine::git_ops::lane_branch(Uuid::new_v4());
    start_cc_session(&bus, mid, &mid_branch, None).await;
    assert_eq!(
        state(mid, mid_branch.clone()).await,
        LaneThreadState::Unfinished
    );
    bus.emit(terminal(
        ThreadEvent::ResponseAborted {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::EngineShutdown,
        },
        mid,
    ))
    .await
    .unwrap();
    assert_eq!(
        state(mid, mid_branch).await,
        LaneThreadState::Unfinished,
        "a shutdown boundary leaves the turn to the thread's own recovery"
    );
    assert_eq!(
        batch_recovery(LaneThreadState::Unfinished),
        BatchRecovery::AwaitLaneThread
    );

    let done = Uuid::new_v4();
    let done_branch = crate::engine::git_ops::lane_branch(Uuid::new_v4());
    start_cc_session(&bus, done, &done_branch, None).await;
    bus.emit(terminal(
        ThreadEvent::ResponseGenerated {
            text: "Resolved.".into(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
        },
        done,
    ))
    .await
    .unwrap();
    bus.emit(terminal(
        ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
        done,
    ))
    .await
    .unwrap();
    assert_eq!(state(done, done_branch).await, LaneThreadState::Finished);
    assert_eq!(
        batch_recovery(LaneThreadState::Finished),
        BatchRecovery::Land
    );
    teardown_test_db(&db).await;
}

/// The device that archived or deleted a thread, in these tests.
fn closing_device() -> crate::engine::thread_events::MessageOrigin {
    crate::engine::thread_events::MessageOrigin::Device {
        device_id: "test-device".into(),
    }
}

async fn delete_threads(bus: &EventBus, thread_ids: Vec<Uuid>) {
    emit_system(
        bus,
        SystemEvent::ThreadsDeleted {
            thread_ids,
            event_count: 0,
            memory_count: 0,
            worktrees_removed: 0,
            widgets_removed: 0,
            actor: Some(closing_device()),
        },
    )
    .await;
}

/// An archive or a delete reaches the lane on the bus, naming who closed the
/// thread, and a lag drops neither. An idle lane thread has no run whose end
/// would report it, so this feed is the only way the lane hears of it.
#[tokio::test]
async fn an_archived_or_deleted_thread_reaches_the_lane_through_a_lag() {
    use super::driver::{forward_lane_thread_closures, LaneMsg};
    use super::lane_thread::LANE_THREAD_CLOSING_EVENT_TYPES;
    use crate::engine::event_bus::{LagReplay, ReplayScope};
    use crate::test_support::{flood_past_bus_capacity, seed_chat_thread, seed_thread_event};

    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let archived = seed_chat_thread(&bus, "a lane thread").await;
    seed_thread_event(
        &bus,
        archived,
        ThreadEvent::ResponseGenerated {
            text: "resolved".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
    )
    .await;
    let deleted = Uuid::new_v4();
    let (mut rx, mut replay) = LagReplay::subscribe(
        &bus,
        ReplayScope::Workspace,
        Some(LANE_THREAD_CLOSING_EVENT_TYPES.to_vec()),
    );

    bus.emit(BusEvent::Thread {
        thread_id: archived,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::with_actor(Some(closing_device())),
    })
    .await
    .expect("archive");
    delete_threads(&bus, vec![deleted]).await;
    flood_past_bus_capacity(&bus);

    let closed = std::sync::Mutex::new(Vec::new());
    assert!(
        forward_lane_thread_closures(rx.recv().await, &mut replay, &pool, |msg| {
            if let LaneMsg::ThreadClosed { thread_id, actor } = msg {
                closed.lock().unwrap().push((thread_id, actor));
            }
        })
        .await
    );
    assert_eq!(
        closed.into_inner().unwrap(),
        vec![
            (archived, Some(closing_device())),
            (deleted, Some(closing_device()))
        ]
    );
    teardown_test_db(&db).await;
}

/// The feed's replay list gates live frames too, so an event the lane acts on
/// but the list omits would never reach it.
#[test]
fn the_lane_replays_every_event_that_closes_a_thread() {
    use super::lane_thread::{lane_thread_closures, LANE_THREAD_CLOSING_EVENT_TYPES};
    let thread = Uuid::new_v4();
    let closing = [
        BusEvent::Thread {
            thread_id: thread,
            event: ThreadEvent::ThreadArchived,
            meta: EventMeta::NONE,
        },
        BusEvent::System(SystemEvent::ThreadsDeleted {
            thread_ids: vec![thread],
            event_count: 0,
            memory_count: 0,
            worktrees_removed: 0,
            widgets_removed: 0,
            actor: None,
        }),
    ];
    for event in &closing {
        assert_eq!(lane_thread_closures(event), vec![(thread, None)]);
        let event_type = match event {
            BusEvent::Thread { event, .. } => event.event_type(),
            BusEvent::System(event) => event.event_type(),
        };
        assert!(LANE_THREAD_CLOSING_EVENT_TYPES.contains(&event_type));
    }
    assert_eq!(LANE_THREAD_CLOSING_EVENT_TYPES.len(), closing.len());
    let saved = BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::ThreadSaved,
        meta: EventMeta::NONE,
    };
    assert!(lane_thread_closures(&saved).is_empty());
}

/// A lane thread archived or deleted before the lane heard of it reads as
/// closed at the next start. The restart then fails its batch, and neither
/// opens the thread again nor waits on it.
#[tokio::test]
async fn a_restart_reads_an_archived_or_deleted_lane_thread_as_closed() {
    use super::recovery::{batch_recovery, BatchRecovery, LaneThreadState};
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let state = |thread: Uuid| {
        let pool = pool.clone();
        async move {
            let branch = crate::engine::git_ops::lane_branch(Uuid::new_v4());
            super::lane_thread::lane_thread_state(&pool, thread, &branch)
                .await
                .unwrap()
        }
    };

    let archived = crate::test_support::seed_chat_thread(&bus, "a lane thread").await;
    crate::test_support::seed_thread_event(
        &bus,
        archived,
        ThreadEvent::ResponseGenerated {
            text: "resolved".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
    )
    .await;
    assert_ne!(state(archived).await, LaneThreadState::Closed);
    crate::test_support::seed_thread_event(&bus, archived, ThreadEvent::ThreadArchived).await;
    assert_eq!(state(archived).await, LaneThreadState::Closed);

    let deleted = Uuid::new_v4();
    assert_eq!(state(deleted).await, LaneThreadState::NotOpened);
    delete_threads(&bus, vec![Uuid::new_v4(), deleted]).await;
    assert_eq!(
        state(deleted).await,
        LaneThreadState::Closed,
        "a delete leaves no event on the thread, yet it is not unopened"
    );
    assert_eq!(batch_recovery(LaneThreadState::Closed), BatchRecovery::Fail);
    teardown_test_db(&db).await;
}

/// A Stop or Discard on a stale lane thread answers the lane while the thread
/// is the lane's: its batch is open, or its last session still ran on the
/// integration branch. A former lane thread that has since run as an
/// ordinary thread on its own branch gets its ordinary teardown.
#[tokio::test]
async fn a_stale_session_goes_to_the_lane_only_while_it_is_the_lanes() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let workspace = tempfile::tempdir().unwrap();
    let route = |thread: Uuid, branch: Option<String>| {
        let pool = pool.clone();
        let workspace = workspace.path().to_path_buf();
        async move {
            lane_batch_of_stale_session(&pool, &workspace, thread, branch.as_deref())
                .await
                .unwrap()
        }
    };

    let ordinary = Uuid::new_v4();
    assert_eq!(
        route(ordinary, Some("lucidos-own".into())).await,
        None,
        "not a lane thread"
    );

    let lane_thread = Uuid::new_v4();
    let batch_id = Uuid::new_v4();
    let lane = crate::engine::git_ops::lane_branch(batch_id);
    let own = format!("lucidos-claude-code-{lane_thread}");
    emit_system(&bus, started(batch_id, lane_thread, vec![])).await;
    for branch in [Some(lane.clone()), Some(own.clone()), None] {
        assert_eq!(
            route(lane_thread, branch.clone()).await,
            Some(batch_id),
            "an open batch's lane thread is the lane's, on {branch:?}"
        );
    }

    emit_system(
        &bus,
        SystemEvent::LaneBatchFailed {
            batch_id,
            reason: "Canceled from the landing lane.".into(),
            actor: None,
        },
    )
    .await;
    assert_eq!(
        route(lane_thread, Some(lane)).await,
        Some(batch_id),
        "its last session still ran on the integration branch"
    );
    assert_eq!(
        route(lane_thread, Some(own)).await,
        None,
        "it has run as an ordinary thread since, so its own teardown runs"
    );
    assert_eq!(route(lane_thread, None).await, None);
    teardown_test_db(&db).await;
}

/// A Cancel can end a batch while the lane's first turn still waits on its
/// worktree. That turn then never runs, not even as an ordinary thread: it
/// would merge every member and propose them again. A turn the user typed
/// runs as an ordinary thread, as before.
#[tokio::test]
async fn a_turn_the_lane_sent_never_outlives_its_batch() {
    use crate::engine::thread_events::{ActorMode, MessageOrigin};
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let not_a_repo = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let lane_thread = Uuid::new_v4();
    let batch_id = Uuid::new_v4();
    emit_system(&bus, started(batch_id, lane_thread, vec![])).await;
    let mut sent = crate::test_support::message_received("Resolve the batch.");
    if let ThreadEvent::MessageReceived { origin, mode, .. } = &mut sent {
        *origin = Some(MessageOrigin::engine(super::lane_thread::lane_turn_reason()));
        *mode = ActorMode::Engine;
    }
    let lane_turn = bus
        .emit(BusEvent::Thread {
            thread_id: lane_thread,
            event: sent,
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .expect("emit")
        .expect("persisted")
        .event_id;
    let typed_turn = Uuid::new_v4();
    let duty = |request: Uuid| {
        lane_duty_for_run(
            &pool,
            workspace.path(),
            not_a_repo.path(),
            lane_thread,
            request,
        )
    };
    assert_eq!(
        duty(lane_turn).await.unwrap().map(|d| d.batch_id),
        Some(batch_id),
        "the open batch carries the lane's turn"
    );

    emit_system(
        &bus,
        SystemEvent::LaneBatchFailed {
            batch_id,
            reason: "Canceled from the landing lane.".into(),
            actor: None,
        },
    )
    .await;
    let refused = duty(lane_turn).await;
    assert!(
        refused.is_err_and(|e| e.to_string() == super::lane_thread::LANE_TURN_OUTLIVED_BATCH),
        "the lane's turn is refused once its batch ended"
    );
    assert_eq!(
        duty(typed_turn).await.unwrap(),
        None,
        "a typed turn runs as an ordinary thread"
    );
    teardown_test_db(&db).await;
}

/// A crash between `LaneBatchFailed` and a member's `ChangeUnqueued` leaves
/// the member reading as queued on its thread. The restart names it, and
/// finds the thread its `ChangeQueued` went on even without its row.
#[tokio::test]
async fn a_restart_finds_a_member_its_failed_batch_left_queued() {
    use super::recovery::LaneRestore;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change = Uuid::new_v4();
    start_cc_session(&bus, thread, "work", None).await;
    propose(&bus, thread, change, "work").await;
    emit(&bus, thread, queued_event(change, "snap1")).await;
    let batch_id = Uuid::new_v4();
    let member = LaneMember {
        change_id: change,
        snapshot_sha: "snap1".into(),
    };
    emit_system(&bus, started(batch_id, Uuid::new_v4(), vec![member])).await;
    emit_system(
        &bus,
        SystemEvent::LaneBatchFailed {
            batch_id,
            reason: "The lane thread was stopped.".into(),
            actor: None,
        },
    )
    .await;

    assert_eq!(queued_changes(&pool, thread).await.unwrap().len(), 1);
    let restore = LaneRestore::of(&load_lane_records(&pool).await.unwrap(), |_| true);
    assert_eq!(restore.stranded, vec![change]);
    assert_eq!(queued_thread(&pool, change).await.unwrap(), Some(thread));
    assert_eq!(queued_thread(&pool, Uuid::new_v4()).await.unwrap(), None);

    emit(
        &bus,
        thread,
        ThreadEvent::ChangeUnqueued {
            change_id: change.to_string(),
            reason: UnqueueReason::BatchFailed,
        },
    )
    .await;
    assert!(queued_changes(&pool, thread).await.unwrap().is_empty());
    let restore = LaneRestore::of(&load_lane_records(&pool).await.unwrap(), |_| true);
    assert!(restore.stranded.is_empty());
    teardown_test_db(&db).await;
}

/// A failed batch's member leaves with `ChangeUnqueued` whatever its row read
/// gives: a failed read once skipped it, and it read as queued for good. Only
/// the Apply All answer and the settle wait on a pending row. The restart
/// repairs a member a crash left queued the same way.
///
/// Pinned in the source: no test engine can run the driver.
#[test]
fn a_failed_batch_unqueues_every_member_before_reading_its_status() {
    let driver = production("engine/landing_lane/driver.rs");
    let body = fn_body(&driver, "unqueue_failed_member");
    let unqueued = body
        .find("self.emit_unqueued(")
        .expect("every member leaves with ChangeUnqueued");
    let gate = body.find("is_pending").expect("a pending gate");
    assert!(unqueued < gate, "the unqueue comes before the pending gate");
    for later in ["notify_apply_all(", "spawn_settle_source("] {
        let at = body.find(later).unwrap_or_else(|| panic!("no {later}"));
        assert!(gate < at, "{later} waits on a pending row");
    }
    assert!(!body.contains(".ok()"), "a failed read skips no member");
    for caller in ["fail_batch", "restore"] {
        assert!(
            fn_body(&driver, caller).contains("self.unqueue_failed_member("),
            "{caller} unqueues through it"
        );
    }
}

/// A pass that finds a waiting member's claim held keeps it queued. The
/// holder may be refused before it reaches the lane. So the lane waits on
/// that claim's release, not on a message from the holder.
#[test]
fn a_waiting_member_held_elsewhere_is_tried_again_when_its_claim_goes() {
    let driver = production("engine/landing_lane/driver.rs");
    let claim_members = fn_body(&driver, "claim_members");
    assert!(claim_members.contains(".try_claim_for_lane(member.change_id)"));
    assert!(!claim_members.contains(".try_claim(member.change_id)"));
    assert!(fn_body(&driver, "start_landing_lane_driver").contains(".on_lane_wait_release("));
    assert!(fn_body(&driver, "handle").contains("LaneMsg::ClaimReleased"));
    assert!(
        fn_body(&driver, "handle").contains("LaneMsg::OriginAnswered => self.retry_origin()"),
        "the follow task's answer from origin retries a landing that waited for it"
    );
    assert!(
        !driver.contains("tokio::time::"),
        "the lane waits on messages, never on a clock (plan non-goal: no timers)"
    );
}

/// A turn cut off at boot past a queued change keeps that change, and its
/// newer work is still re-decided. That re-decide withholds the work as
/// awaiting landing, so the thread shows why it waits.
#[test]
fn recovery_redecides_the_work_past_a_queued_change() {
    let recovery = production("engine/agent_recovery/recovery.rs");
    let body = fn_body(&recovery, "recover_orphaned_worktrees");
    let gate = body
        .find("if !queued {")
        .expect("the withdraw waits on the queue");
    let withdraw = body.find("emit_change_withdrawn(").expect("a withdraw");
    let redecide = body.find("redecide_branch_work(").expect("a re-decide");
    assert!(gate < withdraw, "only the withdraw is gated on the queue");
    let gated = &body[gate..redecide];
    assert_eq!(
        gated.matches('{').count(),
        gated.matches('}').count(),
        "the re-decide sits outside the queue gate"
    );
}
