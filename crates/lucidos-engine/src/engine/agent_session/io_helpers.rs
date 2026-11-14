use crate::engine::AgentUserInput;

/// Drain any follow-up messages that were queued in `msg_rx` but never consumed
/// (e.g. because the CC process exited before reading them). Returns them so
/// callers can re-process them with a fresh Claude Code session instead of showing
/// a scary "interrupted" banner.
pub(super) fn drain_lost_followups(
    msg_rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentUserInput>,
) -> Vec<AgentUserInput> {
    let lost: Vec<_> = std::iter::from_fn(|| msg_rx.try_recv().ok()).collect();
    if !lost.is_empty() {
        log!("[AgentSession] Drained {} lost follow-up(s)", lost.len());
    }
    lost
}

/// Convert drained CC follow-ups into orphaned injections for re-processing,
/// each keeping its kind. Only inputs with an `origin_event_id` are kept.
/// An unread Continue keeps its instructions as engine re-entry. Redelivered
/// non-human inputs stay owed in the store for the next session instead.
pub(crate) fn lost_followups_to_orphans(
    lost: Vec<AgentUserInput>,
) -> Vec<crate::engine::InjectedPrompt> {
    use crate::engine::thread_events::ActorMode;
    use crate::engine::{AgentInputKind, InjectedPromptKind, RedeliveredInput};
    lost.into_iter()
        .filter_map(|input| {
            let (mode, kind) = match input.kind {
                AgentInputKind::User
                | AgentInputKind::Redelivered {
                    input:
                        RedeliveredInput::Message {
                            sender: ActorMode::Human,
                        },
                    ..
                } => (ActorMode::Human, InjectedPromptKind::UserText),
                AgentInputKind::ReentryFromEngine
                | AgentInputKind::CeilingReentry
                | AgentInputKind::Continuation => {
                    (ActorMode::Agent, InjectedPromptKind::ReentryFromEngine)
                }
                AgentInputKind::Redelivered { .. } => return None,
            };
            Some(crate::engine::InjectedPrompt {
                text: input.text,
                event_id: Some(input.origin_event_id?),
                mode,
                spawning_event_id: None,
                images: input.images,
                origin: None,
                kind,
            })
        })
        .collect()
}

