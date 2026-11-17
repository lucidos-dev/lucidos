//! Fit-only renderer credentials. Never an actor or an owner credential.

use super::widget_fit::FitBoxReport;
use rand::{distributions::Alphanumeric, Rng};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use uuid::Uuid;

pub(crate) const SESSION_TTL: Duration = Duration::from_secs(120);
const SESSION_LIMIT: usize = 128;
const SCOPE_TEXT_LIMIT: usize = 256;

/// The workspace is implicit: each engine owns a distinct registry.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FitScope {
    pub thread: Uuid,
    pub app: String,
    pub device: String,
}

/// Only the registration response carries this secret, directly to the launcher.
/// Deliberately neither Debug nor Serialize, so incidental logging cannot leak it.
pub(crate) struct FitGrant {
    pub handle: String,
    pub credential: String,
}

struct SelectedFit {
    request: String,
    tx: oneshot::Sender<Vec<FitBoxReport>>,
}

struct FitSession {
    scope: FitScope,
    credential_hash: [u8; 32],
    expires: Instant,
    selected: Option<SelectedFit>,
}

/// Allowed-ephemeral: both the waiting render and its credential die on restart.
/// Unused sessions expire; canceled receivers are purged before every operation.
#[derive(Default)]
pub(crate) struct FitSessions {
    entries: Mutex<HashMap<String, FitSession>>,
}

impl FitSessions {
    fn prune(entries: &mut HashMap<String, FitSession>) {
        let now = Instant::now();
        entries.retain(|_, session| {
            session.expires > now
                && session
                    .selected
                    .as_ref()
                    .is_none_or(|fit| !fit.tx.is_closed())
        });
    }

