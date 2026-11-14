//! `submit_as_agent` resolves the caller's owner authority itself (ADR 0651).
//! These tests drive it on a bare bus and pool, as the acceptance tests do.

use super::*;

use crate::engine::chat::agent_archive::agent_thread_actor;
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{
    AnswerKind, EventChannel, EventMeta, OwnerApproval, ThreadEvent,
};
use crate::llm::model_registry::{empty, ModelRegistry, ModelRouting};
use crate::test_support::{setup_test_db, teardown_test_db};
use serde_json::json;

struct Bench {
    pool: sqlx::PgPool,
    db: String,
    bus: EventBus,
    workspace: tempfile::TempDir,
    registry: ModelRegistry,
}

impl Bench {
    async fn new() -> Self {
        let (pool, db) = setup_test_db().await;
        let bus = EventBus::new(pool.clone()).0;
        let registry = empty();
        registry.write().unwrap().insert(
            "test-model".into(),
            ModelRouting::single(ProviderKind::OpenAi, "gpt-5"),
        );
        Self {
            pool,
            db,
            bus,
            workspace: tempfile::tempdir().unwrap(),
            registry,
        }
    }

    fn service(&self) -> DraftSubmissionService<'_> {
        DraftSubmissionService {
            bus: &self.bus,
            pool: &self.pool,
            workspace: self.workspace.path(),
            defaults: ModelDefaults {
                registry: &self.registry,
                fallback_model: "test-model",
            },
            configured: &[ProviderKind::OpenAi],
        }
    }

    async fn close(self) {
        self.pool.close().await;
        teardown_test_db(&self.db).await;
    }

    /// A saved chat draft on a new thread, optionally a child of `parent`.
    /// Its tokens are read back from the stored row.
    async fn draft(&self, parent: Option<Uuid>) -> SubmitDraftRequest {
        let thread = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO thread_summaries (thread_id,title,source,message_count,last_activity,\
             has_response,compose_text,state,compose_mode,parent_thread_id) \
             VALUES ($1,'Saved draft','chat',0,now(),false,'saved text','composing','lucidos',$2)",
        )
        .bind(thread)
        .bind(parent)
        .execute(&self.pool)
        .await
        .unwrap();
        let snapshot = self.snapshot(thread).await;
        SubmitDraftRequest {
            thread_id: thread,
            expected_revision: snapshot.draft_revision(),
            expected_epoch: snapshot.report().compose_epoch,
            retry_key: Uuid::new_v4(),
        }
    }

    async fn snapshot(&self, thread: Uuid) -> SavedDraftSnapshot {
        let raw: SavedDraftRow = sqlx::query_as(&format!(
            "SELECT {SAVED_DRAFT_COLS} FROM thread_summaries WHERE thread_id=$1"
        ))
        .bind(thread)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        SavedDraftSnapshot::try_from(raw).unwrap()
    }

    /// Open a turn on `caller`. A `Device` origin is the owner's own turn.
    async fn open_turn(&self, caller: Uuid, origin: MessageOrigin) {
        self.bus
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
                    channel: Some(EventChannel::Chat),
                    ..EventMeta::NONE
                },
            })
            .await
            .unwrap();
    }

    /// The owner grants `caller` a once permission to send `target`'s draft,
    /// through the engine's own card. Returns the permission's id.
    async fn allow_send(&self, caller: Uuid, target: Uuid) -> Uuid {
        use crate::api::permission_requests::{tests::grant, thread_ask};
        use crate::engine::permission::menu::ALLOW_ONCE_LABEL;
        let action = crate::engine::permission::Action::SubmitDraft;
        let ask = thread_ask(&self.pool, caller, action, Some(target), "Send it.")
            .await
            .unwrap();
        grant(&self.bus, caller, ask, ALLOW_ONCE_LABEL).await;
        sqlx::query_scalar(
            "SELECT permission_id FROM permissions WHERE action = 'submit-draft' \
             ORDER BY granted_at DESC LIMIT 1",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    /// Raise an agent-worded card on `caller` and answer it Allow once from a
    /// device. `legacy_card` marks it a pre-permission approval for `target`.
    async fn allow(&self, caller: Uuid, legacy_card: Option<Uuid>) -> String {
        let parsed = crate::engine::agent_session::parse_ask_user_question_inputs(&json!({
            "questions": [{"question":"Submit the saved draft?", "options":[
                {"label":crate::engine::permission::menu::ALLOW_ONCE_LABEL}, {"label":"Don't allow"}
            ]}]
        }))
        .remove(0);
        let option_id = parsed.options[0].id.clone();
        let tool_use_id = format!("toolu-{}#q0", Uuid::new_v4());
        self.bus
            .emit(BusEvent::Thread {
                thread_id: caller,
                event: ThreadEvent::UserQuestionAsked {
                    tool_use_id: tool_use_id.clone(),
                    cc_session_id: String::new(),
                    question: parsed.question,
                    options: parsed.options,
                    worktree_path: None,
                    multi_select: false,
                    owner_approval: legacy_card.map(|target| OwnerApproval {
                        verb: crate::engine::permission::Action::SubmitDraft,
                        target_thread_id: Some(target),
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
        self.bus
            .emit(BusEvent::Thread {
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
        tool_use_id
    }

    /// Receipts, queue entries and acceptance events, in that order.
    async fn counts(&self) -> (i64, i64, i64) {
        sqlx::query_as(
            "SELECT (SELECT count(*) FROM draft_submissions), (SELECT count(*) FROM thread_queue), \
             (SELECT count(*) FROM events WHERE event_type='DraftSubmissionAccepted')",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    async fn compose_text(&self, thread: Uuid) -> String {
        sqlx::query_scalar("SELECT compose_text FROM thread_summaries WHERE thread_id=$1")
            .bind(thread)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn spent(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type='PermissionSpent'")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn recorded_authority(&self, receipt: &DraftSubmissionReceipt) -> serde_json::Value {
        sqlx::query_scalar("SELECT payload->'data'->'authority' FROM events WHERE id=$1")
            .bind(receipt.event_id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

fn owner() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "test-owner".into(),
    }
}

fn slug(error: Box<dyn std::error::Error + Send + Sync>) -> String {
    error.to_string()
}

#[tokio::test]
async fn an_owner_opened_turn_submits_and_a_retry_returns_its_receipt() {
    let bench = Bench::new().await;
    let caller = Uuid::new_v4();
    bench.open_turn(caller, owner()).await;
    let request = bench.draft(None).await;
    let receipt = bench
        .service()
        .submit_as_agent(caller, request.clone())
        .await
        .unwrap();
    assert_eq!(
        bench.recorded_authority(&receipt).await["kind"],
        "standing-instruction"
    );
    // The instruction ends with the owner's turn, yet the receipt stands.
    bench
        .open_turn(caller, agent_thread_actor(Uuid::new_v4()))
        .await;
    assert_eq!(
        bench
            .service()
            .submit_as_agent(caller, request.clone())
            .await
            .unwrap(),
        receipt
    );
    let fresh_bus = EventBus::new(bench.pool.clone()).0;
    let replayed = DraftSubmissionService {
        bus: &fresh_bus,
        ..bench.service()
    }
    .submit_as_agent(caller, request)
    .await
    .unwrap();
    assert_eq!(replayed, receipt);
    assert_eq!(bench.counts().await, (1, 1, 1));
    bench.close().await;
}

/// I7: activity on the target after the read leaves its draft revision alone.
#[tokio::test]
async fn activity_on_the_target_after_the_read_does_not_make_it_stale() {
    let bench = Bench::new().await;
    let caller = Uuid::new_v4();
    bench.open_turn(caller, owner()).await;
    let request = bench.draft(None).await;
    sqlx::query(
        "UPDATE thread_summaries SET last_activity = now() + interval '1 minute', \
         title = 'Renamed' WHERE thread_id = $1",
    )
    .bind(request.thread_id)
    .execute(&bench.pool)
    .await
    .unwrap();
    bench
        .service()
        .submit_as_agent(caller, request.clone())
        .await
        .unwrap();
    assert_eq!(bench.counts().await, (1, 1, 1));
    assert_eq!(bench.compose_text(request.thread_id).await, "");
    bench.close().await;
}

/// I7: a permission card on the draft's own thread moves that row before the
/// acceptance consumes the draft. The consumption must still land.
#[tokio::test]
async fn a_permission_on_the_drafts_own_thread_consumes_the_draft() {
    let bench = Bench::new().await;
    let request = bench.draft(None).await;
    let caller = request.thread_id;
    bench.allow_send(caller, caller).await;
    bench
        .service()
        .submit_as_agent(caller, request.clone())
        .await
        .unwrap();
    assert_eq!(bench.spent().await, 1);
    assert_eq!(bench.compose_text(caller).await, "");
    bench.close().await;
}

/// A fresh key on a sent draft refuses as stale, not unauthorized. The caller
/// rereads instead of asking the owner for a second card.
#[tokio::test]
async fn a_stale_draft_refuses_before_authority_is_looked_up() {
    let bench = Bench::new().await;
    let caller = Uuid::new_v4();
    let request = bench.draft(None).await;
    let stale = SubmitDraftRequest {
        expected_revision: request.expected_revision + 1,
        ..request
    };
    assert_eq!(
        slug(
            bench
                .service()
                .submit_as_agent(caller, stale)
                .await
                .unwrap_err()
        ),
        "draft_snapshot_stale"
    );
    bench.close().await;
}

#[tokio::test]
async fn a_live_permission_submits_once_and_a_retry_survives_the_spend() {
    let bench = Bench::new().await;
    let caller = Uuid::new_v4();
    bench.open_turn(caller, agent_thread_actor(caller)).await;
    let request = bench.draft(None).await;
    let permission_id = bench.allow_send(caller, request.thread_id).await;
    let receipt = bench
        .service()
        .submit_as_agent(caller, request.clone())
        .await
        .unwrap();
    assert_eq!(
        bench.recorded_authority(&receipt).await,
        json!({"kind":"permission","permission_id":permission_id})
    );
    assert_eq!(bench.spent().await, 1);
    assert_eq!(
        bench
            .service()
            .submit_as_agent(caller, request.clone())
            .await
            .unwrap(),
        receipt
    );
    // The spent grant sends no second draft, even on the same target.
    sqlx::query("UPDATE thread_summaries SET compose_text = 'second text' WHERE thread_id = $1")
        .bind(request.thread_id)
        .execute(&bench.pool)
        .await
        .unwrap();
    let snapshot = bench.snapshot(request.thread_id).await;
    let second = SubmitDraftRequest {
        thread_id: request.thread_id,
        expected_revision: snapshot.draft_revision(),
        expected_epoch: snapshot.report().compose_epoch,
        retry_key: Uuid::new_v4(),
    };
    assert_eq!(
        slug(
            bench
                .service()
                .submit_as_agent(caller, second)
                .await
                .unwrap_err()
        ),
        "draft_owner_authority_missing"
    );
    assert_eq!(bench.spent().await, 1);
    assert_eq!(bench.counts().await, (1, 1, 1));
    bench.close().await;
}

#[tokio::test]
async fn subtree_ordinary_cards_legacy_cards_and_other_targets_grant_nothing() {
    let bench = Bench::new().await;
    let caller = Uuid::new_v4();
    bench.open_turn(caller, agent_thread_actor(caller)).await;

    // The caller's own child: reach is not owner authority.
    let child = bench.draft(Some(caller)).await;
    // An ordinary card answered with the same label.
    bench.allow(caller, None).await;
    // A permission naming another thread.
    let other = bench.draft(None).await;
    bench.allow_send(caller, other.thread_id).await;
    // A pre-permission approval card naming this very draft.
    let legacy = bench.draft(None).await;
    bench.allow(caller, Some(legacy.thread_id)).await;

    for request in [child, legacy] {
        let before = bench.snapshot(request.thread_id).await;
        assert_eq!(
            slug(
                bench
                    .service()
                    .submit_as_agent(caller, request.clone())
                    .await
                    .unwrap_err()
            ),
            "draft_owner_authority_missing"
        );
        assert_eq!(bench.snapshot(request.thread_id).await, before);
    }
    // Spawn provenance is no instruction either: a turn the parent opened.
    let spawned = Uuid::new_v4();
    bench.open_turn(spawned, agent_thread_actor(caller)).await;
    let request = bench.draft(None).await;
    assert!(bench
        .service()
        .submit_as_agent(spawned, request)
        .await
        .is_err());
    assert_eq!(bench.counts().await, (0, 0, 0));
    assert_eq!(bench.spent().await, 0);
    bench.close().await;
}

#[tokio::test]
async fn a_stale_revision_or_epoch_refuses_without_mutation() {
    let bench = Bench::new().await;
    let caller = Uuid::new_v4();
    bench.open_turn(caller, owner()).await;
    let request = bench.draft(None).await;
    let before = bench.snapshot(request.thread_id).await;
    for stale in [
        SubmitDraftRequest {
            expected_revision: request.expected_revision + 1,
            ..request.clone()
        },
        SubmitDraftRequest {
            expected_epoch: request.expected_epoch + 1,
            ..request.clone()
        },
    ] {
        assert_eq!(
            slug(
                bench
                    .service()
                    .submit_as_agent(caller, stale)
                    .await
                    .unwrap_err()
            ),
            "draft_snapshot_stale"
        );
    }
    assert_eq!(bench.snapshot(request.thread_id).await, before);
    assert_eq!(bench.counts().await, (0, 0, 0));
    bench.close().await;
}

/// No text is no draft, as the drafts read says. Whitespace is a draft the
/// drafts read lists, so it refuses as one the owner must fix.
#[tokio::test]
async fn an_empty_draft_is_missing_and_a_blank_one_is_not_sendable() {
    let bench = Bench::new().await;
    let caller = Uuid::new_v4();
    bench.open_turn(caller, owner()).await;
    for (text, refusal) in [("", "draft_missing"), ("\n ", "draft_empty")] {
        let request = bench.draft(None).await;
        let revision: i64 = sqlx::query_scalar(
            "UPDATE thread_summaries SET compose_text=$2 WHERE thread_id=$1 \
             RETURNING draft_revision",
        )
        .bind(request.thread_id)
        .bind(text)
        .fetch_one(&bench.pool)
        .await
        .unwrap();
        let current = SubmitDraftRequest {
            expected_revision: revision,
            ..request
        };
        assert_eq!(
            slug(
                bench
                    .service()
                    .submit_as_agent(caller, current)
                    .await
                    .unwrap_err()
            ),
            refusal
        );
    }
    assert_eq!(bench.counts().await, (0, 0, 0));
    bench.close().await;
}

#[test]
fn each_refusal_maps_to_a_status_a_reason_and_a_next_step() {
    let thread = Uuid::new_v4();
    let report = |slug: &str, audience| {
        let error: Box<dyn std::error::Error + Send + Sync> = slug.into();
        refusal_report(&*error, thread, audience)
    };
    let coding = RefusalAudience::CodingAgent;
    for (slug, status, reason) in [
        ("draft_snapshot_stale", 409, "draft_snapshot_stale"),
        ("draft_retry_key_conflict", 409, "draft_retry_key_conflict"),
        (
            "draft_owner_authority_missing",
            403,
            "draft_owner_authority_missing",
        ),
        (
            "draft_submitter_unattributed",
            403,
            "draft_submitter_unattributed",
        ),
        ("draft_missing", 404, "draft_missing"),
        ("draft_empty", 422, "draft_not_sendable"),
        ("draft_snapshot_token_invalid", 400, "draft_request_invalid"),
        ("draft_image_unreadable: gone", 422, "draft_not_sendable"),
        ("draft_channel_unspecified", 422, "draft_not_sendable"),
        (
            "draft_queue_payload_mismatch",
            500,
            "draft_submission_failed",
        ),
        ("connection reset", 500, "draft_submission_failed"),
    ] {
        let got = report(slug, coding);
        assert_eq!((got.status, got.reason), (status, reason), "{slug}");
        assert!(got.message.contains(slug), "{slug}: {}", got.message);
    }
    let stale = report("draft_snapshot_stale", coding).message;
    assert!(stale.contains(&format!("lucidos threads drafts --thread {thread}")));
    assert!(stale.contains("original retry key"), "{stale}");
    let missing = report("draft_owner_authority_missing", coding).message;
    assert!(missing.contains(&format!(
        "lucidos ask-permission submit-draft --thread {thread} --reason"
    )));
    let chat = report("draft_owner_authority_missing", RefusalAudience::ChatAgent).message;
    assert!(!chat.contains("ask-permission"), "{chat}");
    assert!(chat.contains("owner"), "{chat}");
}
