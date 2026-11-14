//! Real coding queue execution over isolated, deterministic CLI children.
use super::*;
use std::sync::Arc;

#[tokio::test]
async fn both_coding_backends_record_the_accepted_saved_input() {
    const CHILD: &str = "LUCIDOS_TEST_SAVED_CODING_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let home = tempfile::tempdir().unwrap();
        let scratch = home.path().join("tmp");
        std::fs::create_dir_all(&scratch).unwrap();
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            tokio::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "engine::event_bus::draft_submission::tests::coding_executor_tests::both_coding_backends_record_the_accepted_saved_input", "--nocapture"])
                .env(CHILD, "1")
                .env("HOME", home.path())
                // The home sits in the OS temp dir, so Landlock grants that
                // dir list-only. A fake agent reads the engine's image files
                // from a folder that already existed at its spawn.
                .env("TMPDIR", &scratch)
                .env("CLAUDE_CONFIG_DIR", home.path().join(".claude"))
                .env("CODEX_HOME", home.path().join(".codex"))
                .env("LUCIDOS_CLI_BIN", stub_cli_path(home.path()))
                .env("LUCIDOS_CODEX_PROTOCOL", "app-server")
                .env("GIT_AUTHOR_NAME", "Test User")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test User")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .env("RUST_MIN_STACK", "16777216")
                .env_remove("LUCIDOS_GATEWAY_DATA")
                .kill_on_drop(true)
                .output(),
        ).await.unwrap().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    install_fake_coding_binaries();
    let (pool, db) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("data/apps/example")).unwrap();
    std::fs::write(
        workspace.path().join("data/apps/example/index.html"),
        "<p>Synthetic app</p>",
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("data/apps/example/manifest.json"),
        r#"{"name":"Synthetic example","description":"Coding executor fixture"}"#,
    )
    .unwrap();
    let engine = new_coding_test_engine(workspace.path(), &db).await;
    crate::engine::git_ops::git_ran_ok(&["add", "data/apps/example"], workspace.path())
        .await
        .unwrap();
    crate::engine::git_ops::git_ran_ok(&["commit", "-m", "test fixture app"], workspace.path())
        .await
        .unwrap();
    let hashes: Vec<String> = [1, 2]
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
    let ordered = vec![hashes[1].clone(), hashes[0].clone(), hashes[1].clone()];
    let expected_images: Vec<String> = ordered
        .iter()
        .map(|hash| {
            crate::core::blobs::read_blob_as_base64(workspace.path(), hash)
                .unwrap()
                .0
        })
        .collect();
    for backend in ["claude-code", "codex"] {
        let mut prepared = fixture(&pool).await;
        for turn in 0..3 {
            if turn > 0 {
                let binding: (String, String, Option<String>) = sqlx::query_as(
                    "SELECT state,source,coding_agent FROM thread_summaries WHERE thread_id=$1",
                )
                .bind(prepared.request.thread_id)
                .fetch_one(&pool)
                .await
                .unwrap();
                assert_eq!(
                    binding,
                    ("active".into(), "claude_code".into(), Some(backend.into()))
                );
            }
            let reply = format!("saved-{backend}-{turn}");
            let text = if turn == 2 {
                String::new()
            } else {
                format!("Say exactly: \"{reply}\"")
            };
            sqlx::query("UPDATE thread_summaries SET state=$4,compose_text=$2,compose_mode='claude_code',compose_selection=$3,compose_images=$5 WHERE thread_id=$1")
            .bind(prepared.request.thread_id).bind(&text)
            .bind(json!({"scope":{"kind":"app","appId":"example"},"codingAgent":backend}))
            .bind(if turn == 0 { "composing" } else { "active" })
            .bind(json!(ordered))
            .execute(&pool).await.unwrap();
            refresh_snapshot(&pool, &mut prepared).await;
            let receipt = engine
                .submit_saved_draft(
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
            let message_id = Uuid::parse_str(queued["event_id"].as_str().unwrap()).unwrap();
            engine
                .clone()
                .execute_thread_queue_entry(crate::engine::thread_queue::ExecutableEntry {
                    id: receipt.queue_entry_id,
                    request: serde_json::from_value(queued).unwrap(),
                })
                .await
                .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let finished: bool = sqlx::query_scalar("SELECT count(*) >= $2 FROM events WHERE aggregate_id=$1 AND event_type IN ('ResponseGenerated','ResponseFailed')")
                    .bind(receipt.thread_id.to_string()).bind(i64::from(turn + 1)).fetch_one(&pool).await.unwrap();
                if finished { break; }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }).await.expect("fake coding turn must record a durable outcome");
            let messages: Vec<(Uuid, serde_json::Value)> = sqlx::query_as("SELECT id,payload FROM events WHERE aggregate_id=$1 AND event_type='MessageReceived' ORDER BY sequence")
            .bind(receipt.thread_id.to_string()).fetch_all(&pool).await.unwrap();
            assert_eq!(
                messages.len(),
                (turn + 1) as usize,
                "{backend}: one record per accepted input"
            );
            let recorded = messages.last().unwrap();
            assert_eq!(recorded.0, message_id);
            assert_eq!(recorded.1["text"], text);
            assert_eq!(recorded.1["origin"]["kind"], "saved_draft");
            assert!(recorded.1["device_id"].is_null());
            assert_eq!(recorded.1["user_image_hashes"], json!(ordered));
            let capture = capture_dir().join(format!("coding-images-{backend}.jsonl"));
            let captures: Vec<serde_json::Value> = std::fs::read_to_string(capture)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(captures.len(), (turn + 1) as usize);
            assert_eq!(
                captures.last().unwrap(),
                &json!(expected_images),
                "{backend}: process receives exact ordered bytes, including duplicates"
            );
            let responses: Vec<String> = sqlx::query_scalar("SELECT payload->>'text' FROM events WHERE aggregate_id=$1 AND event_type='ResponseGenerated' ORDER BY sequence")
            .bind(receipt.thread_id.to_string()).fetch_all(&pool).await.unwrap();
            assert_eq!(responses.len(), (turn + 1) as usize);
            for (index, response) in responses.iter().enumerate() {
                if index < 2 {
                    assert_eq!(response, &format!("saved-{backend}-{index}"));
                } else {
                    assert!(
                        !response.is_empty(),
                        "image-only input reaches a completed fake turn"
                    );
                }
            }
            let failures: Vec<serde_json::Value> = sqlx::query_scalar(
                "SELECT payload FROM events WHERE aggregate_id=$1 AND event_type='ResponseFailed'",
            )
            .bind(receipt.thread_id.to_string())
            .fetch_all(&pool)
            .await
            .unwrap();
            assert!(failures.is_empty(), "{backend}: {failures:?}");
            assert_eq!(
                engine
                    .submit_saved_draft(
                        prepared.request.clone(),
                        prepared.actor.clone(),
                        prepared.authority.clone()
                    )
                    .await
                    .unwrap(),
                receipt
            );
            prepared.request.retry_key = Uuid::new_v4();
        }
    }
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn both_saved_coding_backends_recover_after_the_accepting_process_exits() {
    const STAGE: &str = "LUCIDOS_TEST_SAVED_CODING_RECOVERY_STAGE";
    let Ok(stage) = std::env::var(STAGE) else {
        let (pool, db) = setup_test_db().await;
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        for stage in ["accept", "recover"] {
            let output = tokio::time::timeout(std::time::Duration::from_secs(120),
                tokio::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "engine::event_bus::draft_submission::tests::coding_executor_tests::both_saved_coding_backends_recover_after_the_accepting_process_exits", "--nocapture"])
                    .env(STAGE, stage)
                    .env("LUCIDOS_TEST_CODING_DB", &db)
                    .env("LUCIDOS_TEST_CODING_WORKSPACE", workspace.path())
                    .env("HOME", home.path())
                    .env("CLAUDE_CONFIG_DIR", home.path().join(".claude"))
                    .env("CODEX_HOME", home.path().join(".codex"))
                    .env("LUCIDOS_CLI_BIN", stub_cli_path(home.path()))
                .env("LUCIDOS_CODEX_PROTOCOL", "app-server")
                    .env("GIT_AUTHOR_NAME", "Test User").env("GIT_AUTHOR_EMAIL", "test@example.com")
                    .env("GIT_COMMITTER_NAME", "Test User").env("GIT_COMMITTER_EMAIL", "test@example.com")
                    .env("RUST_MIN_STACK", "16777216")
                    .env_remove("LUCIDOS_GATEWAY_DATA")
                    .kill_on_drop(true).output(),
            ).await.unwrap().unwrap();
            assert!(
                output.status.success(),
                "{stage}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        pool.close().await;
        teardown_test_db(&db).await;
        return;
    };
    let workspace =
        std::path::PathBuf::from(std::env::var_os("LUCIDOS_TEST_CODING_WORKSPACE").unwrap());
    let db = std::env::var("LUCIDOS_TEST_CODING_DB").unwrap();
    if stage == "accept" {
        install_fake_coding_binaries();
        std::fs::create_dir_all(workspace.join("data/apps/example")).unwrap();
        std::fs::write(
            workspace.join("data/apps/example/manifest.json"),
            r#"{"name":"Synthetic recovery app"}"#,
        )
        .unwrap();
        std::fs::write(
            workspace.join("data/apps/example/index.html"),
            "<p>Synthetic recovery</p>",
        )
        .unwrap();
    }
    let engine = new_coding_test_engine(&workspace, &db).await;
    let pool = engine.pool();
    let cases_path = workspace.join(".lucidos/coding-recovery.json");
    if stage == "accept" {
        crate::engine::git_ops::git_ran_ok(&["add", "data/apps/example"], &workspace)
            .await
            .unwrap();
        crate::engine::git_ops::git_ran_ok(
            &["commit", "-m", "test recovery fixture app"],
            &workspace,
        )
        .await
        .unwrap();
        let mut policy = engine.thread_queue.policy().await;
        policy.max_concurrent_coding_agent = 0;
        engine.thread_queue.set_policy(policy, None).await.unwrap();
        let mut cases = Vec::new();
        for backend in ["claude-code", "codex"] {
            let mut prepared = fixture(pool).await;
            let text = format!("Say exactly: \"recovered-{backend}\"");
            sqlx::query("UPDATE thread_summaries SET state='composing',compose_text=$2,compose_mode='claude_code',compose_selection=$3 WHERE thread_id=$1")
                .bind(prepared.request.thread_id).bind(&text)
                .bind(json!({"scope":{"kind":"app","appId":"example"},"codingAgent":backend}))
                .execute(pool).await.unwrap();
            refresh_snapshot(pool, &mut prepared).await;
            let receipt = engine
                .submit_saved_draft(
                    prepared.request.clone(),
                    prepared.actor.clone(),
                    prepared.authority.clone(),
                )
                .await
                .unwrap();
            let queued: serde_json::Value =
                sqlx::query_scalar("SELECT request FROM thread_queue WHERE id=$1")
                    .bind(receipt.queue_entry_id)
                    .fetch_one(pool)
                    .await
                    .unwrap();
            cases.push(json!({"backend":backend,"request":prepared.request,"actor":prepared.actor,"authority":prepared.authority,"receipt":receipt,"message_id":queued["event_id"],"text":text}));
        }
        let messages: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type='MessageReceived' AND payload->'origin'->>'kind'='saved_draft'").fetch_one(pool).await.unwrap();
        assert_eq!(
            messages, 0,
            "accepting process exits before recording or dispatch"
        );
        std::fs::write(cases_path, serde_json::to_vec(&cases).unwrap()).unwrap();
        return;
    }
    assert_eq!(stage, "recover");
    let cases: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(cases_path).unwrap()).unwrap();
    engine.thread_queue.begin_boot_recovery();
    engine.thread_queue.recover_persisted_entries().await;
    let mut policy = engine.thread_queue.policy().await;
    policy.max_concurrent_coding_agent = 2;
    engine.thread_queue.set_policy(policy, None).await.unwrap();
    engine.thread_queue.start_draining();
    tokio::time::timeout(std::time::Duration::from_secs(40), async {
        loop {
            let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_queue")
                .fetch_one(pool)
                .await
                .unwrap();
            if pending == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("fresh process drains both accepted coding inputs");
    for case in cases {
        let receipt: crate::core::draft_submission::DraftSubmissionReceipt =
            serde_json::from_value(case["receipt"].clone()).unwrap();
        let message: serde_json::Value =
            sqlx::query_scalar("SELECT payload FROM events WHERE id=$1")
                .bind(Uuid::parse_str(case["message_id"].as_str().unwrap()).unwrap())
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(message["text"], case["text"]);
        assert_eq!(message["origin"]["kind"], "saved_draft");
        let responses: Vec<Option<String>> = sqlx::query_scalar("SELECT payload->>'text' FROM events WHERE aggregate_id=$1 AND event_type='ResponseGenerated'")
            .bind(receipt.thread_id.to_string()).fetch_all(pool).await.unwrap();
        assert_eq!(
            responses,
            vec![Some(format!(
                "recovered-{}",
                case["backend"].as_str().unwrap()
            ))]
        );
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE aggregate_id=$1 AND event_type='MessageReceived'",
        )
        .bind(receipt.thread_id.to_string())
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
        sqlx::query("UPDATE thread_summaries SET compose_text='newer after recovery',compose_epoch=compose_epoch+1 WHERE thread_id=$1").bind(receipt.thread_id).execute(pool).await.unwrap();
        let replay = engine
            .submit_saved_draft(
                serde_json::from_value(case["request"].clone()).unwrap(),
                serde_json::from_value(case["actor"].clone()).unwrap(),
                serde_json::from_value(case["authority"].clone()).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay, receipt);
        let draft: String =
            sqlx::query_scalar("SELECT compose_text FROM thread_summaries WHERE thread_id=$1")
                .bind(receipt.thread_id)
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(draft, "newer after recovery");
    }
}

async fn new_coding_test_engine(
    workspace: &std::path::Path,
    db: &str,
) -> Arc<crate::engine::LucidosEngine> {
    let engine = Arc::new(
        crate::engine::LucidosEngine::new(
            workspace.to_path_buf(),
            &crate::test_support::test_db_url(db),
            crate::llm::ProviderBuildContext {
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
    engine
}

#[tokio::test]
async fn saved_coding_followups_preserve_live_backend_routing_and_identity() {
    const CHILD: &str = "LUCIDOS_TEST_SAVED_CODING_LIVE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let home = tempfile::tempdir().unwrap();
        let output = tokio::time::timeout(std::time::Duration::from_secs(120),
            tokio::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "engine::event_bus::draft_submission::tests::coding_executor_tests::saved_coding_followups_preserve_live_backend_routing_and_identity", "--nocapture"])
                .env(CHILD, "1").env("HOME", home.path())
                .env("CLAUDE_CONFIG_DIR", home.path().join(".claude")).env("CODEX_HOME", home.path().join(".codex"))
                .env("LUCIDOS_CLI_BIN", stub_cli_path(home.path()))
                .env("LUCIDOS_CODEX_PROTOCOL", "app-server")
                .env("GIT_AUTHOR_NAME", "Test User").env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test User").env("GIT_COMMITTER_EMAIL", "test@example.com")
                .env("RUST_MIN_STACK", "16777216").env_remove("LUCIDOS_GATEWAY_DATA")
                .kill_on_drop(true).output(),
        ).await.unwrap().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    install_fake_coding_binaries();
    let (pool, db) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("data/apps/example")).unwrap();
    std::fs::write(
        workspace.path().join("data/apps/example/manifest.json"),
        r#"{"name":"Synthetic live app"}"#,
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("data/apps/example/index.html"),
        "<p>Synthetic live input</p>",
    )
    .unwrap();
    let engine = new_coding_test_engine(workspace.path(), &db).await;
    crate::engine::git_ops::git_ran_ok(&["add", "data/apps/example"], workspace.path())
        .await
        .unwrap();
    crate::engine::git_ops::git_ran_ok(
        &["commit", "-m", "test live fixture app"],
        workspace.path(),
    )
    .await
    .unwrap();
    for backend in ["claude-code", "codex"] {
        let mut prepared = fixture(&pool).await;
        let old_text = format!("Take 60 seconds. Say exactly: \"old-{backend}\"");
        sqlx::query("UPDATE thread_summaries SET state='composing',compose_text=$2,compose_mode='claude_code',compose_selection=$3 WHERE thread_id=$1")
            .bind(prepared.request.thread_id).bind(old_text).bind(json!({"scope":{"kind":"app","appId":"example"},"codingAgent":backend})).execute(&pool).await.unwrap();
        refresh_snapshot(&pool, &mut prepared).await;
        let first = engine
            .submit_saved_draft(
                prepared.request.clone(),
                prepared.actor.clone(),
                prepared.authority.clone(),
            )
            .await
            .unwrap();
        let queued: serde_json::Value =
            sqlx::query_scalar("SELECT request FROM thread_queue WHERE id=$1")
                .bind(first.queue_entry_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let old_id = Uuid::parse_str(queued["event_id"].as_str().unwrap()).unwrap();
        let old_engine = engine.clone();
        let old = tokio::spawn(async move {
            old_engine
                .execute_thread_queue_entry(crate::engine::thread_queue::ExecutableEntry {
                    id: first.queue_entry_id,
                    request: serde_json::from_value(queued).unwrap(),
                })
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let read: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events WHERE aggregate_id=$1 AND event_type='CodingAgentInputRead' AND payload->>'input_event_id'=$2)")
                    .bind(prepared.request.thread_id.to_string()).bind(old_id.to_string()).fetch_one(&pool).await.unwrap();
                if read { break; }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }).await.expect("fake coding process reads the first input before the redirect");
        let text = format!("Say exactly: \"new-{backend}\"");
        sqlx::query("UPDATE thread_summaries SET compose_text=$2,compose_mode='claude_code',compose_selection=$3 WHERE thread_id=$1")
            .bind(prepared.request.thread_id).bind(&text).bind(json!({"scope":{"kind":"app","appId":"example"},"codingAgent":backend})).execute(&pool).await.unwrap();
        prepared.request.retry_key = Uuid::new_v4();
        refresh_snapshot(&pool, &mut prepared).await;
        let next = engine
            .submit_saved_draft(prepared.request, prepared.actor, prepared.authority)
            .await
            .unwrap();
        let queued: serde_json::Value =
            sqlx::query_scalar("SELECT request FROM thread_queue WHERE id=$1")
                .bind(next.queue_entry_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let next_id = Uuid::parse_str(queued["event_id"].as_str().unwrap()).unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            engine.clone().execute_thread_queue_entry(
                crate::engine::thread_queue::ExecutableEntry {
                    id: next.queue_entry_id,
                    request: serde_json::from_value(queued).unwrap(),
                },
            ),
        )
        .await
        .expect("saved follow-up interrupts the held coding turn")
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), old)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let terminal: (i64, String) = sqlx::query_as("SELECT sequence,event_type FROM events WHERE aggregate_id=$1 AND event_type IN ('ResponseCanceled','ResponseGenerated','ResponseFailed') AND payload->>'request_event_id'=$2 ORDER BY sequence LIMIT 1")
            .bind(next.thread_id.to_string()).bind(old_id.to_string()).fetch_one(&pool).await.unwrap();
        assert_eq!(
            terminal.1,
            if backend == "codex" {
                "ResponseCanceled"
            } else {
                "ResponseGenerated"
            }
        );
        let recorded: (i64, serde_json::Value) =
            sqlx::query_as("SELECT sequence,payload FROM events WHERE id=$1")
                .bind(next_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        if backend == "codex" {
            assert!(
                terminal.0 < recorded.0,
                "Codex redirect records the old terminal before the new message"
            );
        } else {
            assert!(
                recorded.0 < terminal.0,
                "Claude Code forwards the accepted input while the old turn is live"
            );
        }
        assert_eq!(recorded.1["text"], text);
        assert_eq!(recorded.1["origin"]["kind"], "saved_draft");
        // A running coding session deliberately shares its original response
        // anchor across inputs. Input identities and sequence order distinguish
        // these turns; do not impose a different terminal-anchor contract.
        let replies: Vec<(i64, String)> = sqlx::query_as("SELECT sequence,payload->>'text' FROM events WHERE aggregate_id=$1 AND event_type='ResponseGenerated' ORDER BY sequence")
            .bind(next.thread_id.to_string()).fetch_all(&pool).await.unwrap();
        let expected = if backend == "claude-code" {
            vec![format!("old-{backend}"), format!("new-{backend}")]
        } else {
            vec![format!("new-{backend}")]
        };
        assert_eq!(
            replies
                .iter()
                .map(|(_, text)| text.clone())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(replies.last().unwrap().0 > recorded.0);
        let reads: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE aggregate_id=$1 AND event_type='CodingAgentInputRead' AND payload->>'input_event_id'=$2")
            .bind(next.thread_id.to_string()).bind(next_id.to_string()).fetch_one(&pool).await.unwrap();
        assert_eq!(
            reads, 1,
            "the process reads the accepted follow-up identity exactly once"
        );
        let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM events WHERE aggregate_id=$1 AND event_type='MessageReceived' ORDER BY sequence").bind(next.thread_id.to_string()).fetch_all(&pool).await.unwrap();
        assert_eq!(ids, vec![old_id, next_id]);
    }
    pool.close().await;
    teardown_test_db(&db).await;
}

/// The `lucidos` CLI a coding session requires. A runner builds only the engine
/// crate, so no real one sits beside the test binary or on PATH.
fn stub_cli_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".local/bin/lucidos")
}