    /// `source` MUST come from verified subprocess provenance, never a body id.
    pub(crate) fn register(
        &self,
        source: Option<Uuid>,
        scope: FitScope,
    ) -> Result<FitGrant, &'static str> {
        if source != Some(scope.thread) {
            return Err("a verified origin for this thread is required");
        }
        if [&scope.app, &scope.device]
            .iter()
            .any(|s| s.is_empty() || s.len() > SCOPE_TEXT_LIMIT)
        {
            return Err("widget and logical device must be nonempty and bounded");
        }
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| "renderer registry unavailable")?;
        Self::prune(&mut entries);
        if entries.len() >= SESSION_LIMIT {
            return Err("too many fit renderer sessions");
        }
        let credential: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(48)
            .map(char::from)
            .collect();
        let handle = Uuid::new_v4().to_string();
        entries.insert(
            handle.clone(),
            FitSession {
                scope,
                credential_hash: Sha256::digest(credential.as_bytes()).into(),
                expires: Instant::now() + SESSION_TTL,
                selected: None,
            },
        );
        Ok(FitGrant { handle, credential })
    }

    /// Bind a single check to the public handle and all of its original scope.
    pub(crate) fn select(
        &self,
        handle: &str,
        scope: &FitScope,
        request: &str,
    ) -> Result<oneshot::Receiver<Vec<FitBoxReport>>, &'static str> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| "renderer registry unavailable")?;
        Self::prune(&mut entries);
        let session = entries
            .get_mut(handle)
            .ok_or("fit renderer expired or unknown")?;
        if &session.scope != scope || session.selected.is_some() {
            return Err("fit renderer scope mismatch or already selected");
        }
        let (tx, rx) = oneshot::channel();
        session.selected = Some(SelectedFit {
            request: request.to_string(),
            tx,
        });
        Ok(rx)
    }

    /// Resolve only this session's selected request. Refusals never consume it.
    pub(crate) fn answer(
        &self,
        credential: &str,
        request: &str,
        device: &str,
        boxes: Vec<FitBoxReport>,
    ) -> Result<(), &'static str> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| "renderer registry unavailable")?;
        Self::prune(&mut entries);
        let hash: [u8; 32] = Sha256::digest(credential.as_bytes()).into();
        let handle = entries
            .iter()
            .find_map(|(handle, session)| {
                (session.credential_hash == hash
                    && session.scope.device == device
                    && session
                        .selected
                        .as_ref()
                        .is_some_and(|fit| fit.request == request))
                .then(|| handle.clone())
            })
            .ok_or("fit renderer credential or request mismatch")?;
        let session = entries
            .remove(&handle)
            .expect("selected under the same lock");
        session
            .selected
            .expect("selected above")
            .tx
            .send(boxes)
            .map_err(|_| "fit request was canceled")
    }

    /// Drop an unsent or timed-out request and its credential.
    pub(crate) fn forget(&self, handle: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> FitScope {
        FitScope {
            thread: uuid::Uuid::new_v4(),
            app: "example-widget".into(),
            device: "renderer".into(),
        }
    }

    #[test]
    fn registration_requires_the_verified_source_thread() {
        let sessions = FitSessions::default();
        let scope = scope();
        assert!(sessions.register(None, scope.clone()).is_err());
        assert!(sessions
            .register(Some(uuid::Uuid::new_v4()), scope.clone())
            .is_err());
        assert!(sessions.register(Some(scope.thread), scope).is_ok());
    }

    #[tokio::test]
    async fn wrong_answers_leave_the_selected_request_open_and_replay_refuses() {
        let sessions = FitSessions::default();
        let scope = scope();
        let grant = sessions
            .register(Some(scope.thread), scope.clone())
            .unwrap();
        let other = sessions
            .register(Some(scope.thread), scope.clone())
            .unwrap();
        let mut wait = sessions.select(&grant.handle, &scope, "request").unwrap();
        assert!(sessions
            .answer(&other.credential, "request", &scope.device, vec![])
            .is_err());
        assert!(sessions
            .answer(&grant.credential, "request", "wrong-device", vec![])
            .is_err());
        assert!(sessions
            .answer(&grant.handle, "request", &scope.device, vec![])
            .is_err());
        assert!(wait.try_recv().is_err());
        sessions
            .answer(&grant.credential, "request", &scope.device, vec![])
            .unwrap();
        assert!(wait.await.unwrap().is_empty());
        assert!(sessions
            .answer(&grant.credential, "request", &scope.device, vec![])
            .is_err());
    }

    #[test]
    fn selection_is_bound_to_all_scope_fields_and_is_single_use() {
        let sessions = FitSessions::default();
        let scope = scope();
        let grant = sessions
            .register(Some(scope.thread), scope.clone())
            .unwrap();
        for wrong in [
            FitScope {
                thread: uuid::Uuid::new_v4(),
                ..scope.clone()
            },
            FitScope {
                app: "other-widget".into(),
                ..scope.clone()
            },
            FitScope {
                device: "other-device".into(),
                ..scope.clone()
            },
        ] {
            assert!(sessions.select(&grant.handle, &wrong, "request").is_err());
        }
        let _wait = sessions.select(&grant.handle, &scope, "request").unwrap();
        assert!(sessions.select(&grant.handle, &scope, "another").is_err());
    }

    #[test]
    fn expired_sessions_and_foreign_workspaces_cannot_answer() {
        let sessions = FitSessions::default();
        let scope = scope();
        let grant = sessions
            .register(Some(scope.thread), scope.clone())
            .unwrap();
        let mut wait = sessions.select(&grant.handle, &scope, "request").unwrap();
        assert!(FitSessions::default()
            .answer(&grant.credential, "request", &scope.device, vec![])
            .is_err());
        assert!(wait.try_recv().is_err());
        sessions
            .entries
            .lock()
            .unwrap()
            .get_mut(&grant.handle)
            .unwrap()
            .expires = Instant::now() - Duration::from_secs(1);
        assert!(sessions
            .answer(&grant.credential, "request", &scope.device, vec![])
            .is_err());
        assert!(sessions.select(&grant.handle, &scope, "request").is_err());
    }

    #[test]
    fn capacity_and_scope_text_are_bounded_and_expiry_frees_capacity() {
        let sessions = FitSessions::default();
        let scope = scope();
        assert!(sessions
            .register(
                Some(scope.thread),
                FitScope {
                    device: "x".repeat(SCOPE_TEXT_LIMIT + 1),
                    ..scope.clone()
                }
            )
            .is_err());
        assert!(sessions
            .register(
                Some(scope.thread),
                FitScope {
                    app: String::new(),
                    ..scope.clone()
                }
            )
            .is_err());
        for _ in 0..SESSION_LIMIT {
            sessions
                .register(Some(scope.thread), scope.clone())
                .unwrap();
        }
        assert!(sessions
            .register(Some(scope.thread), scope.clone())
            .is_err());
        for session in sessions.entries.lock().unwrap().values_mut() {
            session.expires = Instant::now() - Duration::from_secs(1);
        }
        assert!(sessions.register(Some(scope.thread), scope).is_ok());
    }

    #[test]
    fn cancellation_and_restart_invalidate_the_credential() {
        let sessions = FitSessions::default();
        let scope = scope();
        let grant = sessions
            .register(Some(scope.thread), scope.clone())
            .unwrap();
        let wait = sessions.select(&grant.handle, &scope, "request").unwrap();
        drop(wait);
        assert!(sessions
            .answer(&grant.credential, "request", &scope.device, vec![])
            .is_err());
        assert!(sessions.select(&grant.handle, &scope, "request").is_err());
        assert!(FitSessions::default()
            .answer(&grant.credential, "request", &scope.device, vec![])
            .is_err());
    }
}
