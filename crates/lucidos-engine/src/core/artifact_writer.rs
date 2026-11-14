//! Resolve artifact provenance once, before announcing a write.

use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::ArtifactWriter;
use crate::engine::thread_events::MessageOrigin;

/// The first human message that makes a trigger thread attended. Both the
/// write-time resolver and workspace turn projection use this predicate.
/// `thread` is an engine-owned SQL expression, never request input.
pub(crate) fn first_human_message_sql(thread: &str) -> String {
    format!(
        "SELECT min(m.sequence) FROM events m \
         WHERE m.thread_id = {thread} AND m.event_type = 'MessageReceived' \
           AND m.payload->>'mode' = 'human'"
    )
}

/// Write-path input. Unlike the persisted writer, this may still need to
/// resolve whether the owner has joined a trigger thread.
#[derive(Clone, Debug)]
pub enum ArtifactWriterOrigin {
    Known(ArtifactWriter),
    Thread(Uuid),
}

impl ArtifactWriterOrigin {
    /// A verified subprocess origin outranks app attribution: a scheduled
    /// script may be calling on behalf of an app, but remains unattended.
    pub fn from_actor(actor: &MessageOrigin, app_id: Option<&str>) -> Self {
        if let Some(thread_id) = actor.source_thread_id() {
            return Self::Thread(thread_id);
        }
        let writer = match actor {
            MessageOrigin::Api {
                emitting_trigger_id: Some(trigger_id),
                ..
            } => ArtifactWriter::Trigger {
                trigger_id: trigger_id.clone(),
            },
            MessageOrigin::Device { .. } => match app_id {
                Some(app_id) => ArtifactWriter::App {
                    app_id: app_id.to_owned(),
                },
                None => ArtifactWriter::Person,
            },
            _ => ArtifactWriter::Engine,
        };
        Self::Known(writer)
    }

