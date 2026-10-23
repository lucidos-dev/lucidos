//! Whether a new coding-agent session carried the thread's earlier context,
//! recorded on `SessionStarted` so the thread can say when one started over.

use crate::engine::agent_session::resume::CC_TURN_CLOSER_EVENTS;
use crate::engine::agent_session::spawn::ResumeAttempt;
use crate::engine::thread_events::{FreshSessionReason, SessionContinuity, SessionEndReason};

/// The continuity of the session about to start on `thread_id`. `None` when
/// the thread's history could not be read: an absent field draws nothing,
/// where a guess could tell the user something false. A pending `handoff`
/// makes it a *successor session*.
pub(super) async fn resolve_session_continuity(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    attempt: ResumeAttempt,
    branch_gone: bool,
    handoff: Option<&crate::engine::successor::PendingHandoff>,
) -> Option<SessionContinuity> {
    if let Some(handoff) = handoff.filter(|_| attempt != ResumeAttempt::Resumed) {
        return Some(handoff.continuity());
    }
    let query = format!(
        "SELECT \
           EXISTS (SELECT 1 FROM events WHERE thread_id = $1 AND event_type = 'SessionStarted'), \
           closer.event_type, closer.reason \
         FROM (SELECT 1) AS one \
         LEFT JOIN LATERAL ( \
           SELECT event_type, payload->>'reason' AS reason FROM events \
           WHERE thread_id = $1 AND event_type IN ({CC_TURN_CLOSER_EVENTS}) \
           ORDER BY sequence DESC LIMIT 1 \
         ) AS closer ON true",
    );
    let row = sqlx::query_as::<_, (bool, Option<String>, Option<String>)>(&query)
        .bind(thread_id)
        .fetch_one(pool)
        .await;
    match row {
        Ok((had_session_before, closer_type, closer_reason)) => Some(session_continuity(
            had_session_before,
            attempt,
            branch_gone,
            closer_type.as_deref(),
            closer_reason.as_deref(),
        )),
        Err(e) => {
            log!(
                "[AgentSession] Could not read thread {}'s sessions for continuity: {}",
                thread_id,
                e
            );
            None
        }
    }
}

/// Pure half of [`resolve_session_continuity`]. A refused resume names itself
/// first, then a gone branch, then whatever closed the previous turn.
fn session_continuity(
    had_session_before: bool,
    attempt: ResumeAttempt,
    branch_gone: bool,
    closer_type: Option<&str>,
    closer_reason: Option<&str>,
) -> SessionContinuity {
    let reason = match attempt {
        ResumeAttempt::Resumed => return SessionContinuity::Resumed,
        _ if !had_session_before => return SessionContinuity::First,
        ResumeAttempt::FellBack => FreshSessionReason::ResumeFailed,
        ResumeAttempt::NotAttempted if branch_gone => FreshSessionReason::BranchGone,
        ResumeAttempt::NotAttempted => fresh_reason_after(closer_type, closer_reason),
    };
    SessionContinuity::Fresh { reason }
}

