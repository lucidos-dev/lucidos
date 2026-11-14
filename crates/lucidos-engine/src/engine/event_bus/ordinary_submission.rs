//! Atomic ordinary acceptance. Message recording remains execution-time work:
//! pending questions and coding redirects have their own recording order.
use super::*;
use crate::core::store::ComposeReport;
use crate::engine::thread_events::{ActorMode, MessageOrigin};
use crate::engine::thread_queue::ThreadQueueRequest;
use crate::engine::{InputAdmission, InputOrder, InputPosition};
use serde::{Deserialize, Serialize};

/// The authenticated transport captures this before resolving defaults or contexts.
/// Keep the normalized wire request, including its explicit selections and fence;
/// the resolved execution request is separately retained in the acceptance event.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OrdinarySubmissionIntent {
    thread_id: Uuid,
    message_event_id: Uuid,
    request: serde_json::Value,
}

impl OrdinarySubmissionIntent {
    pub fn new(
        thread_id: Uuid,
        message_event_id: Uuid,
        request: &impl Serialize,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let request = serde_json::to_value(request)?;
        if thread_id.is_nil() || message_event_id.is_nil() || !request.is_object() {
            return Err("ordinary_intent_invalid".into());
        }
        Ok(Self {
            thread_id,
            message_event_id,
            request,
        })
    }
}

/// The immutable acknowledgement of one accepted composer input.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OrdinarySubmissionReceipt {
    pub event_id: Uuid,
    pub entry_id: Uuid,
    pub compose: Option<ComposeReport>,
}

pub struct OrdinaryAcceptance {
    pub receipt: OrdinarySubmissionReceipt,
    /// Replays never join or advance input order. New acceptance retains its
    /// runtime turn in EventBus until the durable queue's executor takes it.
    pub replayed: bool,
}

impl EventBus {
    pub async fn ordinary_submission_receipt(
        &self,
        intent: &OrdinarySubmissionIntent,
        actor: &MessageOrigin,
    ) -> Result<Option<OrdinarySubmissionReceipt>, Box<dyn std::error::Error + Send + Sync>> {
        read_receipt(&self.pool, intent, actor).await
    }

    /// The caller authenticates and fully resolves the request first. A fence
    /// belongs to the send intent, never a fresh read immediately before POST.
    pub(crate) async fn accept_ordinary_submission(
        &self,
        request: ThreadQueueRequest,
        actor: MessageOrigin,
        order: &InputOrder,
        position: Option<InputPosition>,
        expected_epoch: Option<i64>,
        intent: OrdinarySubmissionIntent,
    ) -> Result<OrdinaryAcceptance, Box<dyn std::error::Error + Send + Sync>> {
        let OrdinaryIdentity {
            thread_id,
            message_id,
            text,
            images,
        } = ordinary_identity(&request)?;
        if intent.thread_id != thread_id || intent.message_event_id != message_id {
            return Err("ordinary_intent_identity_mismatch".into());
        }
        if actor.mode() != ActorMode::Human {
            return Err("ordinary_actor_invalid".into());
        }
        if let ThreadQueueRequest::AgentChat { origin, .. } = &request {
            if origin.as_ref() != Some(&actor) {
                return Err("ordinary_actor_mismatch".into());
            }
        }
        let mut append = self.begin_append().await?;
        Self::lock_thread_append(&mut append.tx, thread_id).await?;
        // Message identities are workspace-wide, including concurrent requests
        // naming different threads.
        sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
            .bind(0x6f72_6469_i32)
            .bind(message_id.to_string())
            .execute(&mut *append.tx)
            .await?;
        if let Some(receipt) = read_receipt(&mut *append.tx, &intent, &actor).await? {
            append.tx.rollback().await?;
            return Ok(OrdinaryAcceptance {
                receipt,
                replayed: true,
            });
        }
        if expected_epoch.is_none() {
            let saved_input: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM draft_submissions WHERE thread_id=$1)",
            )
            .bind(thread_id)
            .fetch_one(&mut *append.tx)
            .await?;
            if saved_input {
                return Err("compose_fence_required".into());
            }
        }
        if let Some(expected) = expected_epoch {
            let epoch: Option<i64> = sqlx::query_scalar(
                "SELECT compose_epoch FROM thread_summaries WHERE thread_id=$1 AND state IN ('composing','active') FOR UPDATE",
            ).bind(thread_id).fetch_optional(&mut *append.tx).await?;
            if epoch != Some(expected) {
                return Err("compose_snapshot_stale".into());
            }
        }
        let turn = match order
            .admit_in_transaction(&mut append.tx, thread_id, position, Some(message_id))
            .await?
        {
            InputAdmission::Admitted(turn) => turn,
            InputAdmission::Overtaken => return Err("overtaken".into()),
        };
        let compose =
            crate::core::store::consume_compose_slot(&mut *append.tx, thread_id, text, images)
                .await?;
        let receipt = OrdinarySubmissionReceipt {
            event_id: message_id,
            entry_id: Uuid::new_v4(),
            compose,
        };
        let device_id = match &actor {
            MessageOrigin::Device { device_id } | MessageOrigin::SessionDevice { device_id } => {
                Some(device_id.clone())
            }
            _ => None,
        };
        let event = SystemEvent::OrdinarySubmissionAccepted {
            thread_id,
            message_event_id: message_id,
            entry_id: receipt.entry_id,
            request: Box::new(request),
            intent: Box::new(intent),
            compose: receipt.compose.clone(),
            actor: Some(actor),
        };
        let registration =
            self.register_accepted_input_turn(receipt.entry_id, thread_id, message_id, turn, order);
        let committed = self
            .commit_system_event(
                append,
                Uuid::new_v4(),
                event,
                current_event_trigger_depth(),
                current_trigger_id(),
            )
            .await?;
        registration.commit();
        self.publish_system_event(committed);
        if let Some(report) = &receipt.compose {
            self.emit_or_log(
                report.clone().broadcast(thread_id, device_id),
                "[Compose] ThreadComposeChanged",
            )
            .await;
        }
        Ok(OrdinaryAcceptance {
            receipt,
            replayed: false,
        })
    }
}

