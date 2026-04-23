use crate::core::changes;

/// Lifecycle events that close out a CC turn.
pub(crate) const CC_TURN_CLOSER_EVENTS: &str =
    "'CodingAgentIdled', 'SessionEnded', 'ChangeApplied', 'ChangeDiscarded'";

/// Resolve `(resume_session_id, resume_branch)` for a follow-up CC request.
/// Priority: pending-change branch > caller-supplied session > most recent
/// `CodingAgentIdled` > fresh start. Pending-change branch wins because the
/// change-proposal flow removes the worktree but keeps the branch's commits.
pub(super) async fn resolve_resume_context(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    caller_session_id: Option<String>,
) -> (Option<String>, Option<String>) {
    let pending_branch = match changes::pending_for_thread(pool, thread_id).await {
        Ok(mut pending) => pending.pop().map(|c| c.branch_name),
        Err(e) => {
            log!(
                "[ClaudeCode] Failed to look up pending changes for resume: {}",
                e
            );
            None
        }
    };

    if let Some(branch) = pending_branch {
        log!(
            "[ClaudeCode] Resuming on pending-change branch {} for thread {}",
            branch,
            thread_id
        );
        return (None, Some(branch));
    }

    if caller_session_id.is_some() {
        let branch = lookup_session_branch_for_thread(pool, thread_id).await;
        return (caller_session_id, branch);
    }

    let query = format!(
        "SELECT event_type, payload->>'cc_session_id' FROM events \
         WHERE thread_id = $1 AND event_type IN ({}) \
         ORDER BY sequence DESC LIMIT 1",
        CC_TURN_CLOSER_EVENTS,
    );
    let last_lifecycle = sqlx::query_as::<_, (String, Option<String>)>(&query)
        .bind(thread_id)
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|e| {
            log!(
                "[ClaudeCode] Failed to look up last lifecycle event for resume: {}",
                e
            );
            None
        });

    if let Some((event_type, sid)) = last_lifecycle.as_ref() {
        if event_type == "CodingAgentIdled" {
            let resume_sid = sid.clone().filter(|s| !s.is_empty());
            if resume_sid.is_some() {
                let branch = lookup_session_branch_for_thread(pool, thread_id).await;
                return (resume_sid, branch);
            }
        }
    }

    (None, None)
}

async fn lookup_session_branch_for_thread(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT payload->>'branch' FROM events \
         WHERE thread_id = $1 AND event_type = 'SessionStarted' \
           AND payload->>'branch' IS NOT NULL AND payload->>'branch' != '' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        log!(
            "[ClaudeCode] Failed to look up session branch for {}: {}",
            thread_id,
            e
        );
        e
    })
    .ok()
    .flatten()
}

/// Get a fallback description for a change proposal: thread title if available, else branch name.
pub(crate) async fn change_description_fallback(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    branch_name: &str,
) -> String {
    let title: Option<String> =
        match sqlx::query_scalar("SELECT title FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                log!(
                    "[ClaudeCode] Failed to fetch thread title for change description: {}",
                    e
                );
                None
            }
        };

    match title {
        Some(t) if !t.is_empty() => t,
        _ => branch_name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{
        ActorMode, EventChannel, EventMeta, SessionEndReason, ThreadEvent,
    };
    use crate::test_support::{setup_test_db, teardown_test_db};
    use sqlx::PgPool;
    use uuid::Uuid;

    fn cc_meta() -> EventMeta {
        EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        }
    }

    async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: cc_meta(),
        })
        .await
        .unwrap();
    }

    async fn seed_session_started(bus: &EventBus, thread_id: Uuid, session_id: &str, branch: &str) {
        emit(
            bus,
            thread_id,
            ThreadEvent::MessageReceived {
                text: "go".into(),
                images: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
        )
        .await;
        emit(
            bus,
            thread_id,
            ThreadEvent::SessionStarted {
                session_id: session_id.into(),
                branch: branch.into(),
                repo_id: None,
            },
        )
        .await;
    }

    async fn seed_pending_change(pool: &PgPool, thread_id: Uuid, branch: &str) -> Uuid {
        let change_id = Uuid::new_v4();
        changes::apply_change_proposed(
            pool,
            change_id,
            Uuid::new_v4(),
            Some(thread_id),
            branch,
            "/tmp/repo",
            "work",
            &["src/x.rs".to_string()],
            false,
            true,
        )
        .await
        .unwrap();
        change_id
    }

    #[tokio::test]
    async fn pending_change_after_session_ended_resumes_branch() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let branch = "claude-code/pending";

        seed_session_started(&bus, thread_id, "sess-1", branch).await;
        seed_pending_change(&pool, thread_id, branch).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::SessionEnded {
                reason: SessionEndReason::ChangesProposed,
            },
        )
        .await;

        let (sid, resume_branch) = resolve_resume_context(&pool, thread_id, None).await;
        assert_eq!(sid, None);
        assert_eq!(resume_branch, Some(branch.to_string()));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn pending_change_overrides_wrong_branch_idle_session() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let canonical_branch = "claude-code/canonical";
        let wrong_branch = "claude-code/wrong";

        seed_session_started(&bus, thread_id, "real-session", canonical_branch).await;
        seed_pending_change(&pool, thread_id, canonical_branch).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::SessionEnded {
                reason: SessionEndReason::ChangesProposed,
            },
        )
        .await;

        emit(
            &bus,
            thread_id,
            ThreadEvent::SessionStarted {
                session_id: "wrong-session".into(),
                branch: wrong_branch.into(),
                repo_id: None,
            },
        )
        .await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: Some("wrong-session".into()),
                agent: crate::runtime::AgentKind::ClaudeCode,
            },
        )
        .await;

        let (sid, resume_branch) = resolve_resume_context(&pool, thread_id, None).await;
        assert_eq!(sid, None);
        assert_eq!(resume_branch, Some(canonical_branch.to_string()));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn applied_change_falls_through_to_fresh_start() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let branch = "claude-code/applied";

        seed_session_started(&bus, thread_id, "sess-1", branch).await;
        let change_id = seed_pending_change(&pool, thread_id, branch).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::SessionEnded {
                reason: SessionEndReason::ChangesProposed,
            },
        )
        .await;
        changes::apply_change_applied(&pool, change_id, &[])
            .await
            .unwrap();

        let (sid, resume_branch) = resolve_resume_context(&pool, thread_id, None).await;
        assert_eq!(sid, None);
        assert_eq!(resume_branch, None);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
