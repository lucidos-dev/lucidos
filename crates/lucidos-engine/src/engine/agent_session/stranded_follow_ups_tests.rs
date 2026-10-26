use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, ChildCompletionStatus, EngineReason, EventChannel, EventMeta, MessageOrigin,
    ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};

struct Thread {
    bus: EventBus,
    id: Uuid,
}

impl Thread {
    async fn emit(&self, event: ThreadEvent) -> Uuid {
        self.emit_answering(event, None).await
    }

    /// Emit `event` as part of the request `anchor` opened.
    async fn emit_answering(&self, event: ThreadEvent, anchor: Option<Uuid>) -> Uuid {
        self.bus
            .emit(BusEvent::Thread {
                thread_id: self.id,
                event,
                meta: EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    request_event_id: anchor,
                    ..EventMeta::NONE
                },
            })
            .await
            .unwrap()
            .expect("persisted")
            .event_id
    }

    async fn message(&self, text: &str, mode: ActorMode) -> Uuid {
        self.emit(ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: text.into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode,
            model: None,
            reasoning_effort: None,
            origin: None,
        })
        .await
    }

    async fn user_says(&self, text: &str) -> Uuid {
        self.message(text, ActorMode::Human).await
    }

    async fn session_started(&self) {
        self.session_started_by(None).await;
    }

    /// A cache keep-alive's session, opened by its hidden anchor.
    async fn keep_alive_session_started(&self) {
        let anchor = self
            .emit(ThreadEvent::PromptInjected {
                text: "keep warm".into(),
                mode: ActorMode::Engine,
                origin: Some(MessageOrigin::engine(EngineReason::CacheKeepAlive)),
                injected_message_id: None,
                delivered_event_id: None,
            })
            .await;
        self.session_started_by(Some(anchor)).await;
    }

    async fn session_started_by(&self, anchor: Option<Uuid>) {
        self.emit_answering(
            ThreadEvent::SessionStarted {
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                session_id: String::new(),
                branch: "work".into(),
                repo_id: None,
                coding_agent_kind: Default::default(),
                coding_agent_folder: String::new(),
                app_id: None,
                integrated: false,
                auto_compact_window: None,
                auto_compact_arm: None,
                continuity: None,
            },
            anchor,
        )
        .await;
    }

    async fn forwarded(&self, message: Uuid) {
        self.emit(ThreadEvent::CodingAgentPromptSent {
            text: "forwarded".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            origin: None,
            input_event_id: Some(message),
        })
        .await;
    }

    async fn read(&self, message: Uuid) {
        self.emit(ThreadEvent::CodingAgentInputRead {
            input_event_id: message,
            started_turn: false,
            verbatim: false,
        })
        .await;
    }

    async fn discarded(&self, message: Uuid) {
        self.emit(ThreadEvent::CodingAgentInputDiscarded {
            input_event_id: message,
        })
        .await;
    }

    async fn child_completed(&self, summary: &str) -> Uuid {
        self.emit(ThreadEvent::ChildThreadCompleted {
            child_thread_id: Uuid::new_v4(),
            child_thread_title: Some("child".into()),
            status: ChildCompletionStatus::Success,
            summary: summary.into(),
            pending_change_ids: vec![],
            sub_thread_pending_changes: vec![],
            integrated_branch: None,
        })
        .await
    }

    async fn child_report_filed(&self, summary: &str) -> Uuid {
        self.emit(ThreadEvent::ChildReportFiled {
            child_thread_id: Uuid::new_v4(),
            child_thread_title: Some("child".into()),
            status: ChildCompletionStatus::Success,
            summary: summary.into(),
            pending_change_ids: vec![],
            sub_thread_pending_changes: vec![],
            integrated_branch: None,
        })
        .await
    }

    /// An engine prompt with no starter event, as auto-harden sends.
    async fn engine_prompt_sent(&self, text: &str) -> Uuid {
        self.emit(ThreadEvent::CodingAgentPromptSent {
            text: text.into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            origin: Some(MessageOrigin::engine(EngineReason::MissingHardening)),
            input_event_id: None,
        })
        .await
    }

    async fn injected(&self, text: &str, reason: EngineReason) -> Uuid {
        self.emit(ThreadEvent::PromptInjected {
            text: text.into(),
            mode: ActorMode::Engine,
            origin: Some(MessageOrigin::engine(reason)),
            injected_message_id: None,
            delivered_event_id: None,
        })
        .await
    }

    async fn continuation_requested(&self) -> Uuid {
        self.emit(ThreadEvent::ContinuationRequested {
            reason: "effort_restart".into(),
        })
        .await
    }

    /// The stranded follow-up for `input`, if a fresh session redelivers it.
    async fn stranded_one(&self, input: Uuid) -> Option<StrandedFollowUp> {
        stranded_follow_ups(
            self.bus.pool(),
            Path::new("/nonexistent"),
            self.id,
            &[Uuid::new_v4()],
        )
        .await
        .unwrap()
        .into_iter()
        .find(|follow_up| follow_up.event_id == input)
    }

    async fn removed(&self, message: Uuid) {
        self.emit(ThreadEvent::QueuedMessageRemoved {
            removed_message_id: message,
        })
        .await;
    }

    /// The stranded follow-ups a session starting with `first_prompt` sees.
    async fn stranded(&self, first_prompt: Uuid) -> Vec<Uuid> {
        stranded_follow_ups(
            self.bus.pool(),
            Path::new("/nonexistent"),
            self.id,
            &[first_prompt],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|follow_up| follow_up.event_id)
        .collect()
    }
}