/// Put `input`, just received, back at the head of `msg_rx`. The caller holds
/// the `agent_sessions` lock every sender takes, so nothing slips in between.
pub(super) fn requeue_at_head(
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    msg_rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentUserInput>,
    input: AgentUserInput,
) {
    let behind: Vec<_> = std::iter::from_fn(|| msg_rx.try_recv().ok()).collect();
    // The caller holds the open receiver, so a send cannot fail.
    for input in std::iter::once(input).chain(behind) {
        let _ = msg_tx.send(input);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(text: &str) -> AgentUserInput {
        AgentUserInput {
            text: text.into(),
            images: None,
            origin_event_id: None,
            kind: crate::engine::AgentInputKind::User,
            completion: None,
        }
    }

    #[test]
    fn requeue_puts_the_input_back_ahead_of_what_queued_behind_it() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        tx.send(input("first")).unwrap();
        tx.send(input("second")).unwrap();
        let first = rx.try_recv().unwrap();
        requeue_at_head(&tx, &mut rx, first);
        let order: Vec<String> = drain_lost_followups(&mut rx)
            .into_iter()
            .map(|input| input.text)
            .collect();
        assert_eq!(order, ["first", "second"]);
    }

    #[test]
    fn drain_returns_unconsumed_followups() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        let eid = uuid::Uuid::new_v4();
        tx.send(AgentUserInput {
            text: "follow-up 1".into(),
            images: None,
            origin_event_id: Some(eid),
            kind: crate::engine::AgentInputKind::User,
            completion: None,
        })
        .unwrap();
        tx.send(AgentUserInput {
            text: "internal".into(),
            images: None,
            origin_event_id: None,
            kind: crate::engine::AgentInputKind::User,
            completion: None,
        })
        .unwrap();

        let lost = drain_lost_followups(&mut rx);
        assert_eq!(lost.len(), 2, "drain must return all pending messages");
        assert_eq!(lost[0].origin_event_id, Some(eid));
        assert!(lost[1].origin_event_id.is_none());
        assert!(rx.try_recv().is_err(), "channel must be empty after drain");
    }

    /// A discarded wake stays owed, so the next session redelivers it as a
    /// wake rather than as a message the user never sent.
    #[test]
    fn only_a_redelivered_human_message_becomes_an_orphan() {
        use crate::engine::thread_events::ActorMode;
        use crate::engine::{AgentInputKind, RedeliveredInput};
        let redelivered = |input| AgentUserInput {
            text: "late".into(),
            images: None,
            origin_event_id: Some(uuid::Uuid::new_v4()),
            kind: AgentInputKind::Redelivered {
                sent_at: chrono::Utc::now(),
                input,
            },
            completion: None,
        };
        let human = redelivered(RedeliveredInput::Message {
            sender: ActorMode::Human,
        });
        let human_id = human.origin_event_id;
        let orphans = lost_followups_to_orphans(vec![
            human,
            redelivered(RedeliveredInput::ChildReport),
            redelivered(RedeliveredInput::EnginePrompt),
            redelivered(RedeliveredInput::Message {
                sender: ActorMode::Engine,
            }),
        ]);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].event_id, human_id);
    }

    /// The reported incident drained the engine's own Continue and re-ran it
    /// as something the user said. Preserve unread Continues as engine re-entry
    /// so an answer recap or harden request cannot disappear with the run.
    #[test]
    fn a_drained_input_keeps_its_kind() {
        use crate::engine::thread_events::ActorMode;
        use crate::engine::{AgentInputKind, InjectedPromptKind};
        let anchored = |kind| AgentUserInput {
            origin_event_id: Some(uuid::Uuid::new_v4()),
            kind,
            ..input("drained")
        };
        let kept: Vec<(ActorMode, InjectedPromptKind)> = lost_followups_to_orphans(vec![
            anchored(AgentInputKind::User),
            anchored(AgentInputKind::ReentryFromEngine),
            anchored(AgentInputKind::CeilingReentry),
            anchored(AgentInputKind::Continuation),
        ])
        .into_iter()
        .map(|orphan| (orphan.mode, orphan.kind))
        .collect();
        assert!(
            matches!(
                kept.as_slice(),
                [
                    (ActorMode::Human, InjectedPromptKind::UserText),
                    (ActorMode::Agent, InjectedPromptKind::ReentryFromEngine),
                    (ActorMode::Agent, InjectedPromptKind::ReentryFromEngine),
                    (ActorMode::Agent, InjectedPromptKind::ReentryFromEngine),
                ]
            ),
            "{kept:?}"
        );
    }

    /// Unread instructions survive a run exit and reach the coding agent in
    /// order, without creating a user message or invoking the chat provider.
    #[test]
    fn unread_continuation_payloads_reach_the_coding_agent() {
        crate::test_support::run_on_engine_runtime(
            "engine::agent_session::io_helpers::tests::unread_continuation_payloads_reach_the_coding_agent",
            unread_continuation_payloads_case,
        );
    }

    async fn unread_continuation_payloads_case() {
        use crate::engine::thread_events::ActorMode;
        use crate::engine::{AgentInputKind, AgentSession, InjectedPromptKind};
        let (pool, db) = crate::test_support::setup_test_db().await;
        let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;
        let thread_id = crate::test_support::seed_coding_thread(&engine.event_bus, None).await;
        let payloads = [
            "The user answered the pending question: Approve the implementation.",
            crate::engine::claude_code::AUTO_HARDEN_MESSAGE,
            "Continue.",
        ];
        let mut anchors = Vec::new();
        for reason in [
            "answered_after_idle",
            "harden_requested",
            "user_clicked_continue",
        ] {
            let persisted = engine
                .event_bus
                .emit(crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::ContinuationRequested {
                        reason: reason.into(),
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                })
                .await
                .unwrap()
                .unwrap();
            anchors.push(persisted.event_id);
        }
        let before: i64 =
            sqlx::query_scalar("SELECT max(sequence) FROM events WHERE aggregate_id = $1")
                .bind(thread_id.to_string())
                .fetch_one(&pool)
                .await
                .unwrap();
        let (old_tx, mut old_rx) = tokio::sync::mpsc::unbounded_channel();
        for (text, anchor) in payloads.iter().zip(anchors.iter().copied()) {
            old_tx
                .send(AgentUserInput::continuation((*text).into(), anchor))
                .unwrap();
        }
        let orphans = lost_followups_to_orphans(drain_lost_followups(&mut old_rx));
        assert_eq!(orphans.len(), payloads.len());
        for orphan in &orphans {
            assert_eq!(orphan.mode, ActorMode::Agent);
            assert!(matches!(orphan.kind, InjectedPromptKind::ReentryFromEngine));
        }
        let (session, mut rx) = AgentSession::for_test();
        engine
            .agent_sessions
            .lock()
            .await
            .insert(thread_id, session);
        crate::api::chat::process_orphan_chain(engine.clone(), thread_id, orphans).await;
        for (text, anchor) in payloads.iter().zip(anchors.iter().copied()) {
            let delivered = rx
                .try_recv()
                .expect("unread continuation reaches the coding agent");
            assert_eq!(delivered.text, *text);
            assert_eq!(delivered.origin_event_id, Some(anchor));
            assert!(matches!(delivered.kind, AgentInputKind::ReentryFromEngine));
            assert_eq!(delivered.user_origin(), None);
            assert!(!delivered.kind.writes_prompt_sent());
        }
        assert!(rx.try_recv().is_err(), "each instruction queues once");
        let events: Vec<String> = sqlx::query_scalar(
            "SELECT event_type FROM events WHERE aggregate_id = $1 AND sequence > $2 ORDER BY sequence",
        )
        .bind(thread_id.to_string())
        .bind(before)
        .fetch_all(&pool)
        .await
        .unwrap();
        for forbidden in [
            "MessageReceived",
            "CodingAgentPromptSent",
            "PromptInjected",
            "ResponseGenerated",
            "ResponseFailed",
            "ContextCaptured",
        ] {
            assert!(
                !events.iter().any(|event| event == forbidden),
                "unexpected {forbidden}: {events:?}"
            );
        }
        crate::test_support::teardown_test_db(&db).await;
    }

    #[test]
    fn drain_empty_channel_returns_empty() {
        let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        let lost = drain_lost_followups(&mut rx);
        assert!(lost.is_empty());
    }
}
