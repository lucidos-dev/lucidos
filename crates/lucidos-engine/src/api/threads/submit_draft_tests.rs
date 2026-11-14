use super::*;
use crate::api::actor::{
    init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
};
use crate::api::app_reach::{app_may_call, entry_for, Reach, APP_ID_HEADER};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

const ROUTE: &str = "/threads/:thread_id/submit-draft";

fn token_headers(thread: Option<Uuid>) -> HeaderMap {
    init_agent_origin_secret("submit-draft-test-secret".to_string());
    let mut headers = HeaderMap::new();
    let token = mint_agent_origin_token(thread, 0, None).expect("the secret is installed");
    headers.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
    headers
}

#[test]
fn only_an_agent_inside_a_thread_may_submit() {
    let caller = Uuid::new_v4();
    assert_eq!(
        submitting_agent(&token_headers(Some(caller))).unwrap(),
        caller
    );
    let device = crate::api::gateway_stamp::test_support::stamped_device_headers("a-device");
    let mut local = HeaderMap::new();
    local.insert(
        lucidos_local_token::HEADER_LOCAL_TOKEN,
        "a-local-token".parse().unwrap(),
    );
    for headers in [token_headers(None), device, local, HeaderMap::new()] {
        let refused = submitting_agent(&headers).unwrap_err();
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
    }
}

#[test]
fn no_app_frame_may_reach_the_route() {
    assert!(matches!(entry_for(ROUTE), Some((Reach::Agent, &[]))));
    for method in ["GET", "POST"] {
        assert!(!app_may_call(ROUTE, method));
    }
}

