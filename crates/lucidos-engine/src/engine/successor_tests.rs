use super::*;

#[test]
fn the_decisions_flip_strictly_above_each_threshold() {
    let at = ThreadContext::of_tokens;
    assert_eq!(
        at(None),
        ThreadContext {
            input_tokens: None,
            can_hand_off: true,
            offer: false,
            applied: false,
        }
    );
    let offer_edge = at(Some(HANDOFF_OFFER_TOKENS));
    assert!(!offer_edge.offer);
    assert!(at(Some(HANDOFF_OFFER_TOKENS + 1)).offer);
}

/// I12, I14: a successor takes its old thread's place, never a detached
/// child. A child successor keeps the parent and wake mode, so the parent
/// waits on it and wakes as it would have for the old thread.
#[test]
fn a_successor_takes_its_old_threads_place() {
    use crate::engine::thread_events::WakeMode;
    let top = Placement::of_old_thread(None, Some("wake"), Some(Uuid::new_v4()));
    assert_eq!(
        top,
        Placement {
            parent_thread_id: None,
            wake_mode: None,
            spawning_event_id: None
        }
    );

    let parent = Uuid::new_v4();
    let spawned_by = Uuid::new_v4();
    let child =
        Placement::of_old_thread(Some(parent), Some("wake-when-all-done"), Some(spawned_by));
    assert_eq!(child.parent_thread_id, Some(parent));
    assert_eq!(child.wake_mode, Some(WakeMode::WakeWhenAllDone));
    assert_eq!(child.spawning_event_id, Some(spawned_by));
}

/// I6: a build chain's builder is a `no-wake` child of the planner, so its
/// clean report is filed and opens no planner turn. A failure still wakes it.
#[test]
fn a_build_chain_member_files_a_clean_report() {
    use crate::engine::event_bus::Delivery;
    use crate::engine::thread_events::{ChildCompletionStatus, WakeMode};
    let planner = Uuid::new_v4();
    let placement = Placement::build_chain_under(planner);
    assert_eq!(placement.parent_thread_id, Some(planner));
    assert_eq!(placement.wake_mode, Some(WakeMode::NoWake));
    let report = |status| Delivery::for_report(status, placement.wake_mode, None, false);
    assert_eq!(report(ChildCompletionStatus::Success), Delivery::File);
    assert_eq!(report(ChildCompletionStatus::Failure), Delivery::Wake);
}

fn implementer(agent: CodingAgent, model: Option<&str>, effort: Option<&str>) -> Implementer {
    Implementer {
        coding_agent: agent,
        model: model.map(str::to_string),
        reasoning_effort: effort.map(str::to_string),
    }
}

fn asking(
    agent: Option<CodingAgent>,
    model: Option<&str>,
    effort: Option<&str>,
) -> ImplementerRequest {
    ImplementerRequest {
        coding_agent: agent,
        coding_agent_model: model.map(str::to_string),
        reasoning_effort: effort.map(str::to_string),
    }
}

/// Invariant 1: a handoff that names no implementer carries the old one as
/// is, unvalidated, exactly as before the request could name one.
#[test]
fn a_handoff_naming_no_implementer_carries_the_old_one() {
    let old = implementer(
        CodingAgent::ClaudeCode,
        Some("claude-opus-5-5"),
        Some("max"),
    );
    assert_eq!(
        resolve_implementer(&old, &ImplementerRequest::default()),
        Ok(old)
    );
}

/// Invariant 2: what the handoff names is what the successor runs on.
#[test]
fn the_named_implementer_reaches_the_successor() {
    let old = implementer(
        CodingAgent::ClaudeCode,
        Some("claude-opus-5-5"),
        Some("high"),
    );
    assert_eq!(
        resolve_implementer(&old, &asking(None, Some("sonnet"), Some("medium"))),
        Ok(implementer(
            CodingAgent::ClaudeCode,
            Some("sonnet"),
            Some("medium")
        ))
    );
    assert_eq!(
        resolve_implementer(
            &old,
            &asking(Some(CodingAgent::Codex), Some("gpt-5.6-sol"), Some("max"))
        ),
        Ok(implementer(
            CodingAgent::Codex,
            Some("gpt-5.6-sol"),
            Some("max")
        ))
    );
}