    pub async fn resolve(self, pool: &PgPool) -> Result<ArtifactWriter, sqlx::Error> {
        let thread_id = match self {
            Self::Known(writer) => return Ok(writer),
            Self::Thread(thread_id) => thread_id,
        };
        let attended = first_human_message_sql("s.thread_id");
        let trigger: Option<Option<String>> = sqlx::query_scalar(&format!(
            "SELECT s.trigger_id FROM thread_summaries s \
             WHERE s.thread_id = $1 AND s.source = 'trigger' \
               AND ({attended}) IS NULL"
        ))
        .bind(thread_id)
        .fetch_optional(pool)
        .await?;
        Ok(match trigger {
            Some(Some(trigger_id)) => ArtifactWriter::Trigger { trigger_id },
            Some(None) => {
                return Err(sqlx::Error::Protocol(
                    "unattended trigger thread has no trigger id".into(),
                ))
            }
            None => ArtifactWriter::Thread,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::ActorMode;
    use crate::engine::thread_events::{EventMeta, ThreadEvent};

    #[tokio::test]
    async fn trigger_attendance_is_resolved_before_and_after_owner_joins() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let bus = &bus;
        let emit = |value| async move {
            let event: ThreadEvent = serde_json::from_value(value).unwrap();
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta: EventMeta::NONE,
            })
            .await
            .unwrap();
        };
        emit(serde_json::json!({ "type": "TriggerStarted", "trigger_id": "scheduled-report", "trigger_name": "Report", "prompt": "Prepare report" })).await;
        // An incomplete projection must fail closed rather than silently
        // attribute an unattended write to a normal thread.
        sqlx::query("UPDATE thread_summaries SET trigger_id = NULL WHERE thread_id = $1")
            .bind(thread_id)
            .execute(&pool)
            .await
            .unwrap();
        let missing_id = ArtifactWriterOrigin::Thread(thread_id)
            .resolve(&pool)
            .await
            .unwrap_err();
        assert!(matches!(missing_id, sqlx::Error::Protocol(_)));
        // A failed provenance lookup must refuse before touching the artifact.
        let dir = tempfile::tempdir().unwrap();
        let manager = crate::core::ArtifactManager::new(dir.path().to_path_buf()).unwrap();
        let error = manager
            .write_and_commit(
                &crate::engine::git_ops::lock_workspace_repo().await,
                bus,
                "report.md",
                "unattended report",
                "Write report",
                crate::core::WriteAnnouncement::Entity {
                    source: None,
                    writer_thread_id: Some(thread_id),
                    writer: ArtifactWriterOrigin::Thread(thread_id),
                },
            )
            .await
            .unwrap_err();
        assert!(error.downcast_ref::<sqlx::Error>().is_some());
        assert!(!manager.artifact_exists("report.md"));
        let announced: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'ArtifactCreated'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(announced, 0);

        sqlx::query("UPDATE thread_summaries SET trigger_id = $2 WHERE thread_id = $1")
            .bind(thread_id)
            .bind("scheduled-report")
            .execute(&pool)
            .await
            .unwrap();
        let before = ArtifactWriterOrigin::Thread(thread_id)
            .resolve(&pool)
            .await
            .unwrap();
        assert_eq!(
            before,
            ArtifactWriter::Trigger {
                trigger_id: "scheduled-report".into()
            }
        );

        emit(serde_json::json!({ "type": "MessageReceived", "text": "agent follow-up", "mode": "agent" })).await;
        assert_eq!(
            ArtifactWriterOrigin::Thread(thread_id)
                .resolve(&pool)
                .await
                .unwrap(),
            before
        );
        emit(serde_json::json!({ "type": "MessageReceived", "text": "Please explain the report", "mode": "human" })).await;
        assert_eq!(
            ArtifactWriterOrigin::Thread(thread_id)
                .resolve(&pool)
                .await
                .unwrap(),
            ArtifactWriter::Thread
        );
        // The already resolved value does not acquire attended status later.
        assert!(!before.is_workspace_leaf());
        assert_eq!(
            ArtifactWriterOrigin::Thread(Uuid::new_v4())
                .resolve(&pool)
                .await
                .unwrap(),
            ArtifactWriter::Thread
        );
        pool.close().await;
        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[test]
    fn every_writer_has_explicit_leafhood_and_round_trips() {
        let writers = [
            ArtifactWriter::Person,
            ArtifactWriter::Thread,
            ArtifactWriter::App {
                app_id: "habit-tracker".into(),
            },
            ArtifactWriter::Engine,
            ArtifactWriter::Trigger {
                trigger_id: "scheduled-report".into(),
            },
        ];
        for (index, writer) in writers.iter().enumerate() {
            assert_eq!(writer.is_workspace_leaf(), index != 4);
            let json = serde_json::to_value(writer).unwrap();
            assert_eq!(
                serde_json::from_value::<ArtifactWriter>(json).unwrap(),
                *writer
            );
        }
    }

    #[test]
    fn verified_origin_outranks_app_attribution() {
        let thread_id = Uuid::new_v4();
        let actor = MessageOrigin::Api {
            user_agent: None,
            mode: ActorMode::Agent,
            source_thread_id: Some(thread_id),
            emitting_trigger_id: Some("scheduled-report".into()),
        };
        assert!(
            matches!(ArtifactWriterOrigin::from_actor(&actor, Some("habit-tracker")), ArtifactWriterOrigin::Thread(id) if id == thread_id)
        );
        let actor = MessageOrigin::Api {
            user_agent: None,
            mode: ActorMode::Agent,
            source_thread_id: None,
            emitting_trigger_id: Some("scheduled-report".into()),
        };
        assert!(matches!(
            ArtifactWriterOrigin::from_actor(&actor, Some("habit-tracker")),
            ArtifactWriterOrigin::Known(ArtifactWriter::Trigger { .. })
        ));
    }

    #[test]
    fn devices_distinguish_apps_from_people_and_unattributed_api_is_engine() {
        let device = MessageOrigin::Device {
            device_id: "test-device".into(),
        };
        assert!(matches!(
            ArtifactWriterOrigin::from_actor(&device, None),
            ArtifactWriterOrigin::Known(ArtifactWriter::Person)
        ));
        assert!(matches!(
            ArtifactWriterOrigin::from_actor(&device, Some("habit-tracker")),
            ArtifactWriterOrigin::Known(ArtifactWriter::App { .. })
        ));
        let api = MessageOrigin::Api {
            user_agent: None,
            mode: ActorMode::Human,
            source_thread_id: None,
            emitting_trigger_id: None,
        };
        assert!(matches!(
            ArtifactWriterOrigin::from_actor(&api, None),
            ArtifactWriterOrigin::Known(ArtifactWriter::Engine)
        ));
    }
}
