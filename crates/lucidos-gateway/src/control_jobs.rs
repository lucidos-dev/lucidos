//! Long control mutations that a dropped request must never cut short.
//!
//! A proxy cancels an origin request on its own timeout, Cloudflare at about
//! 100 s. Hyper then drops the handler future at its current `.await`.
//! A delete cut there leaves an engine stopped and a Trash journal half done.
//! So a long mutation runs as a spawned job. The handler waits a bounded time,
//! then answers `202` while the job carries on. A repeat request with the same
//! key attaches to that job instead of starting a second one.

use crate::error::ApiError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// How long a handler waits before answering `202`. Well under Cloudflare's
/// 100 s origin timeout and the common reverse-proxy defaults.
pub(crate) const CONTROL_JOB_WAIT: Duration = Duration::from_secs(20);

/// How long a successful result stays attachable after it finished, for a
/// client whose response was lost on the way back.
const FINISHED_RETENTION: Duration = Duration::from_secs(600);

/// `None` answers `204 No Content`.
pub(crate) type JobBody = Option<serde_json::Value>;
type JobResult = Result<JobBody, ApiError>;

#[derive(Debug)]
pub(crate) enum JobOutcome {
    Finished(JobResult),
    Pending,
}

#[derive(Clone)]
struct Finished {
    result: JobResult,
    at: Instant,
}

type JobChannel = watch::Receiver<Option<Finished>>;

pub(crate) struct ControlJobs {
    jobs: Mutex<HashMap<String, JobChannel>>,
    retention: Duration,
}

impl Default for ControlJobs {
    fn default() -> Self {
        Self::with_retention(FINISHED_RETENTION)
    }
}

impl ControlJobs {
    fn with_retention(retention: Duration) -> Self {
        Self {
            jobs: Mutex::default(),
            retention,
        }
    }

    /// Attach to the job under `key`, starting it with `start` if none is
    /// running or retained. Waits at most `wait` for its result.
    ///
    /// A failure is reported to one request and then forgotten, so the user
    /// can retry at once under the same key.
    pub(crate) async fn run<F>(
        &self,
        key: String,
        wait: Duration,
        start: impl FnOnce() -> F,
    ) -> JobOutcome
    where
        F: Future<Output = JobResult> + Send + 'static,
    {
        let mut channel = {
            let mut jobs = self.jobs.lock().unwrap();
            jobs.retain(|_, job| {
                job.borrow()
                    .as_ref()
                    .is_none_or(|finished| finished.at.elapsed() < self.retention)
            });
            match jobs.get(&key) {
                Some(job) => job.clone(),
                None => {
                    let (sender, receiver) = watch::channel(None);
                    let work = start();
                    tokio::spawn(async move {
                        let result = work.await;
                        sender.send_replace(Some(Finished {
                            result,
                            at: Instant::now(),
                        }));
                    });
                    jobs.insert(key.clone(), receiver.clone());
                    receiver
                }
            }
        };
        let finished = tokio::time::timeout(wait, async {
            channel
                .wait_for(Option::is_some)
                .await
                .map(|finished| finished.clone())
        })
        .await;
        let result = match finished {
            Err(_) => return JobOutcome::Pending,
            Ok(Ok(Some(finished))) => finished.result,
            Ok(Ok(None) | Err(_)) => Err(ApiError::internal(
                "the gateway job ended without a result; retry the action",
            )),
        };
        if result.is_err() {
            let mut jobs = self.jobs.lock().unwrap();
            if jobs.get(&key).is_some_and(|job| job.same_channel(&channel)) {
                jobs.remove(&key);
            }
        }
        JobOutcome::Finished(result)
    }
}