/// A new backend starts from its own defaults: the old model and effort
/// belong to the old backend.
#[test]
fn a_new_backend_carries_neither_model_nor_effort() {
    let old = implementer(
        CodingAgent::ClaudeCode,
        Some("claude-opus-5-5"),
        Some("high"),
    );
    assert_eq!(
        resolve_implementer(&old, &asking(Some(CodingAgent::Codex), None, None)),
        Ok(implementer(CodingAgent::Codex, None, None))
    );
}

/// Invariant 3: an implementer the backend does not offer is refused, with
/// the offered list, rather than swapped for a default.
#[test]
fn an_implementer_the_backend_lacks_is_refused() {
    let old = implementer(CodingAgent::ClaudeCode, None, None);
    for (request, needle) in [
        (asking(None, Some("no-such-model"), None), "no-such-model"),
        (
            asking(Some(CodingAgent::Codex), Some("sonnet"), None),
            "not offered by codex",
        ),
        (
            asking(Some(CodingAgent::Codex), Some("gpt-5.4"), Some("max")),
            "runs only on",
        ),
    ] {
        let refusal = resolve_implementer(&old, &request).unwrap_err();
        assert!(refusal.contains(needle), "{request:?}: {refusal}");
    }
}

/// Invariant 4: a carried effort survives a model change only while the new
/// model accepts it.
#[test]
fn a_carried_effort_never_outlives_its_model() {
    let old = implementer(CodingAgent::Codex, Some("gpt-5.6-sol"), Some("max"));
    assert_eq!(
        resolve_implementer(&old, &asking(None, Some("gpt-5.4"), None)),
        Ok(implementer(CodingAgent::Codex, Some("gpt-5.4"), None))
    );
    assert_eq!(
        resolve_implementer(&old, &asking(None, Some("gpt-5.6-terra"), None)),
        Ok(implementer(
            CodingAgent::Codex,
            Some("gpt-5.6-terra"),
            Some("max")
        ))
    );
}

/// A rung names a bare model, so a pick of the old thread's model keeps the
/// old thread's exact id. A `[1m]` variant is part of the prompt cache's key,
/// so dropping it starts the successor cold.
#[test]
fn a_pick_of_the_same_model_keeps_its_context_variant() {
    for (old_model, pick) in [
        ("claude-opus-5-5[1m]", "claude-opus-5-5"),
        ("claude-sonnet-5-5[1m]", "claude-sonnet-5-5"),
    ] {
        let old = implementer(CodingAgent::ClaudeCode, Some(old_model), Some("high"));
        assert_eq!(
            resolve_implementer(&old, &asking(None, Some(pick), Some("medium"))),
            Ok(implementer(
                CodingAgent::ClaudeCode,
                Some(old_model),
                Some("medium")
            )),
            "{pick} from {old_model}"
        );
        assert_eq!(
            resolve_implementer(&old, &asking(None, Some(pick), None)),
            Ok(old),
            "a model-only pick of {old_model} carries its effort too"
        );
    }
}

/// The old thread's variant never rescues a pick the backend does not offer.
#[test]
fn an_unoffered_spelling_of_the_same_model_is_still_refused() {
    let old = implementer(
        CodingAgent::ClaudeCode,
        Some("claude-opus-5-5[1m]"),
        Some("high"),
    );
    let refusal = resolve_implementer(&old, &asking(None, Some("claude-opus-5-5@default"), None))
        .unwrap_err();
    assert!(refusal.contains("not offered"), "{refusal}");
}

/// A pick that names a variant, or another model, runs as written.
#[test]
fn a_pick_naming_a_variant_or_another_model_runs_as_written() {
    let bare = implementer(
        CodingAgent::ClaudeCode,
        Some("claude-opus-5-5"),
        Some("high"),
    );
    assert_eq!(
        resolve_implementer(&bare, &asking(None, Some("claude-opus-5-5[1m]"), None)),
        Ok(implementer(
            CodingAgent::ClaudeCode,
            Some("claude-opus-5-5[1m]"),
            Some("high")
        ))
    );
    let wide = implementer(
        CodingAgent::ClaudeCode,
        Some("claude-opus-5-5[1m]"),
        Some("high"),
    );
    assert_eq!(
        resolve_implementer(&wide, &asking(None, Some("claude-sonnet-5-5"), None)),
        Ok(implementer(
            CodingAgent::ClaudeCode,
            Some("claude-sonnet-5-5"),
            Some("high")
        ))
    );
}

const FULL_NARRATIVE: &str = "\
# Handoff

