//! Recoverable mention effects released only by recorded message events.
use std::sync::Arc;
use uuid::Uuid;

use super::thread_users::{on_user_only_message, UserOnlyMessageNotice};
use crate::api::SharedEngine;

pub(crate) const DELIVERY_LOCK: i64 = 706467821;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// One worker handles both live notices and notices left by a stopped engine.
/// The task holds a weak reference so it does not keep an engine alive.
pub fn start(engine: &SharedEngine) {
    let engine = Arc::downgrade(engine);
    tokio::spawn(async move {
        loop {
            let Some(engine) = engine.upgrade() else {
                return;
            };
            if let Some(snapshot) = crate::gateway_users::known() {
                if let Err(error) = drain(&engine, &snapshot).await {
                    log!("[UserOnlyMessages] Delivery will retry: {}", error);
                }
            }
            drop(engine);
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });
}

async fn drain(
    engine: &SharedEngine,
    snapshot: &crate::gateway_users::UserSnapshot,
) -> Result<(), BoxError> {
    drain_with(engine.pool(), |notice| {
        on_user_only_message(engine, notice, snapshot)
    })
    .await
}

async fn drain_with<F, Fut>(pool: &sqlx::PgPool, mut deliver: F) -> Result<(), BoxError>
where
    F: FnMut(UserOnlyMessageNotice) -> Fut,
    Fut: std::future::Future<Output = Result<(), BoxError>>,
{
    let pending: Vec<(Uuid, sqlx::types::Json<UserOnlyMessageNotice>)> = sqlx::query_as(
        "SELECT n.event_id, n.notice FROM user_only_message_notices n \
         JOIN events e ON e.id = n.event_id \
         WHERE NOT n.completed AND n.retry_after <= now() AND e.event_type IN ('UserOnlyMessagePosted', 'MessageReceived', 'UserQuestionAnswered') \
         ORDER BY e.sequence LIMIT 50",
    )
    .fetch_all(pool)
    .await?;
    let mut failure = None;
    for (id, notice) in pending {
        // The transaction owns only a lock, not the side effects: each completed
        // notification stays committed if the worker dies before the notice does.
        let mut lock = pool.begin().await?;
        let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
            .bind(DELIVERY_LOCK)
            .fetch_one(&mut *lock)
            .await?;
        if !acquired {
            continue;
        }
        let ready: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM user_only_message_notices n JOIN events e ON e.id = n.event_id WHERE n.event_id = $1 AND NOT n.completed AND n.retry_after <= now())")
            .bind(id).fetch_one(pool).await?;
        if !ready {
            continue;
        }

        if let Err(error) = deliver(notice.0).await {
            sqlx::query("UPDATE user_only_message_notices SET retry_after = now() + least(3600, 30 * (1 << least(retry_attempts, 7))) * interval '1 second', retry_attempts = retry_attempts + 1 WHERE event_id = $1")
                .bind(id).execute(pool).await?;
            failure = Some(error);
            lock.commit().await?;
            continue;
        }
        sqlx::query("UPDATE user_only_message_notices SET completed = true WHERE event_id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        lock.commit().await?;
    }
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{EventMeta, ThreadEvent};
    use crate::test_support::{setup_test_db, teardown_test_db};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn recorded_intent_recovers_and_completed_intent_does_not_replay() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        let id = Uuid::new_v4();
        let notice = UserOnlyMessageNotice {
            thread_id: thread,
            event_id: Some(id),
            text: "Hello".into(),
            mentions: vec!["sam".into()],
            user_only: true,
            sender: None,
        };
        sqlx::query("INSERT INTO user_only_message_notices (event_id, notice) VALUES ($1, $2)")
            .bind(id)
            .bind(sqlx::types::Json(notice))
            .execute(&pool)
            .await
            .unwrap();
        let attempts = AtomicUsize::new(0);
        drain_with(&pool, |_| async {
            attempts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            0,
            "unrecorded sends cannot push"
        );
        bus.emit(BusEvent::Thread {
            thread_id: thread,
            event: ThreadEvent::UserOnlyMessagePosted {
                text: "Hello".into(),
                user_image_hashes: vec![],
                device_id: None,
                mentioned_user_ids: vec!["sam".into()],
            },
            meta: EventMeta {
                event_id: Some(id),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
        assert!(
            drain_with(&pool, |_| async { Err("interrupted delivery".into()) })
                .await
                .is_err()
        );
        sqlx::query("UPDATE user_only_message_notices SET retry_after = now()")
            .execute(&pool)
            .await
            .unwrap();
        // A new worker has no memory of the interrupted attempt.
        drain_with(&pool, |notice| {
            assert_eq!(notice.event_id, Some(id));
            async {
                attempts.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })
        .await
        .unwrap();
        drain_with(&pool, |_| async {
            attempts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "completed notices do not replay after recovery"
        );
        teardown_test_db(&db).await;
    }
    async fn seed_notice(pool: &sqlx::PgPool, bus: &EventBus) -> Uuid {
        let id = Uuid::new_v4();
        let thread = Uuid::new_v4();
        let notice = UserOnlyMessageNotice {
            thread_id: thread,
            event_id: Some(id),
            text: "Hello".into(),
            mentions: vec!["sam".into()],
            user_only: true,
            sender: None,
        };
        sqlx::query("INSERT INTO user_only_message_notices (event_id, notice) VALUES ($1, $2)")
            .bind(id)
            .bind(sqlx::types::Json(notice))
            .execute(pool)
            .await
            .unwrap();
        bus.emit(BusEvent::Thread {
            thread_id: thread,
            event: ThreadEvent::UserOnlyMessagePosted {
                text: "Hello".into(),
                user_image_hashes: vec![],
                device_id: None,
                mentioned_user_ids: vec!["sam".into()],
            },
            meta: EventMeta {
                event_id: Some(id),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
        id
    }

    #[tokio::test]
    async fn failed_notice_does_not_starve_later_messages() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let failed = seed_notice(&pool, &bus).await;
        let later = seed_notice(&pool, &bus).await;
        let delivered = std::sync::Mutex::new(Vec::new());
        assert!(drain_with(&pool, |notice| {
            let delivered = &delivered;
            async move {
                if notice.event_id == Some(failed) {
                    return Err("transport failed".into());
                }
                delivered.lock().unwrap().push(notice.event_id.unwrap());
                Ok(())
            }
        })
        .await
        .is_err());
        assert_eq!(*delivered.lock().unwrap(), vec![later]);
        let attempts = AtomicUsize::new(0);
        drain_with(&pool, |_| async {
            attempts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            0,
            "failed sends back off and completed sends stay completed"
        );
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn concurrent_workers_dispatch_once() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        seed_notice(&pool, &bus).await;
        let entered = tokio::sync::Notify::new();
        let attempts = AtomicUsize::new(0);
        let first = drain_with(&pool, |_| async {
            attempts.fetch_add(1, Ordering::SeqCst);
            entered.notify_one();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok(())
        });
        let second = async {
            entered.notified().await;
            drain_with(&pool, |_| async {
                attempts.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
        };
        let (first, second) = tokio::join!(first, second);
        first.unwrap();
        second.unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        pool.close().await;
        teardown_test_db(&db).await;
    }
}
