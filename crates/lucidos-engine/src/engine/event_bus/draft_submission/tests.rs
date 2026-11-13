//! Synthetic DB fixtures for transactional acceptance and owner authority.
//! Includes real mock-backed chat dispatch; transport authentication and
//! ordinary composer serialization remain separate contracts.
use super::*;
use crate::test_support::{setup_test_db, teardown_test_db};
use serde_json::json;

#[path = "coding_executor_tests.rs"]
mod coding_executor_tests;

#[tokio::test]
async fn accepted_saved_chat_executes_with_original_identity_and_saved_provenance() {
    use crate::engine::thread_queue::ExecutableEntry;
    use crate::llm::ProviderBuildContext;
    use crate::test_support::test_db_url;
    use std::sync::Arc;

    // Construction loads key material and starts runtime services. Isolate it
    // in a child with a disposable home, as the providerless boot test does.
    const CHILD: &str = "LUCIDOS_TEST_SAVED_DRAFT_EXECUTOR_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let home = tempfile::tempdir().unwrap();
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            tokio::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "engine::event_bus::draft_submission::tests::accepted_saved_chat_executes_with_original_identity_and_saved_provenance",
                    "--nocapture",
                ])
                .env("RUST_MIN_STACK", "16777216")
                .env(CHILD, "1")
                .env("GIT_AUTHOR_NAME", "Test User")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test User")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .env("HOME", home.path())
                .env_remove("LUCIDOS_GATEWAY_DATA")
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let (pool, db) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    let engine = Arc::new(
        crate::engine::LucidosEngine::new(
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
        .unwrap(),
    );
    engine.set_self_arc(&engine);
    let registry = crate::llm::model_registry::empty();
    registry.write().unwrap().insert(
        "mock".into(),
        crate::llm::model_registry::ModelRouting::single(crate::llm::ProviderKind::OpenAi, "mock"),
    );
    let service = crate::engine::draft_submission::DraftSubmissionService {
        bus: &engine.event_bus,
        pool: &pool,
        workspace: workspace.path(),
        defaults: crate::core::ModelDefaults {
            registry: &registry,
            fallback_model: "mock",
        },
        configured: &[crate::llm::ProviderKind::OpenAi],
    };
    let image_hashes: Vec<String> = [1, 2]
        .into_iter()
        .map(|color| {
            let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                1,
                1,
                image::Rgba([color, 0, 0, 255]),
            ));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
            crate::core::blobs::write_blob(workspace.path(), &bytes.into_inner())
                .unwrap()
                .hash
        })
        .collect();
    for (state, text, hashes, lose_image) in [
        ("composing", "saved text", vec![], false),
        ("active", "saved text", vec![], false),
        ("composing", "", image_hashes.clone(), false),
        (
            "active",
            "saved text\n",
            vec![
                image_hashes[1].clone(),
                image_hashes[0].clone(),
                image_hashes[1].clone(),
            ],
            false,
        ),
        ("active", "saved text", image_hashes.clone(), true),
    ] {
        let mut prepared = fixture(&pool).await;
        sqlx::query("UPDATE thread_summaries SET state=$2,compose_mode='lucidos',compose_text=$3,compose_images=$4 WHERE thread_id=$1")
            .bind(prepared.request.thread_id)
            .bind(state)
            .bind(text)
            .bind(json!(hashes))
            .execute(&pool)
            .await
            .unwrap();
        refresh_snapshot(&pool, &mut prepared).await;
        let retry = copy(&prepared);
        let receipt = service
            .submit(prepared.request, prepared.actor, prepared.authority)
            .await
            .unwrap();
        let queued: serde_json::Value =
            sqlx::query_scalar("SELECT request FROM thread_queue WHERE id=$1")
                .bind(receipt.queue_entry_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let message_id = Uuid::parse_str(queued["event_id"].as_str().unwrap()).unwrap();
        if lose_image {
            let blob = crate::core::blobs::resolve_blob(workspace.path(), &hashes[0]).unwrap();
            std::fs::remove_file(blob.path).unwrap();
        }
        engine
            .clone()
            .execute_thread_queue_entry(ExecutableEntry {
                id: receipt.queue_entry_id,
                request: serde_json::from_value(queued).unwrap(),
            })
            .await
            .unwrap();
        let messages: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
            "SELECT id,payload FROM events WHERE aggregate_id=$1 AND event_type='MessageReceived' ORDER BY sequence"
        ).bind(receipt.thread_id.to_string()).fetch_all(&pool).await.unwrap();
        assert_eq!(messages.len(), 1, "{state}: exactly one accepted message");
        assert_eq!(messages[0].0, message_id);
        assert_eq!(messages[0].1["text"], text);
        assert_eq!(
            messages[0]
                .1
                .get("user_image_hashes")
                .cloned()
                .unwrap_or_else(|| json!([])),
            json!(hashes)
        );
        assert_eq!(messages[0].1["mode"], "human");
        assert_eq!(messages[0].1["origin"]["kind"], "saved_draft");
        assert_eq!(
            messages[0].1["origin"]["submission_event_id"],
            json!(receipt.event_id)
        );
        assert!(messages[0].1["device_id"].is_null());
        let responses: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE aggregate_id=$1 AND event_type='ResponseGenerated'",
        )
        .bind(receipt.thread_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            responses,
            i64::from(!lose_image),
            "{state}: the real mock-backed chat turn completes"
        );
        let failures: Vec<Option<Uuid>> = sqlx::query_scalar(
            "SELECT (payload->>'request_event_id')::uuid FROM events WHERE aggregate_id=$1 AND event_type='ResponseFailed'",
        )
        .bind(receipt.thread_id.to_string())
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            failures,
            if lose_image {
                vec![Some(message_id)]
            } else {
                vec![]
            }
        );
        assert_eq!(
            service
                .submit(retry.request, retry.actor, retry.authority)
                .await
                .unwrap(),
            receipt
        );
    }
    // An ordinary accepted envelope must retain the same contexts as direct
    // submission. Round-trip the persisted request before the real executor.
    let context_thread = Uuid::new_v4();
    let request = json!({
        "type": "agent-chat", "message": "Use the selected context", "mode": "human",
        "thread_id": context_thread, "event_id": Uuid::new_v4().to_string(),
        "model": "mock", "provider": "openai",
        "app_context": {"app_id": "sample-app"},
        "file_context": "[repo:sample] src/lib.rs:3-8",
        "url_context": {"url": "https://example.com", "title": "Reference", "content": "Selected page"},
    });
    engine
        .clone()
        .execute_thread_queue_entry(ExecutableEntry {
            id: Uuid::new_v4(),
            request: serde_json::from_value(request).unwrap(),
        })
        .await
        .unwrap();
    let captures: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM events WHERE aggregate_id=$1 AND event_type='ContextCaptured'",
    )
    .bind(context_thread.to_string())
    .fetch_all(&pool)
    .await
    .unwrap();
    for name in ["App Context", "File Context", "URL Context"] {
        assert!(
            captures.iter().any(
                |capture| capture["sections"]
                    .as_array()
                    .is_some_and(
                        |sections| sections.iter().any(|section| section["name"] == name
                            && section["content_chars"]
                                .as_u64()
                                .is_some_and(|size| size > 0))
                    )
            ),
            "queued execution lost {name}: {captures:?}"
        );
    }
    // A failed message append must escape the ordinary queue executor. A
    // swallowed error would let its manager delete the only durable work.
    sqlx::raw_sql(
        "CREATE FUNCTION reject_queued_message_for_test() RETURNS trigger AS $$
         BEGIN IF NEW.event_type = 'MessageReceived' THEN
             RAISE EXCEPTION 'simulated message append failure';
         END IF; RETURN NEW; END; $$ LANGUAGE plpgsql;
         CREATE TRIGGER reject_queued_message_for_test BEFORE INSERT ON events
         FOR EACH ROW EXECUTE FUNCTION reject_queued_message_for_test();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let failed = engine
        .clone()
        .execute_thread_queue_entry(ExecutableEntry {
            id: Uuid::new_v4(),
            request: serde_json::from_value(json!({
                "type":"agent-chat", "message":"ordinary failed append", "mode":"human",
                "thread_id":Uuid::new_v4(), "event_id":Uuid::new_v4(),
                "model":"mock", "provider":"openai",
            }))
            .unwrap(),
        })
        .await;
    assert!(
        failed.is_err(),
        "ordinary queue executor swallowed an append failure"
    );
    sqlx::raw_sql("DROP TRIGGER reject_queued_message_for_test ON events; DROP FUNCTION reject_queued_message_for_test();")
        .execute(&pool).await.unwrap();
    verify_ordinary_http_acceptance(engine.clone()).await;
    engine.mark_shutting_down();
    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}