## Outcomes so far
- Phase 2 shipped.

## Decisions (beyond the plan)
- None.

## Gotchas
- None.

## State
- Committed.

## Next step
Phase 3.

## Files
- docs/plans/x.md
";

#[test]
fn a_full_narrative_lacks_nothing() {
    assert!(missing_narrative_sections(FULL_NARRATIVE).is_empty());
}

#[test]
fn each_missing_section_is_named() {
    for section in NARRATIVE_SECTIONS {
        let without: String = FULL_NARRATIVE
            .lines()
            .filter(|line| {
                !line
                    .trim_start_matches('#')
                    .trim()
                    .to_lowercase()
                    .starts_with(&section.to_lowercase())
                    || !line.starts_with('#')
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            missing_narrative_sections(&without),
            vec![section],
            "{section}"
        );
    }
}

#[test]
fn a_bare_one_liner_lacks_every_section() {
    assert_eq!(
        missing_narrative_sections("Carry on with Phase 3."),
        NARRATIVE_SECTIONS.to_vec()
    );
}

mod db {
    use super::super::*;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
    use crate::test_support::{
        message_received, seed_chat_thread, seed_coding_thread, seed_consented_handoff,
        seed_handoff, setup_test_db, start_cc_session, teardown_test_db,
    };

    fn cc_meta() -> EventMeta {
        EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        }
    }

    async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: cc_meta(),
        })
        .await
        .expect("emit");
    }

    async fn coding_thread(bus: &EventBus, parent: Option<Uuid>) -> Uuid {
        seed_coding_thread(bus, parent).await
    }

    async fn hand_off(bus: &EventBus, old: Uuid, parent: Option<Uuid>) -> Uuid {
        seed_handoff(bus, old, parent, false).await
    }

    /// I16, I17: one event links both threads, durably, and a successor
    /// shares its old thread's family root, so it runs in the same trial arm.
    #[tokio::test]
    async fn a_successor_links_both_ways_and_shares_the_family_root() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let parent = seed_chat_thread(&bus, "parent").await;
        let child = coding_thread(&bus, Some(parent)).await;
        let child_successor = hand_off(&bus, child, Some(parent)).await;
        let top = coding_thread(&bus, None).await;
        let top_successor = hand_off(&bus, top, None).await;

        assert_eq!(
            successor_thread_of(&pool, child).await.unwrap(),
            Some(child_successor)
        );
        assert_eq!(
            predecessor_of(&pool, child_successor).await.unwrap(),
            Some(child)
        );
        assert_eq!(
            carried_base(&pool, child_successor)
                .await
                .unwrap()
                .as_deref(),
            Some("0123abc")
        );
        assert!(handed_off(&pool, child).await);
        assert!(!handed_off(&pool, child_successor).await);
        let root = crate::engine::thread_family::family_root;
        assert_eq!(root(&pool, child_successor).await.unwrap(), parent);
        assert_eq!(root(&pool, top_successor).await.unwrap(), top);
        assert_eq!(carried_base(&pool, top).await.unwrap(), None);
        start_cc_session(&bus, child_successor, "claude-code/successor", None).await;
        assert_eq!(
            carried_base(&pool, child_successor).await.unwrap(),
            None,
            "only the first branch starts at the handoff, so a discarded handoff stays gone"
        );

        teardown_test_db(&db).await;
    }

    /// I14: a child that handed off owes its parent nothing, and new work on
    /// it re-arms no report. Its successor reports in its place.
    #[tokio::test]
    async fn a_child_that_handed_off_owes_its_parent_no_report() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let parent = seed_chat_thread(&bus, "parent").await;
        let child = coding_thread(&bus, Some(parent)).await;
        let successor = hand_off(&bus, child, Some(parent)).await;
        let pending = |id: Uuid| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, bool>(
                    "SELECT parent_callback_pending FROM thread_summaries WHERE thread_id = $1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        assert!(!pending(child).await);
        assert!(pending(successor).await, "the successor reports instead");
        emit(&bus, child, message_received("one more thing")).await;
        assert!(
            !pending(child).await,
            "new work on the old thread re-arms nothing"
        );
        assert_eq!(
            crate::engine::turn_end::wake_decision::WakeDuty::of_thread(&pool, child).await,
            crate::engine::turn_end::wake_decision::WakeDuty::NoParent
        );

        teardown_test_db(&db).await;
    }

    /// I15: the old thread's turn ends propose nothing: its work rides on the
    /// successor's branch. The hold answers before any git or marker read.
    #[tokio::test]
    async fn the_old_threads_work_is_held_as_carried() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let old = coding_thread(&bus, None).await;
        hand_off(&bus, old, None).await;
        let hold = crate::engine::change_ops::read_proposal_hold(
            &pool,
            old,
            std::path::Path::new("/nonexistent"),
            "claude-code/x",
            &["src/main.rs".to_string()],
        )
        .await;
        assert_eq!(
            hold,
            Some(crate::engine::change_ops::ProposalHold::CarriedBySuccessor)
        );

        teardown_test_db(&db).await;
    }

    async fn capture(pool: &sqlx::PgPool, thread_id: Uuid, payload: serde_json::Value) {
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
             VALUES (gen_random_uuid(), 'ContextCaptured', $1, $2, 'thread', $2::text)",
        )
        .bind(payload)
        .bind(thread_id)
        .execute(pool)
        .await
        .unwrap();
    }

    /// I9: the size is the latest capture of the thread's own turn. A
    /// sub-agent's call and an auxiliary call on the thread do not count.
    #[tokio::test]
    async fn the_context_size_is_the_latest_turn_capture() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        assert_eq!(
            thread_context(&pool, thread).await.unwrap(),
            ThreadContext::of_tokens(None)
        );
        capture(
            &pool,
            thread,
            serde_json::json!({"usage": {"input_tokens": 200_000, "output_tokens": 1}}),
        )
        .await;
        capture(
            &pool,
            thread,
            serde_json::json!({"usage": {"input_tokens": 900_000}, "parent_tool_use_id": "toolu_1"}),
        )
        .await;
        capture(
            &pool,
            thread,
            serde_json::json!({"usage": {"input_tokens": 950_000}, "purpose": "title"}),
        )
        .await;
        capture(
            &pool,
            thread,
            serde_json::json!({"usage": {"output_tokens": 5}}),
        )
        .await;
        let context = thread_context(&pool, thread).await.unwrap();
        assert_eq!(
            context.input_tokens,
            Some(200_000),
            "a capture with no input count leaves the last size"
        );
        assert!(context.offer);

        teardown_test_db(&db).await;
    }

    /// A thread that already handed off is offered no second handoff at any
    /// size, and the reading says whether the thread has an applied change.
    #[tokio::test]
    async fn the_reading_offers_nothing_after_a_handoff_and_reports_an_apply() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let old = coding_thread(&bus, None).await;
        let large = HANDOFF_OFFER_TOKENS + 1;
        capture(
            &pool,
            old,
            serde_json::json!({"usage": {"input_tokens": large}}),
        )
        .await;
        let before = thread_context(&pool, old).await.unwrap();
        assert!(before.can_hand_off && before.offer && !before.applied);
        sqlx::query(
            "INSERT INTO changes (request_id, thread_id, branch_name, repo_root, status) \
             VALUES (gen_random_uuid(), $1, 'claude-code/x', '/repo', 'applied')",
        )
        .bind(old)
        .execute(&pool)
        .await
        .unwrap();
        assert!(thread_context(&pool, old).await.unwrap().applied);
        hand_off(&bus, old, None).await;
        let after = thread_context(&pool, old).await.unwrap();
        assert_eq!(after.input_tokens, Some(large));
        assert!(!after.can_hand_off && !after.offer);

        teardown_test_db(&db).await;
    }

    /// I21: the old thread asks to be archived once, on its successor's
    /// first session. The archive request then waits until nothing blocks
    /// it, so a running or pinned old thread stays open.
    #[tokio::test]
    async fn the_old_thread_asks_to_archive_on_the_first_session_only() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let old = coding_thread(&bus, None).await;
        let successor = hand_off(&bus, old, None).await;
        let requests = || {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM events \
                     WHERE thread_id = $1 AND event_type = 'ThreadArchiveRequested'",
                )
                .bind(old)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        request_predecessor_archive(&bus, &pool, successor).await;
        assert_eq!(requests().await, 0, "no session yet, no request");
        start_cc_session(&bus, successor, "claude-code/s", None).await;
        request_predecessor_archive(&bus, &pool, successor).await;
        assert_eq!(requests().await, 1);
        start_cc_session(&bus, successor, "claude-code/s", None).await;
        request_predecessor_archive(&bus, &pool, successor).await;
        assert_eq!(requests().await, 1, "a resume asks nothing twice");
        request_predecessor_archive(&bus, &pool, old).await;
        assert_eq!(
            requests().await,
            1,
            "a thread that is no successor asks nothing"
        );

        teardown_test_db(&db).await;
    }

    /// A build chain is the builder under its planner plus each successor
    /// beside it. A successor in an ordinary child's place is in no chain.
    #[tokio::test]
    async fn a_build_chain_follows_successors_under_one_planner() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let planner = coding_thread(&bus, None).await;
        let builder = seed_handoff(&bus, planner, Some(planner), true).await;
        let next = hand_off(&bus, builder, Some(planner)).await;
        assert!(starts_build_chain(&pool, builder).await.unwrap());
        assert!(!starts_build_chain(&pool, next).await.unwrap());
        let chain = build_chain(&pool, next)
            .await
            .unwrap()
            .expect("a chain member");
        assert_eq!(chain.planner, planner);
        assert_eq!(chain.members, vec![builder, next]);
        assert_eq!(build_chain(&pool, planner).await.unwrap(), None);

        let parent = seed_chat_thread(&bus, "parent").await;
        let child = coding_thread(&bus, Some(parent)).await;
        let in_place = hand_off(&bus, child, Some(parent)).await;
        assert_eq!(build_chain(&pool, in_place).await.unwrap(), None);
        assert!(!starts_build_chain(&pool, in_place).await.unwrap());

        teardown_test_db(&db).await;
    }

    /// ADR 0507, I2 and I4: a successor carries the consent its predecessor
    /// recorded, read back from the events table, toward that shared parent
    /// only. A non-successor child and a no-consent handoff carry none.
    #[tokio::test]
    async fn a_successor_carries_its_predecessors_recorded_consent() {
        use crate::engine::thread_events::HandoffConsent;
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let parent = seed_chat_thread(&bus, "parent").await;
        let first = coding_thread(&bus, Some(parent)).await;
        let second = seed_consented_handoff(
            &bus,
            first,
            Some(parent),
            false,
            Some(HandoffConsent::OwnerApproval),
        )
        .await;
        let third = seed_consented_handoff(
            &bus,
            second,
            Some(parent),
            false,
            Some(HandoffConsent::Carried),
        )
        .await;

        assert_eq!(
            carried_consent(&pool, second, parent).await.unwrap(),
            Some(HandoffConsent::OwnerApproval)
        );
        assert_eq!(
            carried_consent(&pool, third, parent).await.unwrap(),
            Some(HandoffConsent::Carried),
            "the consent lasts down the lineage"
        );
        let elsewhere = seed_chat_thread(&bus, "elsewhere").await;
        assert_eq!(
            carried_consent(&pool, second, elsewhere).await.unwrap(),
            None
        );
        assert_eq!(
            carried_consent(&pool, first, parent).await.unwrap(),
            None,
            "a child that is no successor carries nothing"
        );

        let other = coding_thread(&bus, Some(parent)).await;
        let unconsented = hand_off(&bus, other, Some(parent)).await;
        assert_eq!(
            carried_consent(&pool, unconsented, parent).await.unwrap(),
            None
        );

        teardown_test_db(&db).await;
    }

    /// I5: the planner stays open when its builder starts. A later member's
    /// predecessor archives as any old thread does.
    #[tokio::test]
    async fn the_planner_stays_open_and_chain_members_archive() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let planner = coding_thread(&bus, None).await;
        let builder = seed_handoff(&bus, planner, Some(planner), true).await;
        let next = hand_off(&bus, builder, Some(planner)).await;
        let requests = |id: Uuid| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM events \
                     WHERE thread_id = $1 AND event_type = 'ThreadArchiveRequested'",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        start_cc_session(&bus, builder, "claude-code/b", None).await;
        request_predecessor_archive(&bus, &pool, builder).await;
        assert_eq!(requests(planner).await, 0, "the planner stays open");
        start_cc_session(&bus, next, "claude-code/n", None).await;
        request_predecessor_archive(&bus, &pool, next).await;
        assert_eq!(requests(builder).await, 1);

        teardown_test_db(&db).await;
    }

    /// The narrative's sections are written once here. The CLI's `--help`
    /// is a string the CLI crate cannot import, so this pins it.
    #[test]
    fn the_cli_help_names_every_narrative_section() {
        let help = include_str!("../../../lucidos-cli/src/successor.rs");
        for section in NARRATIVE_SECTIONS {
            assert!(
                help.contains(section),
                "`lucidos successor --help` lacks {section}"
            );
        }
    }

    /// The successor's fresh branch gets a copy of the old branch's plan
    /// marker, and a marker it already holds is kept.
    #[tokio::test]
    async fn the_plan_marker_carries_to_the_successor_branch() {
        let (pool, db) = setup_test_db().await;
        let repo = std::path::Path::new("/repo");
        let marker = |branch: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, String>(
                    "SELECT state FROM planned_branches WHERE branch_name = $1",
                )
                .bind(branch)
                .fetch_optional(&pool)
                .await
                .unwrap()
            }
        };
        sqlx::query(
            "INSERT INTO planned_branches (repo_root, branch_name, state, plan_path, head_sha, planned_at) \
             VALUES ($1, 'claude-code/old', 'planned', 'docs/plans/p.md', 'abc', NOW()), \
                    ($1, 'claude-code/kept', 'acknowledged_simple', NULL, 'abc', NOW())",
        )
        .bind(crate::engine::git_ops::canonical_repo_root(repo))
        .execute(&pool)
        .await
        .unwrap();
        let carry = crate::engine::git_ops::carry_planned;
        carry(&pool, repo, "claude-code/old", "claude-code/new")
            .await
            .unwrap();
        carry(&pool, repo, "claude-code/old", "claude-code/kept")
            .await
            .unwrap();
        assert_eq!(marker("claude-code/new").await.as_deref(), Some("planned"));
        assert_eq!(
            marker("claude-code/kept").await.as_deref(),
            Some("acknowledged_simple")
        );

        teardown_test_db(&db).await;
    }

    /// A compaction after the last capture is the size from then on: its
    /// `post_tokens`, or unknown when Claude Code did not report one.
    #[tokio::test]
    async fn a_later_compaction_replaces_the_size() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        let compacted = |payload: serde_json::Value| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
                     VALUES (gen_random_uuid(), 'CodingAgentCompacted', $1, $2, 'thread', $2::text)",
                )
                .bind(payload)
                .bind(thread)
                .execute(&pool)
                .await
                .unwrap();
            }
        };
        let large = HANDOFF_OFFER_TOKENS + 1;
        capture(
            &pool,
            thread,
            serde_json::json!({"usage": {"input_tokens": large}}),
        )
        .await;
        compacted(serde_json::json!({"trigger": "auto", "post_tokens": 40_000})).await;
        let after = thread_context(&pool, thread).await.unwrap();
        assert_eq!(after.input_tokens, Some(40_000));
        assert!(!after.offer);
        compacted(serde_json::json!({"trigger": "manual"})).await;
        assert_eq!(
            thread_context(&pool, thread).await.unwrap().input_tokens,
            None
        );

        teardown_test_db(&db).await;
    }

    /// A thread hands off once. A racing second handoff rolls its whole emit
    /// back, so it leaves no second link and no second event.
    #[tokio::test]
    async fn a_second_handoff_is_refused_and_leaves_no_trace() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let old = coding_thread(&bus, None).await;
        let first = hand_off(&bus, old, None).await;
        let second = bus
            .emit(BusEvent::Thread {
                thread_id: old,
                event: ThreadEvent::SuccessorThreadSpawned {
                    successor_thread_id: Uuid::new_v4(),
                    base_sha: "0123abc".into(),
                    starts_build_chain: false,
                    handoff_consent: None,
                },
                meta: cc_meta(),
            })
            .await;
        let Err(refusal) = second else {
            panic!("a second handoff must be refused");
        };
        assert!(refusal.to_string().contains(ALREADY_HANDED_OFF));
        assert_eq!(successor_thread_of(&pool, old).await.unwrap(), Some(first));
        let spawned: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = 'SuccessorThreadSpawned'",
        )
        .bind(old)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(spawned, 1);

        teardown_test_db(&db).await;
    }

    /// I3, I4: the request recorded at the tap is pending until the thread's
    /// turn ends, and carries the implementer the user picked.
    #[tokio::test]
    async fn a_request_is_pending_until_the_turn_ends() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let old = coding_thread(&bus, None).await;
        assert_eq!(pending_successor_request(&pool, old).await.unwrap(), None);

        let insert = |event_type: &'static str, payload: serde_json::Value| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
                     VALUES (gen_random_uuid(), $1, $2, $3, 'thread', $3::text)",
                )
                .bind(event_type)
                .bind(payload)
                .bind(old)
                .execute(&pool)
                .await
                .unwrap();
            }
        };
        insert(
            "SuccessorRequested",
            serde_json::json!({
                "message": "Add a toggle",
                "coding_agent_model": "claude-sonnet-5-5",
            }),
        )
        .await;
        assert_eq!(
            pending_successor_request(&pool, old).await.unwrap(),
            Some(PendingRequest {
                message: Some("Add a toggle".into()),
                implementer: ImplementerRequest {
                    coding_agent: None,
                    coding_agent_model: Some("claude-sonnet-5-5".into()),
                    reasoning_effort: None,
                },
            })
        );

        insert("CodingAgentIdled", serde_json::json!({})).await;
        assert_eq!(
            pending_successor_request(&pool, old).await.unwrap(),
            None,
            "the turn ended, so the request is closed"
        );

        teardown_test_db(&db).await;
    }
}