struct OrdinaryIdentity<'a> {
    thread_id: Uuid,
    message_id: Uuid,
    text: &'a str,
    images: &'a [String],
}

fn ordinary_identity(
    request: &ThreadQueueRequest,
) -> Result<OrdinaryIdentity<'_>, Box<dyn std::error::Error + Send + Sync>> {
    match request {
        ThreadQueueRequest::AgentChat {
            thread_id,
            event_id: Some(id),
            message,
            image_hashes,
            mode: ActorMode::Human,
            ..
        } => {
            let id = Uuid::parse_str(id)?;
            if id.is_nil() {
                return Err("ordinary_message_identity_missing".into());
            }
            Ok(OrdinaryIdentity {
                thread_id: *thread_id,
                message_id: id,
                text: message,
                images: image_hashes,
            })
        }
        _ => Err("ordinary_request_invalid".into()),
    }
}

async fn read_receipt<'e>(
    executor: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
    intent: &OrdinarySubmissionIntent,
    actor: &MessageOrigin,
) -> Result<Option<OrdinarySubmissionReceipt>, Box<dyn std::error::Error + Send + Sync>> {
    let message_id = intent.message_event_id;
    let payload: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM events WHERE event_type='OrdinarySubmissionAccepted' AND payload->'data'->>'message_event_id'=$1",
    ).bind(message_id.to_string()).fetch_optional(executor).await?;
    let Some(payload) = payload else {
        return Ok(None);
    };
    let event: SystemEvent = serde_json::from_value(payload)?;
    let SystemEvent::OrdinarySubmissionAccepted {
        intent: accepted,
        actor: accepted_actor,
        entry_id,
        compose,
        ..
    } = event
    else {
        return Err("ordinary_receipt_invalid".into());
    };
    if accepted.as_ref() != intent || accepted_actor.as_ref() != Some(actor) {
        return Err("ordinary_retry_identity_conflict".into());
    }
    Ok(Some(OrdinarySubmissionReceipt {
        event_id: message_id,
        entry_id,
        compose,
    }))
}