/// Exercise the production router and executor while capacity holds dispatch
/// behind the acceptance acknowledgement. Runs inside the isolated mock HOME.
async fn verify_ordinary_http_acceptance(engine: std::sync::Arc<crate::engine::LucidosEngine>) {
    use std::sync::Arc;
    let pool = engine.pool().clone();
    let device = "ordinary-http-test-device";
    sqlx::query("INSERT INTO devices (id) VALUES ($1)")
        .bind(device)
        .execute(&pool)
        .await
        .unwrap();
    let mut policy = engine.thread_queue.policy().await;
    // Standing-instruction fixtures leave their synthetic caller turns open.
    // Leave exactly one slot above those existing occupants, then hold it.
    policy.max_concurrent_total = engine.thread_queue.user_entries().await.len() + 1;
    policy.reserved_background = 0;
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        engine.thread_queue.set_policy(policy, None),
    )
    .await
    .expect("HTTP fixture policy drain finishes")
    .unwrap();
    let guard = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        engine
            .thread_queue
            .acquire_user_slot(Uuid::new_v4(), "hold synthetic capacity".into()),
    )
    .await
    .expect("HTTP fixture acquires its only user slot");
    let scheduler = crate::scheduler::SchedulerManager::new(engine.clone(), pool.clone())
        .await
        .unwrap();
    let app = crate::api::create_router(
        engine.clone(),
        pool.clone(),
        engine.event_store().clone(),
        crate::memory::EmbedderSlot::empty(),
        None,
        engine.workspace_path().to_path_buf(),
        Arc::new(tokio::sync::Mutex::new(scheduler)),
        chrono::Utc::now(),
        &crate::net_config::BindChoice::Loopback,
    );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let url = format!(
        "http://{}/api/v1/chat/stream",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    // A people-only input must record while every agent slot is held.
    let people_thread = Uuid::new_v4();
    let people_id = Uuid::new_v4();
    let people_actor = MessageOrigin::Device {
        device_id: device.into(),
    };
    let people_request: ThreadQueueRequest = serde_json::from_value(json!({
        "type":"agent-chat", "mode":"human", "thread_id":people_thread,
        "event_id":people_id, "message":"synthetic user-only message", "origin":people_actor,
        "user_addressing":{"mentions":["synthetic-recipient"], "agent_mentioned":false},
    }))
    .unwrap();
    let people_intent = crate::engine::event_bus::OrdinarySubmissionIntent::new(
        people_thread,
        people_id,
        &people_request,
    )
    .unwrap();
    engine
        .event_bus
        .accept_ordinary_submission(
            people_request,
            people_actor,
            &engine.input_order,
            None,
            None,
            people_intent,
        )
        .await
        .unwrap();
    engine.thread_queue.drain().await;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let posted: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events WHERE id=$1 AND event_type='UserOnlyMessagePosted')")
                .bind(people_id).fetch_one(&pool).await.unwrap();
            if posted { break; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.expect("people-only content bypasses full agent capacity");
    let headers = crate::api::gateway_stamp::test_support::stamped_device_headers(device);
    let thread = Uuid::new_v4();
    let message = Uuid::new_v4();
    let body = json!({"mode":"human", "message":"ordinary accepted content", "thread_id":thread,
        "event_id":message, "new_thread":true, "input_chain":Uuid::new_v4(), "input_seq":1});
    let response = client
        .post(&url)
        .headers(headers.clone())
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let ack: serde_json::Value = response.json().await.unwrap();
    assert_eq!(status, 200, "{ack}");
    assert_eq!(ack["event_id"], message.to_string());
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_queue WHERE thread_id=$1")
        .bind(thread)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        pending, 1,
        "the ack owns durable membership before dispatch"
    );
    let recorded: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events WHERE id=$1)")
        .bind(message)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        !recorded,
        "capacity holds message recording, not acceptance"
    );
    sqlx::query("INSERT INTO thread_summaries (thread_id,title,source,message_count,last_activity,has_response,compose_text,compose_epoch) VALUES ($1,'Synthetic HTTP thread','chat',0,now(),false,'later draft',17)")
        .bind(thread).execute(&pool).await.unwrap();
    let replay = client
        .post(&url)
        .headers(headers.clone())
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.json::<serde_json::Value>().await.unwrap(), ack);
    let mut changed = body.clone();
    changed["message"] = json!("different content under the same identity");
    assert_eq!(
        client
            .post(&url)
            .headers(headers.clone())
            .json(&changed)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let mut stale = body.clone();
    stale["event_id"] = json!(Uuid::new_v4());
    stale["input_seq"] = json!(2);
    stale["compose_epoch"] = json!(16);
    let refused = client
        .post(&url)
        .headers(headers)
        .json(&stale)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 409);
    assert_eq!(
        refused.json::<serde_json::Value>().await.unwrap()["reason"],
        "compose_snapshot_stale"
    );
    let draft: (String, i64) = sqlx::query_as(
        "SELECT compose_text,compose_epoch FROM thread_summaries WHERE thread_id=$1",
    )
    .bind(thread)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(draft, ("later draft".into(), 17));
    let (legacy_thread, legacy_message): (Uuid, Uuid) = sqlx::query_as(
        "SELECT aggregate_id::uuid,id FROM events WHERE event_type='MessageReceived' AND payload->>'text'='Use the selected context'",
    ).fetch_one(&pool).await.unwrap();
    let legacy = client.post(&url).headers(crate::api::gateway_stamp::test_support::stamped_device_headers(device))
        .json(&json!({"mode":"human", "message":"Use the selected context", "thread_id":legacy_thread, "event_id":legacy_message}))
        .send().await.unwrap();
    assert_eq!(legacy.status(), 200);
    let legacy_ack: serde_json::Value = legacy.json().await.unwrap();
    assert_eq!(legacy_ack["event_id"], legacy_message.to_string());
    assert!(legacy_ack["compose"].is_null());
    let legacy_accepts: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type='OrdinarySubmissionAccepted' AND payload->'data'->>'message_event_id'=$1")
        .bind(legacy_message.to_string()).fetch_one(&pool).await.unwrap();
    assert_eq!(
        legacy_accepts, 0,
        "a pre-receipt message retry creates no new acceptance"
    );
    verify_urgent_question_http(&engine, &client, &url, device).await;
    drop(guard);
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let pending: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_queue WHERE thread_id=$1)")
                    .bind(thread)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            if !pending {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("accepted HTTP input completes through the real executor");
    let content: String = sqlx::query_scalar("SELECT payload->>'text' FROM events WHERE id=$1")
        .bind(message)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(content, "ordinary accepted content");
    server.abort();
}