async fn with_thread<F, Fut>(test: F)
where
    F: FnOnce(Thread) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    test(Thread {
        bus,
        id: Uuid::new_v4(),
    })
    .await;
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The reported order: the process takes the follow-up into its queue, dies,
/// and the next session starts with an answer as its first prompt.
#[tokio::test]
async fn a_follow_up_the_previous_session_never_read_is_stranded() {
    with_thread(|thread| async move {
        let first = thread.user_says("build it").await;
        thread.session_started().await;
        thread.read(first).await;
        let queued = thread.user_says("also add a test").await;
        thread.forwarded(queued).await;
        let answer = Uuid::new_v4();

        let stranded = stranded_follow_ups(
            thread.bus.pool(),
            Path::new("/nonexistent"),
            thread.id,
            &[answer],
        )
        .await
        .unwrap();
        assert_eq!(stranded.len(), 1);
        assert_eq!(stranded[0].event_id, queued);
        assert_eq!(stranded[0].text, "also add a test");
        assert!(stranded[0].images.is_none());
    })
    .await;
}

/// The reported failure: Postgres ended the check's connection, and the turn
/// failed on "expected to read 5 bytes, got 0 bytes at EOF". The pool skips its
/// health check, so the query, not the acquire, meets the dead connection.
#[tokio::test]
async fn a_connection_the_server_ended_is_retried_and_the_check_succeeds() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Thread {
        bus,
        id: Uuid::new_v4(),
    };
    thread.session_started().await;
    let queued = thread.user_says("also add a test").await;
    thread.forwarded(queued).await;

    let fragile = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .test_before_acquire(false)
        .connect(&crate::test_support::test_db_url(&db_name))
        .await
        .unwrap();
    let doomed: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&fragile)
        .await
        .unwrap();
    let ended: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
        .bind(doomed)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(ended, "the test must end the pooled connection's backend");
    // The backend exits after the signal, so wait for it to go.
    let mut gone = false;
    for _ in 0..200 {
        gone =
            sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM pg_stat_activity WHERE pid = $1)")
                .bind(doomed)
                .fetch_one(&pool)
                .await
                .unwrap();
        if gone {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(gone, "the ended backend must exit before the check runs");

    let stranded = stranded_follow_ups(
        &fragile,
        Path::new("/nonexistent"),
        thread.id,
        &[Uuid::new_v4()],
    )
    .await
    .expect("the retry reaches the database on a fresh connection");
    assert_eq!(
        stranded.iter().map(|f| f.event_id).collect::<Vec<_>>(),
        vec![queued]
    );
    let replacement: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&fragile)
        .await
        .unwrap();
    assert_ne!(replacement, doomed, "the check ran on a new connection");

    fragile.close().await;
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn several_come_back_oldest_first() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let older = thread.user_says("first").await;
        thread.forwarded(older).await;
        let newer = thread.user_says("second").await;
        thread.forwarded(newer).await;
        assert_eq!(thread.stranded(Uuid::new_v4()).await, [older, newer]);
    })
    .await;
}

