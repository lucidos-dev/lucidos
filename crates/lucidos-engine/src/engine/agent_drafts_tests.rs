use super::*;
use crate::core::draft_submission::SAVED_DRAFT_COLS;
use crate::engine::event_bus::SystemEvent;
use crate::llm::model_registry::{empty, ModelRegistry, ModelRouting};
use crate::test_support::{seed_coding_thread, setup_test_db, teardown_test_db};
use sqlx::PgPool;

/// A PNG whose trailing byte varies, so each seed is its own blob.
fn png(seed: u8) -> Vec<u8> {
    let mut bytes = vec![
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D',
        b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00,
    ];
    bytes.push(seed);
    bytes
}

struct Bench {
    pool: PgPool,
    db: String,
    bus: EventBus,
    store: EventStore,
    workspace: tempfile::TempDir,
    registry: ModelRegistry,
    caller: Uuid,
}

impl Bench {
    async fn new() -> Self {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let caller = seed_coding_thread(&bus, None).await;
        let workspace = tempfile::tempdir().unwrap();
        let artifacts = workspace.path().join("data/artifacts");
        std::fs::create_dir_all(&artifacts).unwrap();
        std::fs::write(artifacts.join("first.png"), png(1)).unwrap();
        std::fs::write(artifacts.join("second.png"), png(2)).unwrap();
        std::fs::write(artifacts.join("notes.txt"), "not an image").unwrap();
        std::fs::create_dir_all(workspace.path().join("data/apps/habit-tracker")).unwrap();
        let registry = empty();
        registry.write().unwrap().insert(
            "test-model".into(),
            ModelRouting::single(ProviderKind::OpenAi, "gpt-5"),
        );
        Self {
            store: EventStore::new(pool.clone()),
            pool,
            db,
            bus,
            workspace,
            registry,
            caller,
        }
    }

    async fn create(&self, request: CreateDraftRequest) -> Result<CreatedDraft, DraftRefusal> {
        AgentDraftService {
            bus: &self.bus,
            store: &self.store,
            pool: &self.pool,
            workspace: self.workspace.path(),
            workspace_name: "myws",
            defaults: ModelDefaults {
                registry: &self.registry,
                fallback_model: "test-model",
            },
            configured: &[ProviderKind::OpenAi],
        }
        .create(request, self.caller, self.actor())
        .await
    }

    fn actor(&self) -> MessageOrigin {
        MessageOrigin::Api {
            user_agent: None,
            mode: crate::engine::thread_events::ActorMode::Agent,
            source_thread_id: Some(self.caller),
            emitting_trigger_id: None,
        }
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.pool).await.unwrap()
    }

    fn blob_count(&self) -> usize {
        let Ok(shards) = std::fs::read_dir(self.workspace.path().join("data/blobs")) else {
            return 0;
        };
        shards
            .filter_map(Result::ok)
            .filter_map(|shard| std::fs::read_dir(shard.path()).ok())
            .map(|files| files.filter_map(Result::ok).count())
            .sum()
    }

    async fn close(self) {
        self.pool.close().await;
        teardown_test_db(&self.db).await;
    }
}

fn chat(text: &str, key: &str) -> CreateDraftRequest {
    CreateDraftRequest {
        text: text.into(),
        images: vec![],
        mode: "lucidos".into(),
        repo_id: None,
        app_id: None,
        coding_agent: None,
        model: None,
        provider: None,
        effort: None,
        retry_key: key.into(),
    }
}

