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

/// The side Q&A must never become a thread event: a recorded event feeds the
/// next session's context (ADR 0318). This module holds no event bus at all.
#[test]
fn the_side_question_module_never_emits_an_event() {
    use crate::test_support::source_scan::{read_production_source, src_root};
    let source = read_production_source(&src_root().join("engine/agent_session/side_question.rs"));
    for forbidden in ["event_bus", ".emit(", "BusEvent", "ThreadEvent"] {
        assert!(
            !source.contains(forbidden),
            "side_question.rs must not touch events, found `{forbidden}`"
        );
    }
}