#[tokio::test]
async fn a_follow_up_the_agent_read_is_not_stranded() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let queued = thread.user_says("also add a test").await;
        thread.forwarded(queued).await;
        thread.read(queued).await;
        assert!(thread.stranded(Uuid::new_v4()).await.is_empty());
    })
    .await;
}

#[tokio::test]
async fn a_follow_up_the_user_removed_is_not_stranded() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let queued = thread.user_says("never mind").await;
        thread.forwarded(queued).await;
        thread.removed(queued).await;
        assert!(thread.stranded(Uuid::new_v4()).await.is_empty());
    })
    .await;
}

#[tokio::test]
async fn an_engine_message_is_not_stranded() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let engine = thread
            .message("Your changes are not hardened.", ActorMode::Engine)
            .await;
        thread.forwarded(engine).await;
        assert!(thread.stranded(Uuid::new_v4()).await.is_empty());
    })
    .await;
}

/// Only the previous session's follow-ups come back. A message an older session
/// lost belongs to a conversation that has moved on.
#[tokio::test]
async fn a_follow_up_an_older_session_lost_stays_where_it_is() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let old = thread.user_says("weeks ago").await;
        thread.forwarded(old).await;
        thread.session_started().await;
        assert!(thread.stranded(Uuid::new_v4()).await.is_empty());
    })
    .await;
}

/// A cache keep-alive forwards nothing. Its beat every few minutes must not
/// push the previous session's follow-up out of the window.
#[tokio::test]
async fn a_keep_alive_session_does_not_hide_the_follow_up() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let queued = thread.user_says("also add a test").await;
        thread.forwarded(queued).await;
        thread.keep_alive_session_started().await;
        thread.keep_alive_session_started().await;
        assert_eq!(thread.stranded(Uuid::new_v4()).await, [queued]);
    })
    .await;
}

/// A redelivery the next process also died holding was forwarded in the
/// previous session too, so it comes back again.
#[tokio::test]
async fn a_redelivery_lost_again_comes_back_again() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let queued = thread.user_says("also add a test").await;
        thread.forwarded(queued).await;
        thread.session_started().await;
        thread.forwarded(queued).await;
        assert_eq!(thread.stranded(Uuid::new_v4()).await, [queued]);
    })
    .await;
}

/// A message no `CodingAgentPromptSent` names was never forwarded, as on an
/// engine from before reads were reported.
#[tokio::test]
async fn a_message_never_forwarded_is_not_stranded() {
    with_thread(|thread| async move {
        thread.session_started().await;
        thread.user_says("an old row").await;
        assert!(thread.stranded(Uuid::new_v4()).await.is_empty());
    })
    .await;
}

/// An effort restart discarded the first prompt. Any later session delivers it,
/// even one that started after another session already began.
#[tokio::test]
async fn a_discarded_prompt_is_stranded_for_every_later_session() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let prompt = thread.user_says("Hey").await;
        thread.discarded(prompt).await;
        thread.session_started().await;
        assert_eq!(thread.stranded(Uuid::new_v4()).await, [prompt]);
    })
    .await;
}

/// The agent read the discarded prompt, so the discard is closed.
#[tokio::test]
async fn a_discarded_prompt_the_agent_read_later_is_not_stranded() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let prompt = thread.user_says("Hey").await;
        thread.discarded(prompt).await;
        thread.read(prompt).await;
        assert!(thread.stranded(Uuid::new_v4()).await.is_empty());
    })
    .await;
}

/// A discarded prompt the user took back is never redelivered.
#[tokio::test]
async fn a_discarded_prompt_the_user_removed_is_not_stranded() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let prompt = thread.user_says("never mind").await;
        thread.discarded(prompt).await;
        thread.removed(prompt).await;
        assert!(thread.stranded(Uuid::new_v4()).await.is_empty());
    })
    .await;
}

/// A parent whose effort pick discarded its child's wake still learns the
/// child finished: the next session redelivers the completion card.
#[tokio::test]
async fn a_discarded_child_wake_is_stranded_with_its_card() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let wake = thread.child_completed("child did the thing").await;
        thread.discarded(wake).await;
        let follow_up = thread.stranded_one(wake).await.expect("the wake is owed");
        assert!(
            follow_up.text.contains("[CHILD THREAD COMPLETED]"),
            "{}",
            follow_up.text
        );
        assert!(follow_up.text.contains("child did the thing"));
    })
    .await;
}