/// I11 and I12: the draft lands exactly, as the send reads it, and nothing
/// navigates, messages, queues or starts.
#[tokio::test]
async fn a_draft_lands_exactly_and_silently() {
    let bench = Bench::new().await;
    let mut rx = bench.bus.subscribe();
    let events_before = bench.count("SELECT count(*) FROM events").await;
    let request = CreateDraftRequest {
        images: vec!["artifacts/second.png".into(), "artifacts/first.png".into()],
        model: Some("test-model".into()),
        ..chat("Here is the weekly summary.\n", "weekly")
    };

    let created = bench.create(request).await.unwrap();
    assert!(created.created);
    let thread_id = draft_thread_id(bench.caller, "weekly");
    assert_eq!(created.draft.thread_id, thread_id.to_string());
    assert_eq!(created.draft.drafted_by, Some(bench.caller.to_string()));
    assert_eq!(created.draft.link, format!("thread:myws/{thread_id}"));

    let raw: SavedDraftRow = sqlx::query_as(&format!(
        "SELECT {SAVED_DRAFT_COLS} FROM thread_summaries WHERE thread_id = $1"
    ))
    .bind(thread_id)
    .fetch_one(&bench.pool)
    .await
    .unwrap();
    let snapshot = SavedDraftSnapshot::try_from(raw).expect("the send accepts the row");
    let hashes = vec![
        crate::core::blobs::compute_hash(&png(2)),
        crate::core::blobs::compute_hash(&png(1)),
    ];
    assert_eq!(snapshot.report().text, "Here is the weekly summary.\n");
    assert_eq!(snapshot.report().image_hashes, hashes, "order is kept");
    assert_eq!(snapshot.report().mode.as_deref(), Some("lucidos"));
    assert_eq!(snapshot.selection().model.as_deref(), Some("test-model"));
    snapshot.resolve_images(bench.workspace.path()).unwrap();

    let (state, initiator): (String, String) =
        sqlx::query_as("SELECT state, initiator FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&bench.pool)
            .await
            .unwrap();
    assert_eq!(
        (state.as_str(), initiator.as_str()),
        ("composing", "system")
    );

    let written: Vec<String> =
        sqlx::query_scalar("SELECT event_type FROM events WHERE thread_id = $1 ORDER BY sequence")
            .bind(thread_id)
            .fetch_all(&bench.pool)
            .await
            .unwrap();
    assert_eq!(written, vec!["ThreadStarted"]);
    assert_eq!(
        bench.count("SELECT count(*) FROM events").await,
        events_before + 1,
        "one event in the whole log"
    );
    assert_eq!(bench.count("SELECT count(*) FROM thread_queue").await, 0);

    let mut announced = false;
    while let Ok(emitted) = rx.try_recv() {
        match &emitted.typed {
            BusEvent::System(SystemEvent::ThreadComposeChanged { id, .. }) if *id == thread_id => {
                announced = true
            }
            BusEvent::Thread {
                event: ThreadEvent::NavigationRequested { .. } | ThreadEvent::MessageReceived { .. },
                ..
            } => panic!("a draft never navigates or messages"),
            _ => {}
        }
    }
    assert!(announced, "every device hears the new draft");

    let sent = crate::test_support::message_received("Here is the weekly summary.\n");
    crate::test_support::seed_thread_event(&bench.bus, thread_id, sent).await;
    let initiator: String =
        sqlx::query_scalar("SELECT initiator FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&bench.pool)
            .await
            .unwrap();
    assert_eq!(
        initiator, "user",
        "the owner's send makes the thread theirs"
    );
    bench.close().await;
}

/// I12: a retry with the same key returns the draft as edited, and writes nothing.
#[tokio::test]
async fn a_retry_never_overwrites_an_edit() {
    let bench = Bench::new().await;
    bench.create(chat("first words", "key")).await.unwrap();
    let thread_id = draft_thread_id(bench.caller, "key");
    sqlx::query(
        "UPDATE thread_summaries SET compose_text = 'edited by the owner' WHERE thread_id = $1",
    )
    .bind(thread_id)
    .execute(&bench.pool)
    .await
    .unwrap();

    let retry = bench.create(chat("other words", "key")).await.unwrap();
    assert!(!retry.created);
    assert_eq!(retry.retry_key, "key");
    assert_eq!(retry.draft.text.as_deref(), Some("edited by the owner"));
    assert_eq!(
        bench
            .count("SELECT count(*) FROM events WHERE event_type = 'ThreadStarted' AND thread_id IN (SELECT thread_id FROM thread_summaries WHERE drafted_by_thread_id IS NOT NULL)")
            .await,
        1
    );

    let other = bench
        .create(chat("other words", "another key"))
        .await
        .unwrap();
    assert!(other.created, "a new key is a new draft");
    bench.close().await;
}

/// I13: every refusal writes no row, no event and no blob.
#[tokio::test]
async fn invalid_input_writes_nothing() {
    let bench = Bench::new().await;
    let image = |path: &str| CreateDraftRequest {
        images: vec!["artifacts/first.png".into(), path.into()],
        ..chat("text", "k")
    };
    let cases = vec![
        (
            "mode",
            CreateDraftRequest {
                mode: "email".into(),
                ..chat("text", "k")
            },
        ),
        ("empty", chat("  \n", "k")),
        ("blank key", chat("text", "  ")),
        ("traversal", image("artifacts/../.env")),
        ("outside data prefixes", image(".env")),
        ("not an image", image("artifacts/notes.txt")),
        ("missing image", image("artifacts/absent.png")),
        (
            "unknown model",
            CreateDraftRequest {
                model: Some("absent-model".into()),
                ..chat("text", "k")
            },
        ),
        (
            "chat with a repo",
            CreateDraftRequest {
                repo_id: Some(Uuid::new_v4().to_string()),
                ..chat("text", "k")
            },
        ),
        (
            "unregistered repo",
            CreateDraftRequest {
                mode: "claude-code".into(),
                repo_id: Some(Uuid::new_v4().to_string()),
                ..chat("text", "k")
            },
        ),
        (
            "missing app",
            CreateDraftRequest {
                mode: "claude-code".into(),
                app_id: Some("absent-app".into()),
                ..chat("text", "k")
            },
        ),
        (
            "unknown coding model",
            CreateDraftRequest {
                mode: "claude-code".into(),
                coding_agent: Some(CodingAgent::Codex),
                model: Some("not-a-model".into()),
                ..chat("text", "k")
            },
        ),
        (
            "too many images",
            CreateDraftRequest {
                images: vec!["artifacts/first.png".into(); MAX_COMPOSE_IMAGES + 1],
                ..chat("text", "k")
            },
        ),
        (
            "too long",
            chat(&"x".repeat(MAX_COMPOSE_TEXT_BYTES + 1), "k"),
        ),
    ];
    let events = bench.count("SELECT count(*) FROM events").await;
    let rows = bench.count("SELECT count(*) FROM thread_summaries").await;
    for (name, request) in cases {
        let refusal = bench.create(request).await.expect_err(name);
        assert!(
            matches!(refusal, DraftRefusal::Invalid(_)),
            "{name}: {refusal}"
        );
        assert_eq!(
            bench.count("SELECT count(*) FROM events").await,
            events,
            "{name}"
        );
        assert_eq!(
            bench.count("SELECT count(*) FROM thread_summaries").await,
            rows,
            "{name}"
        );
        assert_eq!(bench.blob_count(), 0, "{name}");
    }
    bench.close().await;
}

/// A coding-agent draft pins its backend and keeps its destination, so the
/// send never guesses either.
#[tokio::test]
async fn a_coding_draft_pins_its_backend_and_destination() {
    let bench = Bench::new().await;
    let created = bench
        .create(CreateDraftRequest {
            mode: "claude-code".into(),
            app_id: Some("habit-tracker".into()),
            ..chat("Add a streak counter.", "app")
        })
        .await
        .unwrap();
    let snapshot = created
        .draft
        .snapshot
        .expect("a single read carries its snapshot");
    assert_eq!(snapshot.report.mode.as_deref(), Some("claude_code"));
    assert_eq!(
        snapshot.report.selection,
        Some(serde_json::json!({
            "scope": {"kind": "app", "appId": "habit-tracker"},
            "codingAgent": prefs::CODING_AGENT_DEFAULT.read(&bench.pool).await,
        }))
    );

    let codex = bench
        .create(CreateDraftRequest {
            mode: "claude-code".into(),
            coding_agent: Some(CodingAgent::Codex),
            ..chat("Review the branch.", "codex")
        })
        .await
        .unwrap();
    assert_eq!(
        codex.draft.snapshot.unwrap().report.selection,
        Some(serde_json::json!({"codingAgent": "codex"}))
    );
    bench.close().await;
}

/// The owner sent or discarded the draft: the key is spent, and a retry
/// says so rather than creating a second draft.
#[tokio::test]
async fn a_spent_key_refuses_and_creates_nothing() {
    let bench = Bench::new().await;
    bench.create(chat("words", "key")).await.unwrap();
    let thread_id = draft_thread_id(bench.caller, "key");
    bench
        .bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ThreadDiscarded { actor: None },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    let refusal = bench.create(chat("words", "key")).await.unwrap_err();
    assert_eq!(refusal.to_string(), "draft_discarded");
    bench.close().await;
}