/// The real router, inside the isolated engine child of
/// `accepted_saved_chat_executes_with_original_identity_and_saved_provenance`.
/// Covers both transports, retry, stale and every refused caller.
pub(crate) async fn verify_submit_draft_transports(engine: Arc<crate::engine::LucidosEngine>) {
    let pool = engine.pool().clone();
    // The engine's own registry decides the chat model, so it must serve the
    // mock provider's default.
    engine.model_registry().write().unwrap().insert(
        "mock".into(),
        crate::llm::model_registry::ModelRouting::single(crate::llm::ProviderKind::OpenAi, "mock"),
    );
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
    let base = format!("http://{}/api/v1/threads", listener.local_addr().unwrap());
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

    let caller = Uuid::new_v4();
    open_owner_turn(&engine, caller).await;
    let workspace = engine.workspace_path();
    let images: Vec<String> = [3u8, 4]
        .into_iter()
        .map(|color| {
            let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                1,
                1,
                image::Rgba([color, 0, 0, 255]),
            ));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
            crate::core::blobs::write_blob(workspace, &bytes.into_inner())
                .unwrap()
                .hash
        })
        .collect();
    let ordered = vec![images[1].clone(), images[0].clone()];
    let draft = seed_draft(&pool, "send exactly this\n", &ordered).await;
    let url = format!("{base}/{}/submit-draft", draft.0);
    let body = json!({"expected_revision": draft.1, "expected_epoch": draft.2});
    let agent = token_headers(Some(caller));
    let post = |headers: HeaderMap, body: serde_json::Value| {
        let request = client.post(&url).headers(headers).json(&body);
        async move {
            let response = request.send().await.unwrap();
            let status = response.status().as_u16();
            (status, response.json::<serde_json::Value>().await.unwrap())
        }
    };

    // I3: nobody but an agent inside a thread. An app frame is refused by
    // reach before the handler, even holding a token.
    let mut frame = agent.clone();
    frame.insert(APP_ID_HEADER, "habit-tracker".parse().unwrap());
    for (headers, status) in [
        (
            crate::api::gateway_stamp::test_support::stamped_device_headers("submit-test"),
            403,
        ),
        (token_headers(None), 403),
        (frame, 403),
        (HeaderMap::new(), 401),
    ] {
        assert_eq!(post(headers, body.clone()).await.0, status);
    }
    // I1: the body names a revision, never content.
    let mut smuggled = body.clone();
    smuggled["text"] = json!("something else");
    assert_eq!(post(agent.clone(), smuggled).await.0, 400);
    assert_eq!(counts(&pool, &[draft.0]).await, 0);

    let (status, receipt) = post(agent.clone(), body.clone()).await;
    assert_eq!(status, 202, "{receipt}");
    let queued: serde_json::Value =
        sqlx::query_scalar("SELECT request FROM thread_queue WHERE id=$1")
            .bind(Uuid::parse_str(receipt["queue_entry_id"].as_str().unwrap()).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(queued["message"], "send exactly this\n");
    assert_eq!(queued["image_hashes"], json!(ordered));
    assert!(queued["device_id"].is_null());
    let actor: serde_json::Value =
        sqlx::query_scalar("SELECT payload->'data'->'actor' FROM events WHERE id=$1")
            .bind(Uuid::parse_str(receipt["event_id"].as_str().unwrap()).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(actor["source_thread_id"], json!(caller));
    assert_ne!(actor["kind"], "device");

    // I4: the minted key comes back, and reusing it replays the receipt.
    let mut retry = body.clone();
    retry["retry_key"] = receipt["retry_key"].clone();
    assert_eq!(post(agent.clone(), retry).await, (202, receipt.clone()));
    // I5: a fresh key on the consumed revision is stale, with the next step.
    let (status, stale) = post(agent.clone(), body.clone()).await;
    assert_eq!(status, 409, "{stale}");
    assert_eq!(stale["reason"], "draft_snapshot_stale");
    assert!(stale["error"]
        .as_str()
        .unwrap()
        .contains("lucidos threads drafts --thread"));
    assert_eq!(counts(&pool, &[draft.0]).await, 1);

    // An unauthorized refusal names a next step the caller's kind of thread
    // has: only a coding agent is pointed at `ask-permission`.
    let unasked = seed_draft(&pool, "nobody authorized this", &[]).await;
    let unasked_url = format!("{base}/{}/submit-draft", unasked.0);
    let unasked_body = json!({"expected_revision": unasked.1, "expected_epoch": unasked.2});
    let coder = Uuid::new_v4();
    sqlx::query("INSERT INTO thread_summaries (thread_id, is_coding_agent, status) VALUES ($1, TRUE, 'idle')")
        .bind(coder)
        .execute(&pool)
        .await
        .unwrap();
    for (who, points_at_the_card) in [(Uuid::new_v4(), false), (coder, true)] {
        let response = client
            .post(&unasked_url)
            .headers(token_headers(Some(who)))
            .json(&unasked_body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 403);
        let refusal: serde_json::Value = response.json().await.unwrap();
        let message = refusal["error"].as_str().unwrap();
        assert_eq!(
            message.contains("ask-permission"),
            points_at_the_card,
            "{message}"
        );
    }
    assert_eq!(counts(&pool, &[unasked.0]).await, 0);

    // The tool reaches the same service, as the same kind of submitter.
    let second = seed_draft(&pool, "the tool's draft", &[]).await;
    let answer = engine
        .execute_tool(
            crate::llm::tool_names::THREADS,
            &json!({"action":"submit_draft", "thread_id": second.0.to_string(),
                "expected_revision": second.1, "expected_epoch": second.2}),
            "",
            Uuid::new_v4(),
            None,
            caller,
        )
        .await
        .unwrap();
    assert!(answer.contains(&second.0.to_string()), "{answer}");
    let refused = engine
        .execute_tool(
            crate::llm::tool_names::THREADS,
            &json!({"action":"submit_draft", "thread_id": second.0.to_string(),
                "expected_revision": second.1, "expected_epoch": second.2}),
            "",
            Uuid::new_v4(),
            None,
            Uuid::new_v4(),
        )
        .await
        .unwrap_err();
    assert!(refused.contains("draft_"), "{refused}");
    assert_eq!(counts(&pool, &[draft.0, second.0]).await, 2);
    server.abort();
}

/// A saved chat draft. Returns its thread, draft revision and epoch, read
/// back from the stored row.
async fn seed_draft(pool: &sqlx::PgPool, text: &str, images: &[String]) -> (Uuid, i64, i64) {
    let thread = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id,title,source,message_count,last_activity,\
         has_response,compose_text,compose_images,state,compose_mode) \
         VALUES ($1,'Saved draft','chat',0,now(),false,$2,$3,'composing','lucidos')",
    )
    .bind(thread)
    .bind(text)
    .bind(json!(images))
    .execute(pool)
    .await
    .unwrap();
    let (revision, epoch): (i64, i64) = sqlx::query_as(
        "SELECT draft_revision, compose_epoch FROM thread_summaries WHERE thread_id=$1",
    )
    .bind(thread)
    .fetch_one(pool)
    .await
    .unwrap();
    (thread, revision, epoch)
}

async fn open_owner_turn(engine: &crate::engine::LucidosEngine, caller: Uuid) {
    use crate::engine::event_bus::BusEvent;
    use crate::engine::thread_events::{EventChannel, EventMeta, MessageOrigin, ThreadEvent};
    let origin = MessageOrigin::Device {
        device_id: "submit-test-owner".into(),
    };
    engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id: caller,
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: "send my draft".into(),
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
                channel: Some(EventChannel::Chat),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
}

/// Receipts recorded for these drafts.
async fn counts(pool: &sqlx::PgPool, threads: &[Uuid]) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM draft_submissions WHERE thread_id = ANY($1)")
        .bind(threads)
        .fetch_one(pool)
        .await
        .unwrap()
}