#[tokio::test]
async fn a_discarded_child_report_is_stranded_with_its_card() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let report = thread.child_report_filed("filed for later").await;
        thread.discarded(report).await;
        let follow_up = thread
            .stranded_one(report)
            .await
            .expect("the report is owed");
        assert!(
            follow_up.text.contains("[CHILD THREAD REPORT FILED]"),
            "{}",
            follow_up.text
        );
        assert!(follow_up.text.contains("filed for later"));
    })
    .await;
}

#[tokio::test]
async fn a_discarded_engine_prompt_is_stranded_with_its_text() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let prompt = thread.engine_prompt_sent("Harden your changes.").await;
        thread.discarded(prompt).await;
        let follow_up = thread
            .stranded_one(prompt)
            .await
            .expect("the prompt is owed");
        assert_eq!(follow_up.text, "Harden your changes.");
    })
    .await;
}

#[tokio::test]
async fn a_discarded_nudge_is_stranded_with_its_text() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let nudge = thread
            .injected("Propose with a plan.", EngineReason::MissingPlan)
            .await;
        thread.discarded(nudge).await;
        let follow_up = thread.stranded_one(nudge).await.expect("the nudge is owed");
        assert_eq!(follow_up.text, "Propose with a plan.");
    })
    .await;
}

/// An engine coding turn's message is the only prompt its session opened for.
#[tokio::test]
async fn a_discarded_engine_message_is_stranded() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let message = thread.message("Fix main.", ActorMode::Engine).await;
        thread.discarded(message).await;
        let follow_up = thread
            .stranded_one(message)
            .await
            .expect("the message is owed");
        assert_eq!(follow_up.text, "Fix main.");
    })
    .await;
}

/// Every owed kind drops out once some session's agent read it.
#[tokio::test]
async fn a_discarded_input_of_any_kind_the_agent_read_is_not_stranded() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let inputs = [
            thread.child_completed("done").await,
            thread.child_report_filed("filed").await,
            thread.engine_prompt_sent("Harden.").await,
            thread.injected("Plan.", EngineReason::MissingPlan).await,
            thread.message("Fix main.", ActorMode::Engine).await,
        ];
        for input in inputs {
            thread.discarded(input).await;
        }
        assert_eq!(thread.stranded(Uuid::new_v4()).await, inputs);
        for input in inputs {
            thread.read(input).await;
        }
        assert!(thread.stranded(Uuid::new_v4()).await.is_empty());
    })
    .await;
}

/// A later resume sends its own nudge, and a keep-alive only warms the cache.
#[tokio::test]
async fn a_discarded_continuation_or_keep_alive_is_never_owed() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let continuation = thread.continuation_requested().await;
        let keep_alive = thread
            .injected("keep warm", EngineReason::CacheKeepAlive)
            .await;
        thread.discarded(continuation).await;
        thread.discarded(keep_alive).await;
        assert!(thread.stranded(Uuid::new_v4()).await.is_empty());
    })
    .await;
}

/// The starting session sends its own first prompt, so it does not send it twice.
#[tokio::test]
async fn the_first_prompt_is_not_redelivered() {
    with_thread(|thread| async move {
        thread.session_started().await;
        let queued = thread.user_says("also add a test").await;
        thread.forwarded(queued).await;
        assert!(thread.stranded(queued).await.is_empty());
    })
    .await;
}

#[tokio::test]
async fn the_fate_of_an_input_answers_from_any_session() {
    with_thread(|thread| async move {
        let message = thread.user_says("go").await;
        let pool = thread.bus.pool();
        let fate = || input_fate(pool, thread.id, message);
        assert_eq!(fate().await.unwrap(), InputFate::NeverForwarded);
        thread.session_started().await;
        thread.forwarded(message).await;
        assert_eq!(fate().await.unwrap(), InputFate::Stranded);
        thread.read(message).await;
        thread.session_started().await;
        assert_eq!(fate().await.unwrap(), InputFate::Read);
    })
    .await;
}

#[test]
fn the_note_says_the_message_is_late_and_when_it_was_sent() {
    let sent_at = DateTime::parse_from_rfc3339("2026-10-17T09:00:50Z")
        .unwrap()
        .with_timezone(&Utc);
    let note = late_delivery_note(HUMAN_MESSAGE, sent_at);
    assert_eq!(
        note,
        "[The previous session ended before you read the message below. \
         The user sent it at 2026-10-17 09:00 UTC.]"
    );
}