#[test]
fn the_first_handoff_numbers_the_successor_two() {
    assert_eq!(
        successor_title("Fix the drawer", None),
        "Fix the drawer (2)"
    );
}

#[test]
fn a_repeat_handoff_counts_up_and_never_stacks() {
    let second = successor_title("Fix the drawer", None);
    let third = successor_title(&second, None);
    assert_eq!(third, "Fix the drawer (3)");
    assert_eq!(successor_title(&third, None), "Fix the drawer (4)");
}

#[test]
fn legacy_continued_titles_migrate_to_their_chain_position() {
    assert_eq!(
        successor_title("Fix the drawer (continued)", None),
        "Fix the drawer (3)"
    );
    assert_eq!(
        successor_title("Fix the drawer (continued) (continued)", None),
        "Fix the drawer (4)"
    );
}

#[test]
fn a_checkpoint_names_the_work_the_successor_starts() {
    assert_eq!(
        successor_title("Fix the drawer", Some("Harden")),
        "Fix the drawer (2) · Harden"
    );
}

#[test]
fn a_checkpoint_suffix_does_not_survive_the_next_handoff() {
    assert_eq!(
        successor_title("Fix the drawer (2) · Harden", None),
        "Fix the drawer (3)"
    );
    assert_eq!(
        successor_title("Fix the drawer (3) · Phase 2", Some("Harden")),
        "Fix the drawer (4) · Harden"
    );
    assert_eq!(
        successor_title("Fix the drawer (continued) · Harden", Some("Build")),
        "Fix the drawer (3) · Build"
    );
}

