//! Typed snapshot announcements. Payloads contain handles and fixed outcomes,
//! never primitive errors, filesystem paths, database URLs or key material.
use super::{jobs, BoxError};
use crate::engine::event_bus::{EventBus, SystemEvent};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeletionReason {
    Manual,
    Retention,
}

#[derive(Clone)]
pub struct JobEvents {
    pub bus: EventBus,
    pub manual_retention: usize,
    pub recovery_retention: usize,
}

impl JobEvents {
    pub fn progress(&self, job: &jobs::Job) {
        self.bus
            .broadcast_transient_system(SystemEvent::SnapshotProgress {
                workspace_id: job.workspace_id.clone(),
                handle: job.handle.clone(),
                job_id: job.id,
                action: job.action,
                actor: Some(job.actor.clone()),
            });
    }

    pub async fn outcome(&self, job: &jobs::Job) -> Result<(), BoxError> {
        let generation = super::boundary::read(self.bus.pool())
            .await?
            .map(|b| b.restore_id);
        if job.restore_generation != generation {
            return Err("snapshot_job_generation_mismatch".into());
        }
        let workspace_id = job.workspace_id.clone();
        let handle = job.handle.clone();
        let job_id = job.id;
        let actor = Some(job.actor.clone());
        let event = match (&job.action, &job.state) {
            (jobs::Action::Capture, jobs::State::Completed { .. }) => {
                SystemEvent::SnapshotCreated {
                    workspace_id,
                    handle,
                    job_id,
                    actor,
                }
            }
            (jobs::Action::Capture, jobs::State::Failed { reason }) => {
                SystemEvent::SnapshotCaptureFailed {
                    workspace_id,
                    handle,
                    job_id,
                    actor,
                    reason: *reason,
                }
            }
            (jobs::Action::Check, state) if !matches!(state, jobs::State::Running) => {
                SystemEvent::SnapshotChecked {
                    workspace_id,
                    handle,
                    job_id,
                    actor,
                    passed: matches!(state, jobs::State::Completed { .. }),
                }
            }
            _ => return Err("snapshot_job_not_finished".into()),
        };
        self.bus
            .emit_system_once(job.id, event)
            .await
            .map_err(|_| "snapshot_event_failed")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    #[tokio::test]
    async fn durable_announcement_retries_emit_only_once_and_refuse_identity_reuse() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let mut receiver = bus.subscribe();
        let id = uuid::Uuid::new_v4();
        let handle = super::super::handle::Handle::generate();
        let event = |passed| SystemEvent::SnapshotChecked {
            workspace_id: "example".into(),
            handle: handle.clone(),
            job_id: id,
            actor: None,
            passed,
        };
        let (first, second) = tokio::join!(
            bus.emit_system_once(id, event(true)),
            bus.emit_system_once(id, event(true)),
        );
        assert_eq!(first.unwrap().seq, second.unwrap().seq);
        receiver.try_recv().unwrap();
        assert!(receiver.try_recv().is_err());

        // A fresh bus models losing the acknowledgement and restarting.
        let (restarted, _callbacks) = EventBus::new(pool.clone());
        let mut after_restart = restarted.subscribe();
        restarted.emit_system_once(id, event(true)).await.unwrap();
        assert!(after_restart.try_recv().is_err());
        assert!(restarted.emit_system_once(id, event(false)).await.is_err());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn snapshot_outcome_retries_after_failed_append() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let events = JobEvents {
            bus,
            manual_retention: 5,
            recovery_retention: 3,
        };
        let now = chrono::Utc::now();
        let job = jobs::Job {
            restore_generation: None,
            id: uuid::Uuid::new_v4(),
            workspace_id: "example".into(),
            handle: super::super::handle::Handle::generate(),
            action: jobs::Action::Capture,
            actor: crate::engine::thread_events::MessageOrigin::Device {
                device_id: "example-device".into(),
            },
            created_at: now,
            updated_at: now,
            state: jobs::State::Failed {
                reason: jobs::Failure::Interrupted,
            },
        };
        crate::test_support::refuse_event_type(&pool, "SnapshotCaptureFailed").await;
        assert!(events.outcome(&job).await.is_err());
        sqlx::query("DROP TRIGGER refuse_event ON events")
            .execute(&pool)
            .await
            .unwrap();
        let tree = tempfile::tempdir().unwrap();
        let store = super::super::store::Store::at(tree.path().join("store"), "example").unwrap();
        super::super::store::private_dir(&store.root().join("jobs")).unwrap();
        super::super::store::write_json(
            &store.root().join("jobs").join(format!("{}.json", job.id)),
            &job,
        )
        .unwrap();
        for _ in 0..2 {
            let rediscovered = jobs::settle(store.root(), "example", None).unwrap();
            assert_eq!(rediscovered.len(), 1);
            events.outcome(&rediscovered[0]).await.unwrap();
        }
        // Rewinding the database must not replay the surviving external job.
        let generation = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO snapshot_restore_boundary (singleton, restore_id, snapshot, restored_at, event_sequence) VALUES (TRUE, $1, $2, NOW(), 0)")
            .bind(generation).bind(job.handle.as_str()).execute(&pool).await.unwrap();
        assert!(jobs::settle(store.root(), "example", Some(generation))
            .unwrap()
            .is_empty());
        assert!(events.outcome(&job).await.is_err());
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE id = $1 AND event_type = 'SnapshotCaptureFailed'",
        )
        .bind(job.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn durable_announcement_refuses_transient_events() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        assert!(bus
            .emit_system_once(
                uuid::Uuid::new_v4(),
                SystemEvent::BackupProgress {
                    phase: "test".into(),
                    progress: 0,
                    total: 1,
                }
            )
            .await
            .is_err());
        pool.close().await;
        teardown_test_db(&db).await;
    }
}
