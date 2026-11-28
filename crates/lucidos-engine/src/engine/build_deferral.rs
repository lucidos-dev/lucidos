use crate::core::prefs;
use serde::Serialize;
use sqlx::PgPool;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const ACTIVE_CODING_THREADS_SQL: &str = "SELECT thread_id AS id, COALESCE(title, 'Coding thread') AS title, status AS state, COUNT(*) OVER () AS total FROM thread_summaries WHERE is_coding_agent AND ((status = 'running' AND NOT keep_alive_turn) OR status IN ('paused', 'waiting_for_user_answer') OR live_event_wait_count > 0) ORDER BY thread_id LIMIT 3";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct ActiveThread {
    pub id: uuid::Uuid,
    pub title: String,
    pub state: String,
    #[serde(skip)]
    total: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActiveThreads {
    pub count: i64,
    pub threads: Vec<ActiveThread>,
}

pub(crate) async fn active_coding_threads(pool: &PgPool) -> Result<ActiveThreads, sqlx::Error> {
    let threads: Vec<ActiveThread> = sqlx::query_as(ACTIVE_CODING_THREADS_SQL)
        .fetch_all(pool)
        .await?;
    Ok(ActiveThreads {
        count: threads.first().map_or(0, |t| t.total),
        threads,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BuildDeferral {
    Admit,
    Defer(ActiveThreads),
}

fn build_deferral(pref: Option<&str>, active: Option<ActiveThreads>) -> BuildDeferral {
    match (pref, active) {
        (Some("while-coding"), Some(active)) if active.count > 0 => BuildDeferral::Defer(active),
        _ => BuildDeferral::Admit,
    }
}

pub(crate) async fn automatic_build_deferral(pool: &PgPool) -> BuildDeferral {
    let result = async {
        let pref = prefs::DEFER_VERSION_BUILDS.try_read(pool).await?;
        let active = if pref == "while-coding" {
            Some(active_coding_threads(pool).await?)
        } else {
            None
        };
        Ok::<_, sqlx::Error>(build_deferral(Some(&pref), active))
    }
    .await;
    match result {
        Ok(deferral) => deferral,
        Err(error) => {
            log_unreadable_once(&error);
            BuildDeferral::Admit
        }
    }
}

pub(crate) fn log_unreadable_once(error: &sqlx::Error) {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if !LOGGED.swap(true, Ordering::Relaxed) {
        log_legacy!(
            "[Rebuild] build hold unreadable; admitting automatic builds: {}",
            error
        );
    }
}

#[derive(Clone, Debug, Default)]
pub enum BuildRequest {
    #[default]
    None,
    Started,
    Deferred(i64),
}

impl From<BuildDeferral> for BuildRequest {
    fn from(deferral: BuildDeferral) -> Self {
        match deferral {
            BuildDeferral::Admit => Self::Started,
            BuildDeferral::Defer(active) => Self::Deferred(active.count),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_opted_in_known_activity_holds() {
        let active = ActiveThreads {
            count: 2,
            threads: vec![],
        };
        assert!(matches!(
            build_deferral(Some("while-coding"), Some(active.clone())),
            BuildDeferral::Defer(_)
        ));
        for pref in [None, Some("never"), Some("unknown")] {
            assert_eq!(
                build_deferral(pref, Some(active.clone())),
                BuildDeferral::Admit
            );
        }
        assert_eq!(
            build_deferral(Some("while-coding"), None),
            BuildDeferral::Admit
        );
        assert_eq!(
            build_deferral(Some("while-coding"), Some(ActiveThreads::default())),
            BuildDeferral::Admit
        );
    }

    #[tokio::test]
    async fn projection_definition_covers_every_active_state() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db) = setup_test_db().await;
        let id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO thread_summaries(thread_id, title, status, is_coding_agent) VALUES ($1, 'Worker', 'idle', true)").bind(id).execute(&pool).await.unwrap();
        assert_eq!(prefs::DEFER_VERSION_BUILDS.read(&pool).await, "never");
        for (status, keep_alive, waits, expected) in [
            ("running", false, 0, 1),
            ("running", true, 0, 0),
            ("paused", false, 0, 1),
            ("waiting_for_user_answer", false, 0, 1),
            ("idle", false, 1, 1),
            ("finished", false, 1, 1),
            ("idle", false, 0, 0),
            ("finished", false, 0, 0),
            ("failed", false, 0, 0),
            ("canceled", false, 0, 0),
            ("archived", false, 0, 0),
        ] {
            sqlx::query("UPDATE thread_summaries SET status=$2, keep_alive_turn=$3, live_event_wait_count=$4 WHERE thread_id=$1").bind(id).bind(status).bind(keep_alive).bind(waits).execute(&pool).await.unwrap();
            assert_eq!(
                active_coding_threads(&pool).await.unwrap().count,
                expected,
                "{status}, keep_alive={keep_alive}, waits={waits}"
            );
        }
        sqlx::query("UPDATE thread_summaries SET is_coding_agent=false, status='running', live_event_wait_count=1 WHERE thread_id=$1").bind(id).execute(&pool).await.unwrap();
        assert_eq!(active_coding_threads(&pool).await.unwrap().count, 0);
        pool.close().await;
        assert_eq!(automatic_build_deferral(&pool).await, BuildDeferral::Admit);
        teardown_test_db(&db).await;
    }
}