impl crate::engine::LucidosEngine {
    /// Accept a fully resolved ordinary input. Transports authenticate before
    /// calling this; EventBus retains its input turn for durable queue dispatch.
    pub async fn accept_ordinary_submission(
        &self,
        request: ThreadQueueRequest,
        actor: MessageOrigin,
        chain: Option<&str>,
        sequence: Option<i64>,
        expected_epoch: Option<i64>,
        intent: OrdinarySubmissionIntent,
    ) -> Result<OrdinaryAcceptance, Box<dyn std::error::Error + Send + Sync>> {
        let id = ordinary_identity(&request)?.message_id;
        let position = InputPosition::from_wire(chain, sequence, Some(id))?;
        self.event_bus
            .accept_ordinary_submission(
                request,
                actor,
                &self.input_order,
                position,
                expected_epoch,
                intent,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use serde_json::json;

    fn request(thread: Uuid, id: Uuid, actor: &MessageOrigin) -> ThreadQueueRequest {
        serde_json::from_value(json!({
            "type": "agent-chat", "thread_id":thread, "event_id":id,
            "message":"saved text", "image_hashes":["second","first","second"],
            "mode":"human", "origin":actor, "file_context":"resolved file",
        }))
        .unwrap()
    }

    fn intent(request: &ThreadQueueRequest) -> OrdinarySubmissionIntent {
        let identity = ordinary_identity(request).unwrap();
        OrdinarySubmissionIntent::new(identity.thread_id, identity.message_id, request).unwrap()
    }

    async fn seed(pool: &sqlx::PgPool, thread: Uuid) -> i64 {
        sqlx::query_scalar("INSERT INTO thread_summaries (thread_id,title,source,message_count,last_activity,has_response,compose_text) VALUES ($1,'Synthetic draft','chat',0,now(),false,'saved text') RETURNING compose_epoch")
            .bind(thread).fetch_one(pool).await.unwrap()
    }

    #[tokio::test]
    async fn urgent_composer_answer_fences_and_rolls_back_with_its_record() {
        use crate::engine::thread_events::AnswerKind;
        let (pool, db) = setup_test_db().await;
        let (bus, _) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        let epoch = seed(&pool, thread).await;
        let make_answer = || ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: "synthetic-question".into(),
            answer: AnswerKind::FreeText {
                text: "saved text".into(),
                image_hashes: vec![],
            },
            quiet: false,
        };
        let actor = MessageOrigin::Device {
            device_id: "synthetic-owner".into(),
        };
        // Urgent answers must not wait for the turn that is asking this question.
        let _blocked = bus.input_order.join(thread);
        let stale = bus
            .emit_question_answer(
                thread,
                make_answer(),
                EventMeta::with_actor(Some(actor.clone())),
                None,
                Some(epoch + 1),
            )
            .await;
        assert!(stale
            .err()
            .unwrap()
            .to_string()
            .contains("compose_snapshot_stale"));
        let before = ComposeReport::from(
            sqlx::query_as::<_, crate::core::store::ComposeReportRow>(&format!(
                "SELECT {} FROM thread_summaries WHERE thread_id=$1",
                crate::core::store::COMPOSE_REPORT_COLS
            ))
            .bind(thread)
            .fetch_one(&pool)
            .await
            .unwrap(),
        );
        assert_eq!(before.compose_epoch, epoch);
        let id = Uuid::new_v4();
        let meta = EventMeta {
            event_id: Some(id),
            actor: Some(actor.clone()),
            ..EventMeta::NONE
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            bus.emit_question_answer(thread, make_answer(), meta.clone(), None, Some(epoch)),
        )
        .await
        .unwrap()
        .unwrap();
        let after = ComposeReport::from(
            sqlx::query_as::<_, crate::core::store::ComposeReportRow>(&format!(
                "SELECT {} FROM thread_summaries WHERE thread_id=$1",
                crate::core::store::COMPOSE_REPORT_COLS
            ))
            .bind(thread)
            .fetch_one(&pool)
            .await
            .unwrap(),
        );
        assert_eq!(after.compose_epoch, epoch + 1);
        assert!(after.text.is_empty());
        assert!(bus
            .emit_question_answer(
                thread,
                make_answer(),
                EventMeta::with_actor(Some(actor)),
                None,
                Some(epoch + 1)
            )
            .await
            .is_err());
        let duplicate = ComposeReport::from(
            sqlx::query_as::<_, crate::core::store::ComposeReportRow>(&format!(
                "SELECT {} FROM thread_summaries WHERE thread_id=$1",
                crate::core::store::COMPOSE_REPORT_COLS
            ))
            .bind(thread)
            .fetch_one(&pool)
            .await
            .unwrap(),
        );
        assert_eq!(
            duplicate.compose_epoch,
            epoch + 1,
            "duplicate answer rolls back consumption"
        );
        let payload: serde_json::Value =
            sqlx::query_scalar("SELECT payload FROM events WHERE id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(payload["compose"]["compose_epoch"], epoch + 1);
        let persisted = ThreadEvent::from_stored("UserQuestionAnswered", payload).unwrap();
        sqlx::query("UPDATE thread_summaries SET compose_text='newer draft',compose_epoch=$2 WHERE thread_id=$1")
            .bind(thread).bind(epoch + 2).execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        bus.update_thread_projection(&mut tx, thread, &persisted, &meta)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let newer: String =
            sqlx::query_scalar("SELECT compose_text FROM thread_summaries WHERE thread_id=$1")
                .bind(thread)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            newer, "newer draft",
            "replay cannot erase a later compose epoch"
        );
        sqlx::query("UPDATE thread_summaries SET compose_epoch=$2 WHERE thread_id=$1")
            .bind(thread)
            .bind(epoch)
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        bus.update_thread_projection(&mut tx, thread, &persisted, &meta)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let rebuilt: (String, i64) = sqlx::query_as(
            "SELECT compose_text,compose_epoch FROM thread_summaries WHERE thread_id=$1",
        )
        .bind(thread)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rebuilt, (String::new(), epoch + 1));
        // A plain pick stays urgent even with a stale compose fence.
        bus.emit_question_answer(
            thread,
            ThreadEvent::UserQuestionAnswered {
                compose: None,
                tool_use_id: "plain-pick".into(),
                answer: AnswerKind::Selected {
                    option_id: "selected".into(),
                },
                quiet: true,
            },
            EventMeta::NONE,
            None,
            Some(epoch),
        )
        .await
        .unwrap();
        let final_epoch: i64 =
            sqlx::query_scalar("SELECT compose_epoch FROM thread_summaries WHERE thread_id=$1")
                .bind(thread)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            final_epoch,
            epoch + 1,
            "option-only answers never consume compose"
        );
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn acceptance_replays_before_fences_and_retains_exact_payload() {
        let (pool, db) = setup_test_db().await;
        let (bus, _) = EventBus::new(pool.clone());
        let order = InputOrder::default();
        let thread = Uuid::new_v4();
        let epoch = seed(&pool, thread).await;
        let actor = MessageOrigin::Device {
            device_id: "synthetic-owner".into(),
        };
        let input = request(thread, Uuid::new_v4(), &actor);
        let position = InputPosition {
            chain: Uuid::new_v4(),
            seq: 1,
        };
        let mut announcements = bus.subscribe();
        let first = bus
            .accept_ordinary_submission(
                input.clone(),
                actor.clone(),
                &order,
                Some(position),
                Some(epoch),
                intent(&input),
            )
            .await
            .unwrap();
        assert!(!first.replayed);
        let receipt = first.receipt;
        let runtime_turn = bus
            .take_accepted_input_turn(receipt.entry_id)
            .expect("a returned acceptance retains runtime ownership for queue dispatch");
        drop(runtime_turn);
        let mut successor = order.join(thread);
        assert!(
            successor.has_input_ahead(),
            "unrecorded ordinary failure cannot release successors"
        );
        assert_eq!(receipt.compose.as_ref().unwrap().compose_epoch, epoch + 1);
        assert!(matches!(
            announcements.try_recv().unwrap().typed,
            BusEvent::System(SystemEvent::OrdinarySubmissionAccepted { .. })
        ));
        let frame = announcements.try_recv().unwrap();
        let BusEvent::System(SystemEvent::ThreadComposeChanged {
            compose_epoch,
            origin_device_id,
            ..
        }) = frame.typed
        else {
            panic!("accepted compose must announce after commit");
        };
        assert_eq!(compose_epoch, epoch + 1);
        assert_eq!(origin_device_id.as_deref(), Some("synthetic-owner"));
        assert!(announcements.try_recv().is_err());
        let payload: serde_json::Value =
            sqlx::query_scalar("SELECT request FROM thread_queue WHERE id=$1")
                .bind(receipt.entry_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(payload, serde_json::to_value(&input).unwrap());
        let messages: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type='MessageReceived'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(messages, 0, "acceptance must not pre-emit a message");
        sqlx::query("UPDATE thread_summaries SET compose_text='newer draft',compose_epoch=compose_epoch+1 WHERE thread_id=$1")
            .bind(thread).execute(&pool).await.unwrap();
        // A later input has overtaken the replay's sequence.
        let next = order
            .admit(
                &pool,
                thread,
                Some(InputPosition { seq: 2, ..position }),
                Some(Uuid::new_v4()),
            )
            .await
            .unwrap();
        drop(next);
        let (fresh_bus, _) = EventBus::new(pool.clone());
        let mut replay_announcements = fresh_bus.subscribe();
        let replay = fresh_bus
            .accept_ordinary_submission(
                input.clone(),
                actor.clone(),
                &order,
                Some(position),
                Some(epoch),
                intent(&input),
            )
            .await
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.receipt, receipt);
        assert!(
            replay_announcements.try_recv().is_err(),
            "replay announces nothing"
        );
        let text: String =
            sqlx::query_scalar("SELECT compose_text FROM thread_summaries WHERE thread_id=$1")
                .bind(thread)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(text, "newer draft");
        let mut changed = input.clone();
        if let ThreadQueueRequest::AgentChat { message, .. } = &mut changed {
            *message = "altered".into();
        }
        assert!(fresh_bus
            .ordinary_submission_receipt(&intent(&changed), &actor)
            .await
            .unwrap_err()
            .to_string()
            .contains("identity_conflict"));
        let stale_input = request(thread, Uuid::new_v4(), &actor);
        let refused = fresh_bus
            .accept_ordinary_submission(
                stale_input.clone(),
                actor,
                &order,
                None,
                Some(epoch),
                intent(&stale_input),
            )
            .await;
        assert!(refused
            .err()
            .unwrap()
            .to_string()
            .contains("compose_snapshot_stale"));
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn raw_intent_replays_without_resolving_current_defaults() {
        let (pool, db) = setup_test_db().await;
        let (bus, _) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        let epoch = seed(&pool, thread).await;
        let actor = MessageOrigin::Device {
            device_id: "synthetic-owner".into(),
        };
        let id = Uuid::new_v4();
        let intent = OrdinarySubmissionIntent::new(
            thread,
            id,
            &json!({
                "message":"saved text", "model":null, "image_hashes":["second","first","second"]
            }),
        )
        .unwrap();
        let mut prepared = request(thread, id, &actor);
        if let ThreadQueueRequest::AgentChat { model, .. } = &mut prepared {
            *model = Some("resolved-at-acceptance".into());
        }
        let first = bus
            .accept_ordinary_submission(
                prepared,
                actor.clone(),
                &InputOrder::default(),
                None,
                Some(epoch),
                intent.clone(),
            )
            .await
            .unwrap()
            .receipt;
        let (fresh_bus, _) = EventBus::new(pool.clone());
        // No prepared payload or current model registry is needed to retry.
        assert_eq!(
            fresh_bus
                .ordinary_submission_receipt(&intent, &actor)
                .await
                .unwrap(),
            Some(first)
        );
        for altered in [
            OrdinarySubmissionIntent::new(thread, id, &json!({"message":"changed"})).unwrap(),
            OrdinarySubmissionIntent::new(Uuid::new_v4(), id, &intent.request).unwrap(),
        ] {
            assert!(fresh_bus
                .ordinary_submission_receipt(&altered, &actor)
                .await
                .unwrap_err()
                .to_string()
                .contains("identity_conflict"));
        }
        let other = MessageOrigin::Device {
            device_id: "another-device".into(),
        };
        assert!(fresh_bus
            .ordinary_submission_receipt(&intent, &other)
            .await
            .is_err());
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn queue_projection_failure_rolls_back_compose_receipt_and_chain() {
        let (pool, db) = setup_test_db().await;
        let (bus, _) = EventBus::new(pool.clone());
        let order = InputOrder::default();
        let thread = Uuid::new_v4();
        let epoch = seed(&pool, thread).await;
        let actor = MessageOrigin::Device {
            device_id: "synthetic-owner".into(),
        };
        let input = request(thread, Uuid::new_v4(), &actor);
        let position = InputPosition {
            chain: Uuid::new_v4(),
            seq: 1,
        };
        let mut announcements = bus.subscribe();
        sqlx::raw_sql("CREATE FUNCTION refuse_ordinary_queue() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic queue failure'; END $$; CREATE TRIGGER refuse_ordinary_queue BEFORE INSERT ON thread_queue FOR EACH ROW EXECUTE FUNCTION refuse_ordinary_queue();")
            .execute(&pool).await.unwrap();
        assert!(bus
            .accept_ordinary_submission(
                input.clone(),
                actor.clone(),
                &order,
                Some(position),
                Some(epoch),
                intent(&input),
            )
            .await
            .is_err());
        assert!(bus
            .ordinary_submission_receipt(&intent(&input), &actor)
            .await
            .unwrap()
            .is_none());
        assert!(
            announcements.try_recv().is_err(),
            "rollback announces nothing"
        );
        let actual: (String, i64) = sqlx::query_as(
            "SELECT compose_text,compose_epoch FROM thread_summaries WHERE thread_id=$1",
        )
        .bind(thread)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(actual, ("saved text".into(), epoch));
        let chains: i64 =
            sqlx::query_scalar("SELECT count(*) FROM thread_input_chains WHERE thread_id=$1")
                .bind(thread)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(chains, 0);
        sqlx::query("DROP TRIGGER refuse_ordinary_queue ON thread_queue")
            .execute(&pool)
            .await
            .unwrap();
        let accepted = bus
            .accept_ordinary_submission(
                input.clone(),
                actor,
                &order,
                Some(position),
                Some(epoch),
                intent(&input),
            )
            .await
            .unwrap();
        drop(accepted);
        pool.close().await;
        teardown_test_db(&db).await;
    }
}