/// A wrong author would mislead the agent about whose request it is.
#[test]
fn the_note_names_who_sent_the_input() {
    let note = |input| late_delivery_note(input, Utc::now());
    for (input, sender) in [
        (
            RedeliveredInput::Message {
                sender: ActorMode::Engine,
            },
            "Lucidos sent it",
        ),
        (RedeliveredInput::EnginePrompt, "Lucidos sent it"),
        (
            RedeliveredInput::Message {
                sender: ActorMode::Agent,
            },
            "Another thread sent it",
        ),
        (RedeliveredInput::ChildReport, "The child thread sent it"),
    ] {
        let note = note(input);
        assert!(note.contains(sender), "{note}");
        assert!(note.contains("still applies"), "{note}");
    }
    assert!(note(RedeliveredInput::ChildReport).contains("child thread's report below"));
}

/// Only a message writes a prompt row: a wake or an engine prompt already
/// renders as its own event.
#[test]
fn only_a_redelivered_message_writes_a_prompt_row() {
    let redelivered = |input| AgentInputKind::Redelivered {
        sent_at: Utc::now(),
        input,
    };
    assert!(AgentInputKind::User.writes_prompt_sent());
    assert!(redelivered(HUMAN_MESSAGE).writes_prompt_sent());
    assert!(redelivered(RedeliveredInput::Message {
        sender: ActorMode::Engine
    })
    .writes_prompt_sent());
    assert!(!AgentInputKind::ReentryFromEngine.writes_prompt_sent());
    assert!(!redelivered(RedeliveredInput::ChildReport).writes_prompt_sent());
    assert!(!redelivered(RedeliveredInput::EnginePrompt).writes_prompt_sent());
}

/// A wake is no user's message, so no device block is held for it.
#[test]
fn a_redelivered_wake_has_no_user_origin() {
    let origin = Uuid::new_v4();
    let input = |input| AgentUserInput {
        text: String::new(),
        images: None,
        origin_event_id: Some(origin),
        kind: AgentInputKind::Redelivered {
            sent_at: Utc::now(),
            input,
        },
        completion: None,
    };
    assert_eq!(input(HUMAN_MESSAGE).user_origin(), Some(origin));
    assert_eq!(input(RedeliveredInput::ChildReport).user_origin(), None);
    assert_eq!(input(RedeliveredInput::EnginePrompt).user_origin(), None);
}

const HUMAN_MESSAGE: RedeliveredInput = RedeliveredInput::Message {
    sender: ActorMode::Human,
};

fn stranded_message(text: &str) -> StrandedFollowUp {
    StrandedFollowUp {
        event_id: Uuid::new_v4(),
        input: HUMAN_MESSAGE,
        text: text.into(),
        images: None,
        sent_at: Utc::now(),
    }
}

/// The redelivery is its own input on the session's channel, named by the
/// original message, ahead of a follow-up sent during the startup. It is
/// marked unforwarded before it enters the channel, so a withdraw in between
/// waits for its forward instead of overtaking it.
#[tokio::test]
async fn a_redelivery_is_queued_as_its_own_input_ahead_of_startup_input() {
    let (msg_tx, mut msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
    let mut session = AgentSession::for_test_with_sender(msg_tx.clone());
    msg_tx
        .send(AgentUserInput {
            text: "sent during startup".into(),
            images: None,
            origin_event_id: Some(Uuid::new_v4()),
            kind: AgentInputKind::User,
            completion: None,
        })
        .unwrap();
    let follow_up = stranded_message("also add a test");
    let id = follow_up.event_id;
    let sent_at = follow_up.sent_at;

    queue_ahead_of_startup_input(&mut session, &mut msg_rx, vec![follow_up]);

    assert!(session.unforwarded_inputs.contains(&id));
    let first = msg_rx.try_recv().expect("the redelivery is queued");
    assert_eq!(first.origin_event_id, Some(id));
    assert_eq!(first.text, "also add a test");
    assert_eq!(
        first.kind,
        AgentInputKind::Redelivered {
            sent_at,
            input: HUMAN_MESSAGE
        }
    );
    assert_eq!(first.user_origin(), Some(id));
    let second = msg_rx.try_recv().expect("the startup input is put back");
    assert_eq!(second.text, "sent during startup");
    assert!(msg_rx.try_recv().is_err());
}