#[test]
fn a_dot_in_a_plain_title_is_part_of_the_title() {
    assert_eq!(
        successor_title("Fix · the drawer", None),
        "Fix · the drawer (2)"
    );
}

#[test]
fn only_a_small_parenthesised_number_counts_as_a_position() {
    assert_eq!(successor_title("Report (1)", None), "Report (1) (2)");
    assert_eq!(successor_title("Report (2024)", None), "Report (2024) (2)");
}

#[test]
fn a_title_that_is_only_a_marker_keeps_its_text() {
    assert_eq!(successor_title("(continued)", None), "(continued) (2)");
}

#[test]
fn the_brief_names_the_old_thread_and_how_to_rebuild() {
    let id = Uuid::new_v4();
    let brief = handoff_brief("[Old](thread:ws/x)", id, None, Some("Fixed the drawer"));
    assert!(brief.contains("continues [Old](thread:ws/x)"));
    assert!(brief.contains(&format!("lucidos events query --thread-id {id}")));
    assert!(brief.contains("git log"));
    assert!(brief.contains("docs/plans/"));
    assert!(brief.contains("The change it applied last: Fixed the drawer"));
}

/// I3: a typed request is quoted in the brief, and with none the successor
/// is told to ask where to pick up.
#[test]
fn the_brief_carries_a_typed_request_or_asks_for_one() {
    let id = Uuid::new_v4();
    let with = handoff_brief(
        "[Old](thread:ws/x)",
        id,
        Some("Add a dark-mode toggle"),
        None,
    );
    assert!(with.contains("Add a dark-mode toggle"));
    assert!(!with.contains("AskUserQuestion"));
    let without = handoff_brief("[Old](thread:ws/x)", id, None, None);
    assert!(without.contains("end your first turn on an AskUserQuestion card"));
}
