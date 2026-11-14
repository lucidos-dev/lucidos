use super::*;
use crate::test_support::{setup_test_db, teardown_test_db};

fn row(source: &str, coding_agent: Option<&str>) -> Option<ThreadRow> {
    Some((
        source.to_string(),
        coding_agent.map(str::to_string),
        "active".to_string(),
    ))
}

#[test]
fn all_agents_take_side_questions() {
    assert_eq!(
        agent_for(row("claude_code", Some("claude-code"))),
        Ok(SideQuestionAgent::ClaudeCode)
    );
    // Rows from before the column existed were all Claude Code.
    assert_eq!(
        agent_for(row("claude_code", None)),
        Ok(SideQuestionAgent::ClaudeCode)
    );
    assert_eq!(
        agent_for(row("claude_code", Some("codex"))),
        Ok(SideQuestionAgent::Codex)
    );
    assert_eq!(agent_for(row("chat", None)), Ok(SideQuestionAgent::Lucidos));
    assert_eq!(agent_for(None), Err(NO_SUCH_THREAD));
    // A draft still composing has not started.
    let draft = Some(("chat".to_string(), None, "composing".to_string()));
    assert_eq!(agent_for(draft), Err(NO_SUCH_THREAD));
}

/// An ask naming a blob the workspace never received is refused before
/// anything is recorded, and the refusal names the hash.
#[test]
fn an_unknown_image_is_refused_by_name() {
    let workspace = tempfile::tempdir().unwrap();
    let hash = "d".repeat(64);
    match check_images(workspace.path(), std::slice::from_ref(&hash)) {
        Err(SideQuestionFailure::UnknownImage(message)) => assert!(message.contains(&hash)),
        other => panic!("expected an unknown-image refusal, got {other:?}"),
    }
    assert_eq!(check_images(workspace.path(), &[]), Ok(()));
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
async fn a_codex_thread_is_accepted_without_recording_during_admission() {
    let (pool, db_name) = setup_test_db().await;
    let codex = Uuid::new_v4();
    let claude = Uuid::new_v4();
    seed_thread(&pool, codex, "claude_code", "codex").await;
    seed_thread(&pool, claude, "claude_code", "claude-code").await;
    let before = event_count(&pool).await;

    assert_eq!(
        check_thread(&pool, codex).await,
        Ok(SideQuestionAgent::Codex)
    );
    assert_eq!(
        check_thread(&pool, claude).await,
        Ok(SideQuestionAgent::ClaudeCode)
    );
    assert_eq!(
        check_thread(&pool, Uuid::new_v4()).await,
        Err(SideQuestionFailure::Refused(NO_SUCH_THREAD))
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
        image_hashes: vec![],
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
            image_hashes: vec![],
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

/// Record one side-question event on a test thread.
async fn record_event(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    record(bus, thread_id, event, EventMeta::NONE)
        .await
        .unwrap();
}

fn asked(side_question_id: Uuid) -> ThreadEvent {
    ThreadEvent::SideQuestionAsked {
        side_question_id,
        question: "q".into(),
        image_hashes: vec![],
    }
}

fn failed(side_question_id: Uuid) -> ThreadEvent {
    ThreadEvent::SideQuestionFailed {
        side_question_id,
        error: "busy".into(),
    }
}

/// A retry re-asks under the card's own id, so an id may be asked again once
/// every ask failed. One still running, or answered, may not.
#[tokio::test]
async fn an_id_may_be_asked_again_only_after_every_ask_failed() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let id = Uuid::new_v4();
    assert_eq!(may_ask(&pool, thread_id, id).await, Ok(true));

    record_event(&bus, thread_id, asked(id)).await;
    assert_eq!(may_ask(&pool, thread_id, id).await, Ok(false));

    record_event(&bus, thread_id, failed(id)).await;
    assert_eq!(may_ask(&pool, thread_id, id).await, Ok(true));
    assert_eq!(may_ask(&pool, Uuid::new_v4(), id).await, Ok(true));

    record_event(&bus, thread_id, asked(id)).await;
    assert_eq!(may_ask(&pool, thread_id, id).await, Ok(false));

    record_event(&bus, thread_id, failed(id)).await;
    assert_eq!(may_ask(&pool, thread_id, id).await, Ok(true));

    record_event(&bus, thread_id, asked(id)).await;
    let answer = ThreadEvent::SideQuestionAnswered {
        side_question_id: id,
        answer: "a".into(),
    };
    record_event(&bus, thread_id, answer).await;
    assert_eq!(may_ask(&pool, thread_id, id).await, Ok(false));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A restart during a retry leaves the re-ask unsettled, though an earlier
/// ask under the same id already failed. Recovery fails the re-ask too.
#[tokio::test]
async fn recovery_fails_a_retry_a_restart_interrupted() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let id = Uuid::new_v4();
    record_event(&bus, thread_id, asked(id)).await;
    record_event(&bus, thread_id, failed(id)).await;
    record_event(&bus, thread_id, asked(id)).await;

    assert_eq!(fail_unsettled_side_questions(&pool, &bus).await.unwrap(), 1);
    assert_eq!(fail_unsettled_side_questions(&pool, &bus).await.unwrap(), 0);
    assert_eq!(
        recorded_types(&pool, thread_id).await,
        [
            "SideQuestionAsked",
            "SideQuestionFailed",
            "SideQuestionAsked",
            "SideQuestionFailed"
        ]
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Exercise session resolution, the runtime fork and the real auxiliary capture together.
#[tokio::test]
async fn codex_side_question_records_its_resolved_model_and_cost() {
    use crate::llm::ProviderBuildContext;
    use crate::test_support::test_db_url;
    let (pool, db) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    let script = workspace.path().join("codex-stub.sh");
    let guard_reply = serde_json::json!({"id":6,"result":{"data":[{"errors":[],"hooks":[{
        "eventName":"preToolUse","handlerType":"command",
        "command":crate::runtime::codex_side_question::TOOL_GUARD_COMMAND,
        "matcher":"*","async":false,"enabled":true,"trustStatus":"trusted"
    }]}]}})
    .to_string()
    .replace('\'', "'\"'\"'");
    std::fs::write(&script, r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{"id":1,"result":{}}' ;;
    *'"method":"config/read"'*) printf '%s\n' '{"id":5,"result":{"config":{"mcp_servers":{}}}}' ;;
    *'"method":"hooks/list"'*) printf '%s\n' '__GUARD_REPLY__' ;;
    *'"method":"thread/fork"'*) printf '%s\n' '{"id":2,"result":{"thread":{"id":"fork"},"model":"gpt-5.5"}}' ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":3,"result":{"turn":{"id":"turn"}}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"fork","item":{"type":"agentMessage","id":"answer","text":"An answer"}}}'
      printf '%s\n' '{"method":"thread/tokenUsage/updated","params":{"threadId":"fork","tokenUsage":{"last":{"inputTokens":100,"cachedInputTokens":20,"outputTokens":5}}}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"fork","turn":{"id":"turn","status":"completed"}}}' ;;
  esac
done
"#.replace("__GUARD_REPLY__", &guard_reply)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    for (key, value) in [
        (
            crate::core::prefs::AGENTS_UNSANDBOXED.spec.key,
            "true".to_string(),
        ),
        (
            crate::core::prefs::CODING_AGENT_CODEX_PATH.key(),
            script.to_string_lossy().into_owned(),
        ),
    ] {
        crate::core::preferences::PreferenceStore::set_row_for_test(&pool, key, &value)
            .await
            .unwrap();
    }
    let engine = LucidosEngine::new(
        workspace.path().to_path_buf(),
        &test_db_url(&db),
        ProviderBuildContext {
            default_model: "mock".into(),
            model_is_mock: true,
            vertex_project_id: String::new(),
            vertex_location: crate::llm::vertex::location_handle(String::new()),
            vertex_token_cache: None,
            model_registry: Default::default(),
            not_served: Default::default(),
            claude_gateway_access: None,
            chatgpt_plan_access: None,
        },
    )
    .await
    .unwrap();
    let thread = Uuid::new_v4();
    seed_thread(&pool, thread, "claude_code", "codex").await;
    record(
        &engine.event_bus,
        thread,
        ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some("main".into()),
            coding_agent: CodingAgent::Codex,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
        EventMeta::NONE,
    )
    .await
    .unwrap();
    let answer = engine
        .answer_codex_side_question(
            thread,
            "question",
            &[],
            tokio::time::Instant::now() + std::time::Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(answer, "An answer");
    let capture: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE thread_id=$1 AND event_type='ContextCaptured'",
    )
    .bind(thread)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(capture["purpose"], "side_question");
    assert_eq!(capture["model"], "gpt-5.5");
    assert_eq!(capture["usage"]["input_tokens"], 100);
    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}