async fn verify_urgent_question_http(
    engine: &std::sync::Arc<crate::engine::LucidosEngine>,
    client: &reqwest::Client,
    chat_url: &str,
    device: &str,
) {
    use crate::engine::thread_events::{AnswerKind, EventChannel};
    let pool = engine.pool();
    let thread = Uuid::new_v4();
    let actor = MessageOrigin::Device {
        device_id: device.into(),
    };
    open_turn(pool, thread, actor.clone()).await;
    let parsed = crate::engine::agent_session::parse_ask_user_question_inputs(&json!({
        "questions": [{ "question":"Synthetic question", "options":[{"label":"Continue"},{"label":"Stop"}] }]
    })).remove(0);
    let tool = format!("synthetic-question-{}", Uuid::new_v4());
    engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id: thread,
            event: ThreadEvent::UserQuestionAsked {
                tool_use_id: tool.clone(),
                cc_session_id: String::new(),
                question: parsed.question,
                options: parsed.options,
                worktree_path: None,
                multi_select: false,
                owner_approval: None,
                unattested_act: None,
                permission_request: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::Chat),
                actor: Some(actor),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    let epoch: i64 = sqlx::query_scalar("UPDATE thread_summaries SET compose_text='synthetic answer' WHERE thread_id=$1 RETURNING compose_epoch").bind(thread).fetch_one(pool).await.unwrap();
    let _waiting_turn = engine.input_order.join(thread);
    let mut waiter = engine.question_wait_registry.register(&tool).await;
    let url = format!(
        "{}/threads/{thread}/answer-question",
        chat_url.strip_suffix("/chat/stream").unwrap()
    );
    let id = Uuid::new_v4();
    let mut body = json!({"tool_use_id":tool,"event_id":id,"compose_epoch":epoch + 1,
        "answer":AnswerKind::FreeText {text:"synthetic answer".into(),image_hashes:vec![]}});
    let response = client
        .post(&url)
        .headers(crate::api::gateway_stamp::test_support::stamped_device_headers(device))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["reason"],
        "compose_snapshot_stale"
    );
    let answers: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(answers, 0);
    body["compose_epoch"] = json!(epoch);
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client
            .post(&url)
            .headers(crate::api::gateway_stamp::test_support::stamped_device_headers(device))
            .json(&body)
            .send(),
    )
    .await
    .expect("urgent question answer cannot wait for capacity or predecessors")
    .unwrap();
    assert_eq!(response.status(), 200);
    let receipt: serde_json::Value = response.json().await.unwrap();
    assert_eq!(receipt["compose"]["compose_epoch"], epoch + 1);
    let recorded: String = sqlx::query_scalar("SELECT event_type FROM events WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(recorded, "UserQuestionAnswered");
    waiter
        .try_recv()
        .expect("the urgent answer wakes its live question waiter");
    sqlx::query("UPDATE thread_summaries SET compose_text='newer answer draft' WHERE thread_id=$1")
        .bind(thread)
        .execute(pool)
        .await
        .unwrap();
    let response = client
        .post(&url)
        .headers(crate::api::gateway_stamp::test_support::stamped_device_headers(device))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    let draft: (String, i64) = sqlx::query_as(
        "SELECT compose_text,compose_epoch FROM thread_summaries WHERE thread_id=$1",
    )
    .bind(thread)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(draft, ("newer answer draft".into(), epoch + 1));
    crate::engine::thread_events::emit_response_canceled(
        &engine.event_bus,
        pool,
        thread,
        crate::engine::thread_events::CancelCause::UserStop,
        String::new(),
        vec![],
        None,
        None,
        EventMeta::NONE,
        "Synthetic question teardown",
    )
    .await;
    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id=$1")
            .bind(thread)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        status, "idle",
        "the synthetic asking turn releases its capacity"
    );
}

