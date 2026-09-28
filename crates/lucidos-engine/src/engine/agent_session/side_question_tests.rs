use super::*;
use crate::test_support::{setup_test_db, teardown_test_db};

fn row(source: &str, coding_agent: Option<&str>) -> Option<(String, Option<String>)> {
    Some((source.to_string(), coding_agent.map(str::to_string)))
}

#[test]
fn only_a_leading_btw_word_is_a_side_question() {
    for yes in ["/btw what is X?", "/btw", "  /btw\nmultiline", "/btw\tq"] {
        assert!(is_side_question(yes), "{yes:?}");
    }
    for no in [
        "/btwx q",
        "btw q",
        "what about /btw q",
        "/compact",
        "hello",
        "",
    ] {
        assert!(!is_side_question(no), "{no:?}");
    }
}

#[test]
fn only_claude_code_threads_take_side_questions() {
    assert_eq!(refusal_for(row("claude_code", Some("claude-code"))), None);
    // Rows from before the column existed were all Claude Code.
    assert_eq!(refusal_for(row("claude_code", None)), None);
    assert_eq!(
        refusal_for(row("claude_code", Some("codex"))),
        Some(CODEX_UNSUPPORTED)
    );
    assert_eq!(
        refusal_for(row("chat", None)),
        Some(NOT_CODING_AGENT_THREAD)
    );
    assert_eq!(refusal_for(None), Some(NOT_CODING_AGENT_THREAD));
}

/// A session map holding one live Claude Code session whose side questions
/// arrive on the returned receiver.
fn live_session(
    thread_id: Uuid,
) -> (
    Mutex<HashMap<Uuid, AgentSession>>,
    tokio::sync::mpsc::UnboundedReceiver<SideQuestionRequest>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let (mut session, _msg_rx) = AgentSession::for_test();
    session.side_question_tx = Some(tx);
    (Mutex::new(HashMap::from([(thread_id, session)])), rx)
}

/// A deadline no test here reaches.
fn deadline() -> tokio::time::Instant {
    tokio::time::Instant::now() + std::time::Duration::from_secs(10)
}

#[tokio::test]
async fn a_live_session_answers_through_its_side_question_channel() {
    let thread_id = Uuid::new_v4();
    let (sessions, mut rx) = live_session(thread_id);
    let driver = tokio::spawn(async move {
        let request = rx.recv().await.expect("side question reaches the driver");
        assert_eq!(request.question, "what is X?");
        request.reply.send(Ok("X is a letter.".into())).unwrap();
    });
    let answer = ask_live(&sessions, thread_id, "what is X?", deadline()).await;
    driver.await.unwrap();
    assert_eq!(answer, Ok(Some("X is a letter.".into())));
}

#[tokio::test]
async fn a_process_that_ends_before_answering_falls_back_to_cold() {
    let thread_id = Uuid::new_v4();
    let (sessions, mut rx) = live_session(thread_id);
    // The driver exits and drops the pending reply unanswered.
    tokio::spawn(async move { drop(rx.recv().await) });
    assert_eq!(
        ask_live(&sessions, thread_id, "q", deadline()).await,
        Ok(None)
    );
}

#[tokio::test]
async fn claude_codes_refusal_reaches_the_asker() {
    let thread_id = Uuid::new_v4();
    let (sessions, mut rx) = live_session(thread_id);
    tokio::spawn(async move {
        let request = rx.recv().await.unwrap();
        request.reply.send(Err("busy".into())).unwrap();
    });
    assert_eq!(
        ask_live(&sessions, thread_id, "q", deadline()).await,
        Err(SideQuestionFailure::Failed("busy".into()))
    );
}