/// Why no session was offered for resume, read off the event that closed the
/// previous turn. An idle that left nothing resumable, or no closer at all
/// (the session died mid-turn), has no session to offer.
fn fresh_reason_after(
    closer_type: Option<&str>,
    closer_reason: Option<&str>,
) -> FreshSessionReason {
    let end_reason = closer_reason
        .and_then(|r| serde_json::from_value::<SessionEndReason>(serde_json::Value::from(r)).ok());
    match closer_type {
        Some("ChangeApplied") => FreshSessionReason::ChangeApplied,
        Some("ChangeDiscarded") => FreshSessionReason::ChangeDiscarded,
        Some("SessionEnded") if end_reason == Some(SessionEndReason::StaleResume) => {
            FreshSessionReason::ResumeFailed
        }
        Some("SessionEnded") => FreshSessionReason::SessionEnded,
        _ => FreshSessionReason::NoResumableSession,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
    use crate::test_support::{setup_test_db, teardown_test_db};

    fn fresh(reason: FreshSessionReason) -> SessionContinuity {
        SessionContinuity::Fresh { reason }
    }

    #[test]
    fn a_taken_resume_is_resumed_and_a_first_session_is_first() {
        assert_eq!(
            session_continuity(true, ResumeAttempt::Resumed, false, None, None),
            SessionContinuity::Resumed
        );
        assert_eq!(
            session_continuity(false, ResumeAttempt::NotAttempted, false, None, None),
            SessionContinuity::First
        );
    }

    #[test]
    fn a_fallback_spawn_is_never_recorded_as_resumed() {
        assert_eq!(
            session_continuity(
                true,
                ResumeAttempt::FellBack,
                false,
                Some("CodingAgentIdled"),
                None
            ),
            fresh(FreshSessionReason::ResumeFailed)
        );
    }

    #[test]
    fn a_gone_branch_names_itself() {
        assert_eq!(
            session_continuity(
                true,
                ResumeAttempt::NotAttempted,
                true,
                Some("CodingAgentIdled"),
                None
            ),
            fresh(FreshSessionReason::BranchGone)
        );
    }

    #[test]
    fn the_previous_turn_closer_names_the_reason() {
        let after = |closer: Option<&str>, reason: Option<&str>| {
            session_continuity(true, ResumeAttempt::NotAttempted, false, closer, reason)
        };
        assert_eq!(
            after(Some("ChangeApplied"), None),
            fresh(FreshSessionReason::ChangeApplied)
        );
        assert_eq!(
            after(Some("ChangeDiscarded"), None),
            fresh(FreshSessionReason::ChangeDiscarded)
        );
        assert_eq!(
            after(Some("SessionEnded"), Some("stale_resume")),
            fresh(FreshSessionReason::ResumeFailed)
        );
        assert_eq!(
            after(Some("SessionEnded"), Some("shutdown")),
            fresh(FreshSessionReason::SessionEnded)
        );
        assert_eq!(
            after(Some("CodingAgentIdled"), None),
            fresh(FreshSessionReason::NoResumableSession)
        );
        assert_eq!(
            after(None, None),
            fresh(FreshSessionReason::NoResumableSession)
        );
    }

    /// A pending handoff makes the session a successor session, unless the
    /// spawn resumed after all, which a handoff never does.
    #[test]
    fn a_pending_handoff_names_the_successor_session() {
        let handoff: crate::engine::successor::PendingHandoff =
            serde_json::from_value(serde_json::json!({
                "opening": "brief", "message": "m", "checkpoint": "pre-harden",
                "coding_agent": "claude-code",
            }))
            .unwrap();
        let expected = SessionContinuity::Handoff {
            checkpoint: Some("pre-harden".into()),
            progress: None,
            opening: crate::engine::thread_events::HandoffOpening::Brief,
        };
        assert_eq!(handoff.continuity(), expected);
        assert_eq!(
            serde_json::to_value(&expected).unwrap(),
            serde_json::json!({ "kind": "handoff", "checkpoint": "pre-harden", "opening": "brief" })
        );
    }

    /// The wire spelling is the contract the client reads.
    #[test]
    fn continuity_serializes_kebab_case_on_kind() {
        assert_eq!(
            serde_json::to_value(fresh(FreshSessionReason::BranchGone)).unwrap(),
            serde_json::json!({ "kind": "fresh", "reason": "branch-gone" })
        );
    }

    /// Rows written before the field existed carry no `continuity` key.
    #[test]
    fn a_session_started_without_continuity_still_parses() {
        let mut row =
            serde_json::to_value(session_started(Some(SessionContinuity::First))).unwrap();
        let fields = row.as_object_mut().expect("an internally tagged object");
        assert!(fields.remove("continuity").is_some());
        let parsed: ThreadEvent = serde_json::from_value(row).unwrap();
        assert!(matches!(
            parsed,
            ThreadEvent::SessionStarted {
                continuity: None,
                ..
            }
        ));
    }

    fn session_started(continuity: Option<SessionContinuity>) -> ThreadEvent {
        ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: String::new(),
            branch: "lucidos-claude-code-repo-lucidos-fix-1a2b3c4d".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity,
        }
    }

    #[tokio::test]
    async fn continuity_reads_the_threads_history() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = uuid::Uuid::new_v4();
        assert_eq!(
            resolve_session_continuity(&pool, thread_id, ResumeAttempt::NotAttempted, false, None)
                .await,
            Some(SessionContinuity::First)
        );

        let meta = EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        };
        for event in [
            session_started(Some(SessionContinuity::First)),
            ThreadEvent::SessionEnded {
                reason: SessionEndReason::StaleResume,
            },
        ] {
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta: meta.clone(),
            })
            .await
            .unwrap();
        }
        assert_eq!(
            resolve_session_continuity(&pool, thread_id, ResumeAttempt::NotAttempted, false, None)
                .await,
            Some(fresh(FreshSessionReason::ResumeFailed))
        );
        let handoff: crate::engine::successor::PendingHandoff =
            serde_json::from_value(serde_json::json!({
                "opening": "narrative", "message": "m", "coding_agent": "codex",
            }))
            .unwrap();
        assert_eq!(
            resolve_session_continuity(
                &pool,
                thread_id,
                ResumeAttempt::NotAttempted,
                false,
                Some(&handoff)
            )
            .await,
            Some(handoff.continuity()),
            "a handoff outranks whatever closed the previous turn"
        );
        teardown_test_db(&db_name).await;
    }
}