#[tokio::test]
async fn submission_service_resolves_chat_and_both_app_backends_and_replays_consumed_drafts() {
    use crate::engine::draft_submission::DraftSubmissionService;
    use crate::llm::model_registry::{empty, ModelRouting, ProviderKind};
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("data/apps/example")).unwrap();
    let registry = empty();
    registry.write().unwrap().insert(
        "test-model".into(),
        ModelRouting::single(ProviderKind::OpenAi, "gpt-5"),
    );
    let service = DraftSubmissionService {
        bus: &bus,
        pool: &pool,
        workspace: workspace.path(),
        defaults: crate::core::ModelDefaults {
            registry: &registry,
            fallback_model: "test-model",
        },
        configured: &[ProviderKind::OpenAi],
    };
    for backend in [None, Some("codex"), Some("claude-code")] {
        let mut prepared = fixture(&pool).await;
        let mode = if backend.is_some() {
            "claude_code"
        } else {
            "lucidos"
        };
        let selection = backend
            .map(|backend| json!({"scope":{"kind":"app","appId":"example"},"codingAgent":backend}))
            .unwrap_or(json!({}));
        sqlx::query("UPDATE thread_summaries SET state='composing',compose_mode=$2,compose_selection=$3 WHERE thread_id=$1")
            .bind(prepared.request.thread_id).bind(mode).bind(selection).execute(&pool).await.unwrap();
        refresh_snapshot(&pool, &mut prepared).await;
        let receipt = service
            .submit(
                prepared.request.clone(),
                prepared.actor.clone(),
                prepared.authority.clone(),
            )
            .await
            .unwrap();
        let queued: serde_json::Value =
            sqlx::query_scalar("SELECT request FROM thread_queue WHERE id=$1")
                .bind(receipt.queue_entry_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(queued["thread_id"], json!(prepared.request.thread_id));
        assert_eq!(queued["message"], "saved text");
        assert_eq!(queued["mode"], "human");
        assert_eq!(queued["use_coding_agent"], backend.is_some());
        assert_eq!(queued["cc_model"], serde_json::Value::Null);
        if let Some(backend) = backend {
            assert_eq!(queued["app_id"], "example");
            assert_eq!(queued["coding_agent"], backend);
        } else {
            assert_eq!(queued["model"], "test-model");
            assert_eq!(queued["provider"], "openai");
        }
        assert_eq!(
            service
                .submit(prepared.request, prepared.actor, prepared.authority)
                .await
                .unwrap(),
            receipt
        );
    }
    assert_eq!(counts(&pool).await, (3, 3, 3));
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn submission_service_refuses_ambiguous_channel_and_missing_destinations_without_mutation() {
    use crate::engine::draft_submission::DraftSubmissionService;
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let workspace = tempfile::tempdir().unwrap();
    let registry = crate::llm::model_registry::empty();
    let service = DraftSubmissionService {
        bus: &bus,
        pool: &pool,
        workspace: workspace.path(),
        defaults: crate::core::ModelDefaults {
            registry: &registry,
            fallback_model: "missing-model",
        },
        configured: &[],
    };
    for (mode, selection) in [
        (None, json!({})),
        (Some("claude_code"), json!({})),
        (
            Some("claude_code"),
            json!({"scope":{"kind":"external","repoId":Uuid::new_v4()}}),
        ),
        (
            Some("claude_code"),
            json!({"scope":{"kind":"app","appId":"missing"}}),
        ),
        (
            Some("claude_code"),
            json!({"scope":{"kind":"app","appId":"../outside"}}),
        ),
        (Some("lucidos"), json!({})),
    ] {
        let mut prepared = fixture(&pool).await;
        sqlx::query("UPDATE thread_summaries SET state='composing',compose_mode=$2,compose_selection=$3 WHERE thread_id=$1")
            .bind(prepared.request.thread_id).bind(mode).bind(selection).execute(&pool).await.unwrap();
        refresh_snapshot(&pool, &mut prepared).await;
        let request = prepared.request.clone();
        assert!(service
            .submit(request.clone(), prepared.actor, prepared.authority)
            .await
            .is_err());
        let raw: ComposeReportRow = sqlx::query_as(&format!(
            "SELECT {COMPOSE_REPORT_COLS} FROM thread_summaries WHERE thread_id=$1"
        ))
        .bind(request.thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            SavedDraftSnapshot::try_from(raw).unwrap(),
            prepared.snapshot
        );
        assert_eq!(counts(&pool).await, (0, 0, 0));
    }
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn submission_service_resolves_registered_destinations_and_strict_backend_defaults() {
    use crate::core::preference_catalog as prefs;
    use crate::core::repositories::RepositoryStore;
    use crate::engine::draft_submission::DraftSubmissionService;
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let workspace = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("data/apps/example")).unwrap();
    let source_repo = RepositoryStore::register(
        &pool,
        &bus,
        crate::engine::LucidosEngine::DEFAULT_REPO_NAME,
        source.path().to_str().unwrap(),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let repo = RepositoryStore::register(
        &pool,
        &bus,
        "example-repo",
        external.path().to_str().unwrap(),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let registry = crate::llm::model_registry::empty();
    let service = DraftSubmissionService {
        bus: &bus,
        pool: &pool,
        workspace: workspace.path(),
        defaults: crate::core::ModelDefaults {
            registry: &registry,
            fallback_model: "unused",
        },
        configured: &[],
    };
    // Defaults are read for each acceptance, while active bindings win over
    // account defaults. Omitted model/effort remain omitted queue overrides.
    for (scope, stored, active, expected) in [
        (json!({"kind":"lucidos"}), "codex", false, "codex"),
        (
            json!({"kind":"external","repoId":repo.id}),
            "claude-code",
            false,
            "claude-code",
        ),
        (
            json!({"kind":"app","appId":"example"}),
            "codex",
            true,
            "claude-code",
        ),
        (
            json!({"kind":"app","appId":"example"}),
            "unknown",
            false,
            "",
        ),
    ] {
        crate::test_support::seed_preference(&pool, prefs::CODING_AGENT_DEFAULT.key(), stored)
            .await
            .unwrap();
        let mut prepared = fixture(&pool).await;
        sqlx::query("UPDATE thread_summaries SET state=$2,source='claude_code',compose_mode='claude_code',compose_selection=$3,coding_agent_kind=$4,coding_agent_folder=$5 WHERE thread_id=$1")
            .bind(prepared.request.thread_id).bind(if active {"active"} else {"composing"})
            .bind(json!({"scope":scope})).bind(active.then_some("app"))
            .bind(active.then(|| workspace.path().join("data/apps/example").to_string_lossy().into_owned()))
            .execute(&pool).await.unwrap();
        refresh_snapshot(&pool, &mut prepared).await;
        let result = service
            .submit(prepared.request.clone(), prepared.actor, prepared.authority)
            .await;
        if expected.is_empty() {
            assert_eq!(
                result.unwrap_err().to_string(),
                "draft_coding_backend_unsupported"
            );
            let text: String =
                sqlx::query_scalar("SELECT compose_text FROM thread_summaries WHERE thread_id=$1")
                    .bind(prepared.request.thread_id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(text, "saved text");
        } else {
            let receipt = result.unwrap();
            let queued: serde_json::Value =
                sqlx::query_scalar("SELECT request FROM thread_queue WHERE id=$1")
                    .bind(receipt.queue_entry_id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(queued["coding_agent"], expected);
            assert!(queued["cc_model"].is_null());
            assert!(queued["reasoning_effort"].is_null());
            if scope["kind"] == "external" {
                assert_eq!(queued["repo_id"], repo.id.to_string());
            }
        }
    }
    // Source sessions persist their repository id, including before kind
    // stamping existed. A legacy external binding stays external.
    for (kind, bound_repo, scope) in [
        (Some("lucidos"), source_repo.id, json!({"kind":"lucidos"})),
        (None, source_repo.id, json!({"kind":"lucidos"})),
        (None, repo.id, json!({"kind":"external","repoId":repo.id})),
    ] {
        let mut prepared = fixture(&pool).await;
        sqlx::query("UPDATE thread_summaries SET state='active',source='claude_code',compose_mode='claude_code',compose_selection=$2,coding_agent_kind=$3,cc_repo_id=$4 WHERE thread_id=$1")
            .bind(prepared.request.thread_id)
            .bind(json!({"scope":scope}))
            .bind(kind)
            .bind(bound_repo.to_string())
            .execute(&pool).await.unwrap();
        refresh_snapshot(&pool, &mut prepared).await;
        let receipt = service
            .submit(prepared.request, prepared.actor, prepared.authority)
            .await
            .unwrap();
        let queued: serde_json::Value =
            sqlx::query_scalar("SELECT request FROM thread_queue WHERE id=$1")
                .bind(receipt.queue_entry_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        if bound_repo == source_repo.id {
            assert!(queued["repo_id"].is_null());
        } else {
            assert_eq!(queued["repo_id"], repo.id.to_string());
        }
    }
    // A removed source checkout must not disable unrelated app/external
    // targets in a packaged workspace with an old source registration.
    drop(source);
    crate::test_support::seed_preference(&pool, prefs::CODING_AGENT_DEFAULT.key(), "codex")
        .await
        .unwrap();
    for scope in [
        json!({"kind":"app","appId":"example"}),
        json!({"kind":"external","repoId":repo.id}),
    ] {
        let mut prepared = fixture(&pool).await;
        sqlx::query("UPDATE thread_summaries SET state='composing',compose_mode='claude_code',compose_selection=$2 WHERE thread_id=$1")
            .bind(prepared.request.thread_id).bind(json!({"scope":scope})).execute(&pool).await.unwrap();
        refresh_snapshot(&pool, &mut prepared).await;
        service
            .submit(prepared.request, prepared.actor, prepared.authority)
            .await
            .unwrap();
    }
    let mut prepared = fixture(&pool).await;
    sqlx::query("UPDATE thread_summaries SET state='composing',compose_mode='claude_code',compose_selection='{}' WHERE thread_id=$1")
        .bind(prepared.request.thread_id).execute(&pool).await.unwrap();
    refresh_snapshot(&pool, &mut prepared).await;
    let error = service
        .submit(prepared.request.clone(), prepared.actor, prepared.authority)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "draft_lucidos_source_missing");
    let raw: ComposeReportRow = sqlx::query_as(&format!(
        "SELECT {COMPOSE_REPORT_COLS} FROM thread_summaries WHERE thread_id=$1"
    ))
    .bind(prepared.request.thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        SavedDraftSnapshot::try_from(raw).unwrap(),
        prepared.snapshot
    );
    assert_eq!(counts(&pool).await, (8, 8, 8));
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn receipt_lookup_needs_no_current_draft_or_live_queue() {
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let prepared = fixture(&pool).await;
    let request = prepared.request.clone();
    let actor = prepared.actor.clone();
    assert_eq!(
        bus.saved_draft_receipt(&request, &actor).await.unwrap(),
        None
    );
    let receipt = bus.accept_saved_draft(prepared).await.unwrap();
    bus.emit(BusEvent::System(SystemEvent::ThreadQueueCompleted {
        entry_id: receipt.queue_entry_id,
    }))
    .await
    .unwrap();
    // The consumed slot is empty and cannot produce a SavedDraftSnapshot.
    let raw: ComposeReportRow = sqlx::query_as(&format!(
        "SELECT {COMPOSE_REPORT_COLS} FROM thread_summaries WHERE thread_id=$1"
    ))
    .bind(request.thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(SavedDraftSnapshot::try_from(raw).is_err());
    let restored = EventBus::new(pool.clone()).0;
    assert_eq!(
        restored
            .saved_draft_receipt(&request, &actor)
            .await
            .unwrap(),
        Some(receipt)
    );
    for change in ["thread", "revision", "epoch"] {
        let mut conflict = request.clone();
        match change {
            "thread" => conflict.thread_id = Uuid::new_v4(),
            "revision" => conflict.expected_revision += 1,
            _ => conflict.expected_epoch += 1,
        }
        assert_eq!(
            restored
                .saved_draft_receipt(&conflict, &actor)
                .await
                .unwrap_err()
                .to_string(),
            "draft_retry_key_conflict"
        );
    }
    let other_agent = MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Agent,
        source_thread_id: Some(Uuid::new_v4()),
        emitting_trigger_id: None,
    };
    assert!(restored
        .saved_draft_receipt(&request, &other_agent)
        .await
        .is_err());
    let human = MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Human,
        source_thread_id: actor.source_thread_id(),
        emitting_trigger_id: None,
    };
    assert!(restored
        .saved_draft_receipt(&request, &human)
        .await
        .is_err());
    assert_eq!(counts(&pool).await, (1, 0, 1));
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn active_thread_channel_and_backend_cannot_change_at_acceptance() {
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    for (source, bound_backend, requested_backend) in [
        ("chat", None, "codex"),
        ("claude_code", Some("codex"), "claude-code"),
        ("claude_code", None, "codex"),
        ("claude_code", Some("unknown-backend"), "claude-code"),
    ] {
        let mut prepared = fixture(&pool).await;
        sqlx::query("UPDATE thread_summaries SET state='active',source=$2,coding_agent=$3 WHERE thread_id=$1")
            .bind(prepared.request.thread_id).bind(source).bind(bound_backend)
            .execute(&pool).await.unwrap();
        refresh_snapshot(&pool, &mut prepared).await;
        let target = prepared.request.thread_id;
        let mut queue = serde_json::to_value(&prepared.queue_request).unwrap();
        queue["use_coding_agent"] = json!(true);
        queue["coding_agent"] = json!(requested_backend);
        prepared.queue_request = serde_json::from_value(queue).unwrap();
        assert!(bus.accept_saved_draft(prepared).await.is_err());
        let text: String =
            sqlx::query_scalar("SELECT compose_text FROM thread_summaries WHERE thread_id=$1")
                .bind(target)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(text, "saved text");
        assert_eq!(counts(&pool).await, (0, 0, 0));
    }
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn active_coding_threads_keep_current_and_legacy_backend_bindings() {
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    for (bound, requested) in [
        (Some("codex"), "codex"),
        (Some("claude-code"), "claude-code"),
        (None, "claude-code"),
    ] {
        let mut prepared = fixture(&pool).await;
        sqlx::query("UPDATE thread_summaries SET state='active',source='claude_code',coding_agent=$2 WHERE thread_id=$1")
            .bind(prepared.request.thread_id).bind(bound).execute(&pool).await.unwrap();
        refresh_snapshot(&pool, &mut prepared).await;
        let mut queue = serde_json::to_value(&prepared.queue_request).unwrap();
        queue["use_coding_agent"] = json!(true);
        queue["coding_agent"] = json!(requested);
        prepared.queue_request = serde_json::from_value(queue).unwrap();
        bus.accept_saved_draft(prepared).await.unwrap();
    }
    assert_eq!(counts(&pool).await, (3, 3, 3));
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn acceptance_refuses_changed_saved_settings_without_consumption() {
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    for (mode, selection, change) in [
        (
            "lucidos",
            json!({"model":"saved-model"}),
            json!({"model":"different"}),
        ),
        (
            "lucidos",
            json!({"reasoningEffort":"high"}),
            json!({"reasoning_effort":"low"}),
        ),
        (
            "lucidos",
            json!({"provider":"saved-provider"}),
            json!({"provider":"different"}),
        ),
        (
            "claude_code",
            json!({"ccModel":"saved-model"}),
            json!({"cc_model":"different"}),
        ),
        (
            "claude_code",
            json!({"ccReasoningEffort":"high"}),
            json!({"reasoning_effort":"low"}),
        ),
        ("lucidos", json!({}), json!({"use_coding_agent":true})),
        ("claude_code", json!({}), json!({"use_coding_agent":false})),
        (
            "claude_code",
            json!({}),
            json!({"cc_model":"default","coding_agent":"codex"}),
        ),
        (
            "claude_code",
            json!({}),
            json!({"reasoning_effort":"high","coding_agent":"codex"}),
        ),
        (
            "claude_code",
            json!({"codingAgent":"codex"}),
            json!({"coding_agent":"claude-code"}),
        ),
        (
            "lucidos",
            json!({}),
            json!({"spawning_event_id":Uuid::new_v4()}),
        ),
        ("lucidos", json!({}), json!({"integrated":true})),
        (
            "lucidos",
            json!({}),
            json!({"app_context":{"app_id":"injected"}}),
        ),
        ("lucidos", json!({}), json!({"file_context":"injected.rs"})),
        (
            "lucidos",
            json!({}),
            json!({"url_context":{"url":"https://example.com","content":"injected"}}),
        ),
        (
            "lucidos",
            json!({}),
            json!({"conflict_change_id":Uuid::new_v4()}),
        ),
        (
            "lucidos",
            json!({}),
            json!({"spawning_trigger_id":"synthetic-trigger"}),
        ),
        ("lucidos", json!({}), json!({"event_id":Uuid::nil()})),
    ] {
        let mut prepared = fixture(&pool).await;
        let raw: ComposeReportRow = sqlx::query_as(&format!(
            "UPDATE thread_summaries SET compose_mode=$2,compose_selection=$3 WHERE thread_id=$1 RETURNING {COMPOSE_REPORT_COLS}"
        )).bind(prepared.request.thread_id).bind(mode).bind(selection).fetch_one(&pool).await.unwrap();
        prepared.snapshot = SavedDraftSnapshot::try_from(raw).unwrap();
        prepared.request.expected_revision = prepared.snapshot.report().summary_version;
        let mut queue = serde_json::to_value(&prepared.queue_request).unwrap();
        queue["use_coding_agent"] = json!(mode == "claude_code");
        for (key, value) in change.as_object().unwrap() {
            queue[key] = value.clone();
        }
        prepared.queue_request = serde_json::from_value(queue).unwrap();
        let target = prepared.request.thread_id;
        let before = prepared.snapshot.report().clone();
        let error = bus.accept_saved_draft(prepared).await.unwrap_err();
        assert_eq!(error.to_string(), "draft_queue_payload_mismatch");
        let after: ComposeReportRow = sqlx::query_as(&format!(
            "SELECT {COMPOSE_REPORT_COLS} FROM thread_summaries WHERE thread_id=$1"
        ))
        .bind(target)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            SavedDraftSnapshot::try_from(after).unwrap().report(),
            &before
        );
        assert_eq!(counts(&pool).await, (0, 0, 0));
    }
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn acceptance_preserves_explicit_picks_and_allows_resolved_defaults() {
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    for (mode, selection, settings) in [
        (
            "lucidos",
            json!({"model":"saved-model","reasoningEffort":"high","provider":"saved-provider"}),
            json!({"model":"saved-model","reasoning_effort":"high","provider":"saved-provider"}),
        ),
        (
            "claude_code",
            json!({"ccModel":"default","codingAgent":"codex"}),
            json!({"cc_model":"default","reasoning_effort":null,"coding_agent":"codex"}),
        ),
        (
            "lucidos",
            json!({"model":null,"reasoningEffort":null,"provider":null}),
            json!({"model":"resolved-model","reasoning_effort":"medium","provider":"resolved-provider"}),
        ),
        (
            "claude_code",
            json!({"ccModel":null,"ccReasoningEffort":null,"codingAgent":null}),
            json!({"cc_model":null,"reasoning_effort":null,"coding_agent":"codex"}),
        ),
    ] {
        let mut prepared = fixture(&pool).await;
        let raw: ComposeReportRow = sqlx::query_as(&format!(
            "UPDATE thread_summaries SET compose_mode=$2,compose_selection=$3 WHERE thread_id=$1 RETURNING {COMPOSE_REPORT_COLS}"
        )).bind(prepared.request.thread_id).bind(mode).bind(selection).fetch_one(&pool).await.unwrap();
        prepared.snapshot = SavedDraftSnapshot::try_from(raw).unwrap();
        prepared.request.expected_revision = prepared.snapshot.report().summary_version;
        sqlx::query("UPDATE thread_summaries SET state='composing' WHERE thread_id=$1")
            .bind(prepared.request.thread_id)
            .execute(&pool)
            .await
            .unwrap();
        refresh_snapshot(&pool, &mut prepared).await;
        let mut queue = serde_json::to_value(&prepared.queue_request).unwrap();
        queue["use_coding_agent"] = json!(mode == "claude_code");
        for (key, value) in settings.as_object().unwrap() {
            queue[key] = value.clone();
        }
        prepared.queue_request = serde_json::from_value(queue).unwrap();
        let receipt = bus.accept_saved_draft(prepared).await.unwrap();
        let accepted: serde_json::Value =
            sqlx::query_scalar("SELECT request FROM thread_queue WHERE id=$1")
                .bind(receipt.queue_entry_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        for (key, value) in settings.as_object().unwrap() {
            assert_eq!(&accepted[key], value);
        }
    }
    assert_eq!(counts(&pool).await, (4, 4, 4));
    pool.close().await;
    teardown_test_db(&db).await;
}

async fn refresh_snapshot(pool: &sqlx::PgPool, prepared: &mut PreparedDraftSubmission) {
    let raw: ComposeReportRow = sqlx::query_as(&format!(
        "SELECT {COMPOSE_REPORT_COLS} FROM thread_summaries WHERE thread_id=$1"
    ))
    .bind(prepared.request.thread_id)
    .fetch_one(pool)
    .await
    .unwrap();
    prepared.snapshot = SavedDraftSnapshot::try_from(raw).unwrap();
    prepared.request.expected_revision = prepared.snapshot.report().summary_version;
    prepared.request.expected_epoch = prepared.snapshot.report().compose_epoch;
}

async fn fixture(pool: &sqlx::PgPool) -> PreparedDraftSubmission {
    let thread = Uuid::new_v4();
    let caller = Uuid::new_v4();
    let instruction_event_id = open_turn(
        pool,
        caller,
        MessageOrigin::Device {
            device_id: "test-owner".into(),
        },
    )
    .await;
    sqlx::query("INSERT INTO thread_summaries (thread_id,title,source,message_count,last_activity,has_response,compose_text) VALUES ($1,'Saved draft','chat',0,now(),false,'saved text')")
        .bind(thread).execute(pool).await.unwrap();
    let sql = format!("SELECT {COMPOSE_REPORT_COLS} FROM thread_summaries WHERE thread_id=$1");
    let raw: ComposeReportRow = sqlx::query_as(&sql)
        .bind(thread)
        .fetch_one(pool)
        .await
        .unwrap();
    let snapshot = SavedDraftSnapshot::try_from(raw).unwrap();
    let request = SubmitDraftRequest {
        thread_id: thread,
        expected_revision: snapshot.report().summary_version,
        expected_epoch: snapshot.report().compose_epoch,
        retry_key: Uuid::new_v4(),
    };
    let actor = MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Agent,
        source_thread_id: Some(caller),
        emitting_trigger_id: None,
    };
    let queue_request = ThreadQueueRequest::AgentChat {
        user_addressing: None,
        message: "saved text".into(),
        thread_id: thread,
        depth: 0,
        event_id: Some(Uuid::new_v4().to_string()),
        image_hashes: vec![],
        device_id: None,
        app_context: None,
        file_context: None,
        url_context: None,
        conflict_change_id: None,
        model: None,
        reasoning_effort: None,
        provider: None,
        use_coding_agent: Some(false),
        repo_id: None,
        cc_model: None,
        coding_agent: None,
        title: None,
        mode: ActorMode::Human,
        origin: None,
        parent_thread_id: None,
        spawning_event_id: None,
        wake_mode: None,
        app_id: None,
        integrated: false,
        spawning_trigger_id: None,
    };
    PreparedDraftSubmission {
        request,
        snapshot,
        queue_request,
        actor,
        authority: DraftSubmissionAuthority::StandingInstruction {
            instruction_event_id,
        },
    }
}

async fn open_turn(pool: &sqlx::PgPool, caller: Uuid, origin: MessageOrigin) -> Uuid {
    EventBus::new(pool.clone())
        .0
        .emit(BusEvent::Thread {
            thread_id: caller,
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: "instruction".into(),
                user_image_hashes: vec![],
                device_id: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: origin.mode(),
                model: None,
                reasoning_effort: None,
                origin: Some(origin),
            },
            meta: EventMeta {
                channel: Some(crate::engine::thread_events::EventChannel::Chat),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap()
        .unwrap()
        .event_id
}

fn copy(prepared: &PreparedDraftSubmission) -> PreparedDraftSubmission {
    PreparedDraftSubmission {
        request: prepared.request.clone(),
        snapshot: prepared.snapshot.clone(),
        queue_request: prepared.queue_request.clone(),
        actor: prepared.actor.clone(),
        authority: prepared.authority.clone(),
    }
}

async fn counts(pool: &sqlx::PgPool) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM draft_submissions), (SELECT count(*) FROM thread_queue), (SELECT count(*) FROM events WHERE event_type='DraftSubmissionAccepted')")
        .fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn saved_acceptance_retains_its_place_between_ordinary_inputs() {
    use crate::engine::InputAdmission;
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let prepared = fixture(&pool).await;
    let target = prepared.request.thread_id;
    let retry = copy(&prepared);
    let InputAdmission::Admitted(before) = bus
        .input_order
        .admit(&pool, target, None, None)
        .await
        .unwrap()
    else {
        panic!("unpositioned input refused")
    };
    let receipt = bus.accept_saved_draft(prepared).await.unwrap();
    let InputAdmission::Admitted(mut after) = bus
        .input_order
        .admit(&pool, target, None, None)
        .await
        .unwrap()
    else {
        panic!("unpositioned input refused")
    };
    assert!(bus.accepted_input_has_predecessor(receipt.queue_entry_id));
    let mut saved = bus
        .take_accepted_input_turn(receipt.queue_entry_id)
        .expect("acceptance retains recording order");
    assert!(saved.has_input_ahead());
    assert!(after.has_input_ahead());
    drop(before);
    saved.wait_for_predecessor().await;
    assert!(after.has_input_ahead(), "saved input still owns its place");
    assert_eq!(bus.accept_saved_draft(retry).await.unwrap(), receipt);
    assert!(
        bus.take_accepted_input_turn(receipt.queue_entry_id)
            .is_none(),
        "replay must not create a second turn"
    );
    drop(saved);
    assert!(
        after.has_input_ahead(),
        "unrecorded failure must retain the barrier"
    );
    let raw: serde_json::Value = sqlx::query_scalar("SELECT request FROM thread_queue WHERE id=$1")
        .bind(receipt.queue_entry_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: target,
        event: ThreadEvent::MessageReceived {
            text: raw["message"].as_str().unwrap().into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            provider: None,
            origin: Some(serde_json::from_value(raw["origin"].clone()).unwrap()),
            voice_session_id: None,
            wake_mode: None,
        },
        meta: EventMeta {
            event_id: Some(Uuid::parse_str(raw["event_id"].as_str().unwrap()).unwrap()),
            channel: Some(crate::engine::thread_events::EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    after.wait_for_predecessor().await;
    assert!(!after.has_input_ahead());
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn acceptance_commits_receipt_queue_payload_audit_and_consumption_together() {
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let prepared = fixture(&pool).await;
    let target = prepared.request.thread_id;
    let old_epoch = prepared.request.expected_epoch;
    let receipt = bus.accept_saved_draft(prepared).await.unwrap();
    assert_eq!(counts(&pool).await, (1, 1, 1));
    let identity: (String, String, Option<Uuid>) =
        sqlx::query_as("SELECT aggregate,aggregate_id,thread_id FROM events WHERE id=$1")
            .bind(receipt.event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        identity,
        ("thread".into(), target.to_string(), Some(target))
    );
    let compose: (String, i64) = sqlx::query_as(
        "SELECT compose_text,compose_epoch FROM thread_summaries WHERE thread_id=$1",
    )
    .bind(target)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(compose, (String::new(), old_epoch + 1));
    let payload: serde_json::Value = sqlx::query_scalar("SELECT payload FROM events WHERE id=$1")
        .bind(receipt.event_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(payload["data"]["snapshot"]["text"], "saved text");
    assert_eq!(payload["data"]["actor"]["kind"], "api");
    assert_eq!(payload["data"]["actor"]["mode"], "agent");
    assert_eq!(payload["data"]["authority"]["kind"], "standing-instruction");
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT request FROM thread_queue WHERE id=$1")
            .bind(receipt.queue_entry_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let restored: ThreadQueueRequest = serde_json::from_value(stored).unwrap();
    assert_eq!(restored.thread_id(), Some(target));
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn same_key_retry_survives_new_draft_and_new_bus_without_reconsuming() {
    let (pool, db) = setup_test_db().await;
    let prepared = fixture(&pool).await;
    let first = EventBus::new(pool.clone())
        .0
        .accept_saved_draft(copy(&prepared))
        .await
        .unwrap();
    sqlx::query("UPDATE thread_summaries SET compose_text='later draft' WHERE thread_id=$1")
        .bind(first.thread_id)
        .execute(&pool)
        .await
        .unwrap();
    let replayed = EventBus::new(pool.clone())
        .0
        .accept_saved_draft(prepared)
        .await
        .unwrap();
    assert_eq!(replayed, first);
    assert_eq!(counts(&pool).await, (1, 1, 1));
    let text: String =
        sqlx::query_scalar("SELECT compose_text FROM thread_summaries WHERE thread_id=$1")
            .bind(first.thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(text, "later draft");
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn different_keys_racing_one_revision_accept_exactly_once() {
    let (pool, db) = setup_test_db().await;
    let first = fixture(&pool).await;
    let mut second = copy(&first);
    second.request.retry_key = Uuid::new_v4();
    let a = EventBus::new(pool.clone()).0;
    let b = EventBus::new(pool.clone()).0;
    let (a, b) = tokio::join!(a.accept_saved_draft(first), b.accept_saved_draft(second));
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(counts(&pool).await, (1, 1, 1));
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn simultaneous_same_key_requests_return_the_same_receipt() {
    let (pool, db) = setup_test_db().await;
    let prepared = fixture(&pool).await;
    let retry = copy(&prepared);
    let a = EventBus::new(pool.clone()).0;
    let b = EventBus::new(pool.clone()).0;
    let (a, b) = tokio::join!(a.accept_saved_draft(prepared), b.accept_saved_draft(retry));
    assert_eq!(a.unwrap(), b.unwrap());
    assert_eq!(counts(&pool).await, (1, 1, 1));
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_retry_key_cannot_be_reused_for_another_target_or_submitter() {
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let prepared = fixture(&pool).await;
    let retry = copy(&prepared);
    let receipt = bus.accept_saved_draft(prepared).await.unwrap();
    let mut another = fixture(&pool).await;
    let target = another.request.thread_id;
    another.request.retry_key = receipt.retry_key;
    assert!(bus.accept_saved_draft(another).await.is_err());
    let mut other_caller = retry;
    other_caller.actor = MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Agent,
        source_thread_id: Some(Uuid::new_v4()),
        emitting_trigger_id: None,
    };
    assert!(bus.accept_saved_draft(other_caller).await.is_err());
    assert_eq!(counts(&pool).await, (1, 1, 1));
    let text: String =
        sqlx::query_scalar("SELECT compose_text FROM thread_summaries WHERE thread_id=$1")
            .bind(target)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(text, "saved text");
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn stale_selection_only_edit_refuses_without_mutation() {
    let (pool, db) = setup_test_db().await;
    let prepared = fixture(&pool).await;
    let thread = prepared.request.thread_id;
    sqlx::query("UPDATE thread_summaries SET compose_selection=$2 WHERE thread_id=$1")
        .bind(thread)
        .bind(json!({"model":"another-model"}))
        .execute(&pool)
        .await
        .unwrap();
    assert!(EventBus::new(pool.clone())
        .0
        .accept_saved_draft(prepared)
        .await
        .is_err());
    assert_eq!(counts(&pool).await, (0, 0, 0));
    let text: String =
        sqlx::query_scalar("SELECT compose_text FROM thread_summaries WHERE thread_id=$1")
            .bind(thread)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(text, "saved text");
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn projection_failure_rolls_back_receipt_event_and_compose() {
    let (pool, db) = setup_test_db().await;
    let prepared = fixture(&pool).await;
    let thread = prepared.request.thread_id;
    // Fault injection in this disposable DB only: fail the last projection
    // write, after the receipt and compose writes have happened.
    sqlx::query(
        "ALTER TABLE thread_queue ADD CONSTRAINT refuse_test_entry CHECK (summary <> 'saved text')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let bus = EventBus::new(pool.clone()).0;
    assert!(bus.accept_saved_draft(prepared).await.is_err());
    assert!(
        bus.accepted_input_turns.lock().unwrap().is_empty(),
        "rollback releases provisional ownership"
    );
    let mut next = bus.input_order.join(thread);
    assert!(
        !next.has_input_ahead(),
        "rollback leaves no stuck predecessor"
    );
    assert_eq!(counts(&pool).await, (0, 0, 0));
    let text: String =
        sqlx::query_scalar("SELECT compose_text FROM thread_summaries WHERE thread_id=$1")
            .bind(thread)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(text, "saved text");
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn completed_queue_entry_stays_completed_on_receipt_retry() {
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let prepared = fixture(&pool).await;
    let retry = copy(&prepared);
    let first = bus.accept_saved_draft(prepared).await.unwrap();
    bus.emit(BusEvent::System(SystemEvent::ThreadQueueCompleted {
        entry_id: first.queue_entry_id,
    }))
    .await
    .unwrap();
    assert_eq!(counts(&pool).await, (1, 0, 1));
    assert!(bus.take_accepted_input_turn(first.queue_entry_id).is_none());
    let mut next = bus.input_order.join(first.thread_id);
    assert!(
        !next.has_input_ahead(),
        "durable completion releases an undispatched turn"
    );
    assert_eq!(bus.accept_saved_draft(retry).await.unwrap(), first);
    assert_eq!(counts(&pool).await, (1, 0, 1));
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn deleting_acceptance_audit_cascades_its_receipt() {
    let (pool, db) = setup_test_db().await;
    let prepared = fixture(&pool).await;
    let receipt = EventBus::new(pool.clone())
        .0
        .accept_saved_draft(prepared)
        .await
        .unwrap();
    // Exercise the FK using the thread-deletion event predicate. The route's
    // own queue cleanup is covered by the family-deletion integration suite.
    sqlx::query(
        "DELETE FROM events WHERE thread_id=$1 OR (aggregate='thread' AND aggregate_id=$2)",
    )
    .bind(receipt.thread_id)
    .bind(receipt.thread_id.to_string())
    .execute(&pool)
    .await
    .unwrap();
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM draft_submissions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
    pool.close().await;
    teardown_test_db(&db).await;
}

async fn approval(pool: &sqlx::PgPool, prepared: &mut PreparedDraftSubmission, owner_card: bool) {
    use crate::engine::thread_events::{AnswerKind, OwnerApproval};
    let caller = prepared.actor.source_thread_id().unwrap();
    open_turn(pool, caller, prepared.actor.clone()).await;
    let parsed = crate::engine::agent_session::parse_ask_user_question_inputs(&json!({
        "questions": [{"question":"Submit the saved draft?", "options":[
            {"label":crate::api::owner_approval::ALLOW_ONCE_LABEL}, {"label":"Don't allow"}
        ]}]
    }))
    .remove(0);
    let option_id = parsed.options[0].id.clone();
    let tool_use_id = format!("toolu-{}#q0", Uuid::new_v4());
    let bus = EventBus::new(pool.clone()).0;
    bus.emit(BusEvent::Thread {
        thread_id: caller,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.clone(),
            cc_session_id: String::new(),
            question: parsed.question,
            options: parsed.options,
            worktree_path: None,
            multi_select: false,
            owner_approval: owner_card.then_some(OwnerApproval {
                verb: crate::engine::permission::Action::SubmitDraft,
                target_thread_id: Some(prepared.request.thread_id),
                target_workspace_id: None,
                snapshot_handle: None,
            }),
            unattested_act: None,
            permission_request: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: caller,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: tool_use_id.clone(),
            quiet: false,
            answer: AnswerKind::Selected { option_id },
        },
        meta: EventMeta::with_actor(Some(MessageOrigin::Device {
            device_id: "test-owner".into(),
        })),
    })
    .await
    .unwrap();
    prepared.authority = DraftSubmissionAuthority::OwnerApproval { tool_use_id };
}

#[tokio::test]
async fn standing_instruction_must_name_the_callers_current_owner_turn() {
    let (pool, db) = setup_test_db().await;
    let bus = EventBus::new(pool.clone()).0;
    let prepared = fixture(&pool).await;
    let mut forged = copy(&prepared);
    forged.authority = DraftSubmissionAuthority::StandingInstruction {
        instruction_event_id: Uuid::new_v4(),
    };
    assert!(bus.accept_saved_draft(forged).await.is_err());
    let caller = prepared.actor.source_thread_id().unwrap();
    open_turn(&pool, caller, prepared.actor.clone()).await;
    assert!(bus.accept_saved_draft(prepared).await.is_err());
    assert_eq!(counts(&pool).await, (0, 0, 0));
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_regular_card_never_authorizes_acceptance() {
    let (pool, db) = setup_test_db().await;
    let mut prepared = fixture(&pool).await;
    approval(&pool, &mut prepared, false).await;
    assert!(EventBus::new(pool.clone())
        .0
        .accept_saved_draft(prepared)
        .await
        .is_err());
    assert_eq!(counts(&pool).await, (0, 0, 0));
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn saved_draft_content_does_not_grant_a_fresh_owner_instruction() {
    let (pool, db) = setup_test_db().await;
    let prepared = fixture(&pool).await;
    let caller = prepared.actor.source_thread_id().unwrap();
    open_turn(
        &pool,
        caller,
        MessageOrigin::SavedDraft {
            submission_event_id: Uuid::new_v4(),
            submitted_by_thread_id: Uuid::new_v4(),
        },
    )
    .await;
    assert!(EventBus::new(pool.clone())
        .0
        .accept_saved_draft(prepared)
        .await
        .is_err());
    assert_eq!(counts(&pool).await, (0, 0, 0));
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn approval_spend_rolls_back_with_acceptance_and_retry_does_not_spend_again() {
    let (pool, db) = setup_test_db().await;
    let mut prepared = fixture(&pool).await;
    approval(&pool, &mut prepared, true).await;
    let bus = EventBus::new(pool.clone()).0;
    sqlx::query(
        "ALTER TABLE thread_queue ADD CONSTRAINT refuse_test_entry CHECK (summary <> 'saved text')",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(bus.accept_saved_draft(copy(&prepared)).await.is_err());
    assert_eq!(counts(&pool).await, (0, 0, 0));
    let spent: i64 =
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type='OwnerApprovalSpent'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(spent, 0);
    sqlx::query("ALTER TABLE thread_queue DROP CONSTRAINT refuse_test_entry")
        .execute(&pool)
        .await
        .unwrap();
    let receipt = bus.accept_saved_draft(copy(&prepared)).await.unwrap();
    assert_eq!(bus.accept_saved_draft(prepared).await.unwrap(), receipt);
    let spent: i64 =
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type='OwnerApprovalSpent'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(spent, 1);
    assert_eq!(counts(&pool).await, (1, 1, 1));
    pool.close().await;
    teardown_test_db(&db).await;
}