/// Where the fake agents record the image bytes they receive. The agent runs
/// in the OS sandbox, which refuses a new file directly in `$HOME` on Linux
/// but allows one inside `~/.cache`.
fn capture_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".cache")
}

fn install_fake_coding_binaries() {
    use std::os::unix::fs::PermissionsExt;
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    std::fs::create_dir_all(capture_dir()).unwrap();
    let bin = home.join(".local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(stub_cli_path(&home), "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(stub_cli_path(&home), std::fs::Permissions::from_mode(0o755)).unwrap();
    let claude = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../lucidos-app/e2e/fake-claude-code/claude.mjs"
    ));
    let claude = claude.replace(
        "import { realpathSync } from 'node:fs';",
        "import { realpathSync, appendFileSync } from 'node:fs';",
    );
    let capture = r#"let capturedInput = '';
let observedFollowup = false;
if (!process.argv.includes('--version')) process.stdin.on('data', chunk => {
  capturedInput += chunk.toString();
  let end;
  while ((end = capturedInput.indexOf('\n')) >= 0) {
    const line = capturedInput.slice(0, end); capturedInput = capturedInput.slice(end + 1);
    const frame = JSON.parse(line);
    if (frame.type === 'user' && JSON.stringify(frame.message).includes('new-claude-code')) observedFollowup = true;
    if (frame.type === 'user' && Array.isArray(frame.message?.content)) {
      const images = frame.message.content.filter(item => item.type === 'image').map(item => item.source.data);
      if (images.length) appendFileSync(process.env.HOME + '/.cache/coding-images-claude-code.jsonl', JSON.stringify(images) + '\n');
    }
  }
});
"#;
    let claude = claude.replace(
        "export const FAKE_VERSION",
        &(capture.to_string() + "export const FAKE_VERSION"),
    );
    let claude = claude.replace(
        "const timer = setTimeout(() => done(false), delayMs);",
        "const timer = delayMs === 60000 ? setInterval(() => { if (observedFollowup) { clearInterval(timer); done(false); } }, 20) : setTimeout(() => done(false), delayMs);",
    );
    std::fs::write(bin.join("fake-claude.mjs"), claude).unwrap();
    std::os::unix::fs::symlink(bin.join("fake-claude.mjs"), bin.join("claude")).unwrap();
    std::fs::write(bin.join("codex"), r##"#!/usr/bin/env node
const { createInterface } = require('node:readline');
const { appendFileSync, readFileSync } = require('node:fs');
if (process.argv.includes('--version')) { console.log('codex-cli 0.139.0'); process.exit(0); }
const send = (frame) => console.log(JSON.stringify(frame));
let pendingTurnTimer;
createInterface({input:process.stdin}).on('line', raw => {
  const frame = JSON.parse(raw);
  const id = frame.id;
  if (frame.method === 'initialize') send({id,result:{userAgent:'synthetic-codex/0.139.0'}});
  else if (frame.method === 'thread/start' || frame.method === 'thread/resume') send({id,result:{thread:{id:'synthetic-session'},model:'gpt-5.5'}});
  else if (frame.method === 'turn/start') {
    const images = frame.params.input.filter(item => item.type === 'localImage').map(item => readFileSync(item.path).toString('base64'));
    if (images.length) appendFileSync(process.env.HOME + '/.cache/coding-images-codex.jsonl', JSON.stringify(images) + '\n');
    const text = frame.params.input.filter(item => item.type === 'text').map(item => item.text).join('\n');
    const reply = /Say exactly:\s*"([^"]*)"/.exec(text)?.[1] ?? 'no synthetic reply rule';
    send({id,result:{turn:{id:'synthetic-turn',items:[],status:'inProgress'}}});
    send({method:'turn/started',params:{threadId:'synthetic-session',turn:{id:'synthetic-turn',items:[],status:'inProgress'}}});
    send({method:'item/agentMessage/delta',params:{threadId:'synthetic-session',turnId:'synthetic-turn',itemId:'message',delta:reply}});
    send({method:'item/completed',params:{threadId:'synthetic-session',turnId:'synthetic-turn',completedAtMs:1,item:{id:'message',type:'agentMessage',text:reply}}});
    const delay = Number(/Take (\d+) seconds/.exec(text)?.[1] ?? 0) * 1000;
    pendingTurnTimer = setTimeout(() => send({method:'turn/completed',params:{threadId:'synthetic-session',turn:{id:'synthetic-turn',items:[],status:'completed'}}}), delay);
  } else if (frame.method === 'turn/interrupt') {
    clearTimeout(pendingTurnTimer);
    send({id,result:{}});
    send({method:'turn/completed',params:{threadId:'synthetic-session',turn:{id:'synthetic-turn',items:[],status:'interrupted'}}});
  } else if (id !== undefined) send({id,result:{}});
});
"##).unwrap();
    for name in ["claude", "codex"] {
        std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}