impl IntoResponse for JobOutcome {
    fn into_response(self) -> Response {
        match self {
            JobOutcome::Finished(Ok(Some(body))) => Json(body).into_response(),
            JobOutcome::Finished(Ok(None)) => StatusCode::NO_CONTENT.into_response(),
            JobOutcome::Finished(Err(error)) => error.into_response(),
            JobOutcome::Pending => (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "pending": true })),
            )
                .into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::Notify;

    type Work = std::pin::Pin<Box<dyn Future<Output = JobResult> + Send>>;

    fn gated(runs: Arc<AtomicUsize>, gate: Arc<Notify>) -> impl FnOnce() -> Work {
        move || {
            Box::pin(async move {
                runs.fetch_add(1, Ordering::SeqCst);
                gate.notified().await;
                Ok(Some(serde_json::json!({ "done": true })))
            })
        }
    }

    #[tokio::test]
    async fn a_dropped_request_never_cancels_the_job() {
        let jobs = ControlJobs::default();
        let runs = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Notify::new());
        let request = jobs.run(
            "k".into(),
            Duration::from_secs(60),
            gated(runs.clone(), gate.clone()),
        );
        assert!(tokio::time::timeout(Duration::from_millis(50), request)
            .await
            .is_err());
        gate.notify_one();
        let outcome = jobs
            .run("k".into(), Duration::from_secs(5), || async {
                panic!("a retained job must not start again")
            })
            .await;
        assert!(matches!(outcome, JobOutcome::Finished(Ok(Some(_)))));
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_slow_job_answers_pending_then_a_repeat_attaches() {
        let jobs = ControlJobs::default();
        let runs = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Notify::new());
        let first = jobs
            .run(
                "k".into(),
                Duration::from_millis(20),
                gated(runs.clone(), gate.clone()),
            )
            .await;
        assert!(matches!(first, JobOutcome::Pending));
        assert_eq!(first.into_response().status(), StatusCode::ACCEPTED);
        gate.notify_one();
        let second = jobs
            .run(
                "k".into(),
                Duration::from_secs(5),
                gated(runs.clone(), gate.clone()),
            )
            .await;
        assert_eq!(second.into_response().status(), StatusCode::OK);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn distinct_keys_run_separately_and_errors_keep_their_status() {
        let jobs = ControlJobs::default();
        let refused = jobs
            .run("a".into(), Duration::from_secs(5), || async {
                Err(ApiError::conflict("refused"))
            })
            .await;
        assert_eq!(refused.into_response().status(), StatusCode::CONFLICT);
        let empty = jobs
            .run("b".into(), Duration::from_secs(5), || async { Ok(None) })
            .await;
        assert_eq!(empty.into_response().status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn a_failed_job_reports_once_then_a_retry_runs_again() {
        let jobs = ControlJobs::default();
        let runs = Arc::new(AtomicUsize::new(0));
        let failing = |runs: Arc<AtomicUsize>| {
            move || async move {
                runs.fetch_add(1, Ordering::SeqCst);
                Err(ApiError::conflict("workspace stop timed out"))
            }
        };
        let first = jobs
            .run("k".into(), Duration::from_secs(5), failing(runs.clone()))
            .await;
        assert_eq!(first.into_response().status(), StatusCode::CONFLICT);
        let retry = jobs
            .run("k".into(), Duration::from_secs(5), failing(runs.clone()))
            .await;
        assert_eq!(retry.into_response().status(), StatusCode::CONFLICT);
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_failure_after_pending_reaches_the_repeat_request() {
        let jobs = ControlJobs::default();
        let gate = Arc::new(Notify::new());
        let opened = gate.clone();
        let first = jobs
            .run("k".into(), Duration::from_millis(20), move || async move {
                opened.notified().await;
                Err(ApiError::conflict("refused"))
            })
            .await;
        assert!(matches!(first, JobOutcome::Pending));
        gate.notify_one();
        let repeat = jobs
            .run("k".into(), Duration::from_secs(5), || async {
                panic!("a pending job must not start again")
            })
            .await;
        assert_eq!(repeat.into_response().status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn retention_counts_from_the_finish_not_the_start() {
        let jobs = ControlJobs::with_retention(Duration::from_millis(300));
        let first = jobs
            .run("k".into(), Duration::from_millis(10), || async {
                tokio::time::sleep(Duration::from_millis(400)).await;
                Ok(Some(serde_json::json!({ "done": true })))
            })
            .await;
        assert!(matches!(first, JobOutcome::Pending));
        tokio::time::sleep(Duration::from_millis(500)).await;
        let repeat = jobs
            .run("k".into(), Duration::from_secs(5), || async {
                panic!("a job finished within retention must not start again")
            })
            .await;
        assert!(matches!(repeat, JobOutcome::Finished(Ok(Some(_)))));
    }

    #[tokio::test]
    async fn a_panicked_job_names_its_failure() {
        let jobs = ControlJobs::default();
        let outcome = jobs
            .run("k".into(), Duration::from_secs(5), || async {
                panic!("job panicked")
            })
            .await;
        assert_eq!(
            outcome.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