/// The live wait ends at the shared deadline. A cold retry after it can never
/// push the question past the budget the browser waits for.
#[tokio::test]
async fn a_live_question_stops_at_the_shared_deadline() {
    let thread_id = Uuid::new_v4();
    let (sessions, mut rx) = live_session(thread_id);
    // The driver holds the request and never answers.
    let held = tokio::spawn(async move { rx.recv().await });
    let started = tokio::time::Instant::now();
    let deadline = started + std::time::Duration::from_millis(200);
    assert_eq!(
        ask_live(&sessions, thread_id, "q", deadline).await,
        Err(SideQuestionFailure::Failed(
            crate::runtime::claude_code::side_question_timeout_message()
        ))
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    drop(held);
}

#[tokio::test]
async fn no_live_process_goes_cold() {
    let thread_id = Uuid::new_v4();
    let empty = Mutex::new(HashMap::new());
    assert_eq!(ask_live(&empty, thread_id, "q", deadline()).await, Ok(None));

    let (sessions, _rx) = live_session(thread_id);
    sessions
        .lock()
        .await
        .get_mut(&thread_id)
        .unwrap()
        .process_exited = true;
    assert_eq!(
        ask_live(&sessions, thread_id, "q", deadline()).await,
        Ok(None)
    );

    // A Codex session carries no side-question channel.
    let (codex, _msg_rx) = AgentSession::for_test();
    let sessions = Mutex::new(HashMap::from([(thread_id, codex)]));
    assert_eq!(
        ask_live(&sessions, thread_id, "q", deadline()).await,
        Ok(None)
    );
}

async fn seed_thread(pool: &sqlx::PgPool, thread_id: Uuid, source: &str, coding_agent: &str) {
    sqlx::query(
        "INSERT INTO thread_summaries \
         (thread_id, title, source, coding_agent, message_count, last_activity, has_response, is_saved) \
         VALUES ($1, 't', $2, $3, 1, NOW(), FALSE, FALSE)",
    )
    .bind(thread_id)
    .bind(source)
    .bind(coding_agent)
    .execute(pool)
    .await
    .expect("seed thread");
}

async fn event_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_codex_thread_is_refused_and_nothing_is_recorded() {
    let (pool, db_name) = setup_test_db().await;
    let codex = Uuid::new_v4();
    let claude = Uuid::new_v4();
    seed_thread(&pool, codex, "claude_code", "codex").await;
    seed_thread(&pool, claude, "claude_code", "claude-code").await;
    let before = event_count(&pool).await;

    assert_eq!(
        check_thread(&pool, codex).await,
        Err(SideQuestionFailure::Refused(CODEX_UNSUPPORTED))
    );
    assert_eq!(check_thread(&pool, claude).await, Ok(()));
    assert_eq!(
        check_thread(&pool, Uuid::new_v4()).await,
        Err(SideQuestionFailure::Refused(NOT_CODING_AGENT_THREAD))
    );
    assert_eq!(event_count(&pool).await, before);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The side-question event types recorded on a thread, oldest first.
async fn recorded_types(pool: &sqlx::PgPool, thread_id: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT event_type FROM events WHERE thread_id = $1 \
         AND event_type LIKE 'SideQuestion%' ORDER BY sequence",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// An ask is recorded, and its id then counts as asked on that thread only.
#[tokio::test]
async fn a_recorded_ask_is_known_by_its_id_on_its_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let side_question_id = Uuid::new_v4();
    assert_eq!(
        was_asked(&pool, thread_id, side_question_id).await,
        Ok(false)
    );

    let asked = ThreadEvent::SideQuestionAsked {
        side_question_id,
        question: "what does it return?".into(),
    };
    record(&bus, thread_id, asked, EventMeta::NONE)
        .await
        .unwrap();

    assert_eq!(
        was_asked(&pool, thread_id, side_question_id).await,
        Ok(true)
    );
    assert_eq!(
        was_asked(&pool, Uuid::new_v4(), side_question_id).await,
        Ok(false)
    );
    assert_eq!(
        recorded_types(&pool, thread_id).await,
        ["SideQuestionAsked"]
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn an_ask_settles_as_its_answer_or_its_failure() {
    let id = Uuid::new_v4();
    let json = |event: ThreadEvent| serde_json::to_value(event).unwrap();
    assert_eq!(
        json(settled_event(id, &Ok("A string.".into()))),
        json(ThreadEvent::SideQuestionAnswered {
            side_question_id: id,
            answer: "A string.".into()
        })
    );
    assert_eq!(
        json(settled_event(
            id,
            &Err(SideQuestionFailure::Failed("busy".into()))
        )),
        json(ThreadEvent::SideQuestionFailed {
            side_question_id: id,
            error: "busy".into()
        })
    );
    assert_eq!(
        json(settled_event(
            id,
            &Err(SideQuestionFailure::Refused(NO_SESSION_YET))
        )),
        json(ThreadEvent::SideQuestionFailed {
            side_question_id: id,
            error: NO_SESSION_YET.into()
        })
    );
}

/// A restart kills the process an answer was on its way to. Recovery fails
/// every unsettled ask once, and leaves settled ones alone.
#[tokio::test]
async fn recovery_fails_only_the_asks_nothing_settled() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let (open, answered) = (Uuid::new_v4(), Uuid::new_v4());
    for side_question_id in [open, answered] {
        let asked = ThreadEvent::SideQuestionAsked {
            side_question_id,
            question: "q".into(),
        };
        record(&bus, thread_id, asked, EventMeta::NONE)
            .await
            .unwrap();
    }
    let answer = ThreadEvent::SideQuestionAnswered {
        side_question_id: answered,
        answer: "a".into(),
    };
    record(&bus, thread_id, answer, EventMeta::NONE)
        .await
        .unwrap();

    assert_eq!(fail_unsettled_side_questions(&pool, &bus).await.unwrap(), 1);
    assert_eq!(fail_unsettled_side_questions(&pool, &bus).await.unwrap(), 0);

    let failed_id: String = sqlx::query_scalar(
        "SELECT payload->>'side_question_id' FROM events \
         WHERE thread_id = $1 AND event_type = 'SideQuestionFailed'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(failed_id, open.to_string());

    pool.close().await;
    teardown_test_db(&db_name).await;
}
