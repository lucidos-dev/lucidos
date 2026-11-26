use crate::engine::agent_session::external_edits;
use crate::engine::LucidosEngine;
use uuid::Uuid;

#[easy_ext::ext(IdleSnapshotExt)]
pub(crate) impl LucidosEngine {
    async fn emit_coding_agent_idled(
        &self,
        thread_id: Uuid,
        snapshot: CodingAgentIdleSnapshot<'_>,
        meta: &crate::engine::thread_events::EventMeta,
        coding_agent: crate::runtime::CodingAgent,
    ) {
        let cc_session_id = {
            let guard = self.agent_sessions.lock().await;
            guard
                .get(&thread_id)
                .and_then(|s| s.backend_session_id.clone())
        };
        let worktree_head_sha = match snapshot.worktree_path {
            Some(p) if snapshot.turn != IdleTurn::KeepAlive => {
                external_edits::git_head_sha(p).await
            }
            _ => None,
        };
        crate::engine::cc_permission::resolve_pending_permissions_as_session_ended(
            self.pool(),
            &self.event_bus,
            &self.pending_cc_permission,
            thread_id,
            None,
        )
        .await;
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                        has_changes: snapshot.has_changes,
                        is_external_repo: snapshot.is_external_repo,
                        requires_restart: snapshot.requires_restart,
                        cc_session_id,
                        coding_agent,
                        reason: snapshot.turn.reason().map(str::to_string),
                        worktree_path: snapshot
                            .worktree_path
                            .map(|p| p.to_string_lossy().into_owned()),
                        worktree_head_sha,
                        bg_bash_pending: snapshot.bg_bash_pending,
                        turn_end: snapshot.turn_end,
                    },
                    meta: meta.clone(),
                },
                "[AgentSession] CodingAgentIdled",
            )
            .await;
    }
}

pub(super) struct CodingAgentIdleSnapshot<'a> {
    pub(super) has_changes: bool,
    pub(super) is_external_repo: bool,
    pub(super) requires_restart: bool,
    pub(super) bg_bash_pending: bool,
    pub(super) worktree_path: Option<&'a std::path::Path>,
    pub(super) turn: IdleTurn,
    pub(super) turn_end: Option<crate::engine::thread_events::TurnEndFacts>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IdleTurn {
    Agent,
    KeepAlive,
    CeilingReentry,
}

impl IdleTurn {
    pub(super) fn of(keep_alive: bool, ceiling_reentry: bool) -> Self {
        if keep_alive {
            Self::KeepAlive
        } else if ceiling_reentry {
            Self::CeilingReentry
        } else {
            Self::Agent
        }
    }

    fn reason(self) -> Option<&'static str> {
        use crate::engine::thread_events::{CEILING_REENTRY_IDLE_REASON, KEEP_ALIVE_IDLE_REASON};
        match self {
            Self::Agent => None,
            Self::KeepAlive => Some(KEEP_ALIVE_IDLE_REASON),
            Self::CeilingReentry => Some(CEILING_REENTRY_IDLE_REASON),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::thread_events::{CEILING_REENTRY_IDLE_REASON, KEEP_ALIVE_IDLE_REASON};

    #[test]
    fn only_an_agent_turn_idles_without_a_reason() {
        assert_eq!(IdleTurn::Agent.reason(), None);
        assert_eq!(IdleTurn::KeepAlive.reason(), Some(KEEP_ALIVE_IDLE_REASON));
        assert_eq!(
            IdleTurn::CeilingReentry.reason(),
            Some(CEILING_REENTRY_IDLE_REASON)
        );
    }

    #[test]
    fn an_engine_turn_is_never_classified_as_an_agent_turn() {
        assert_eq!(IdleTurn::of(false, false), IdleTurn::Agent);
        assert_eq!(IdleTurn::of(true, false), IdleTurn::KeepAlive);
        assert_eq!(IdleTurn::of(false, true), IdleTurn::CeilingReentry);
        assert!(IdleTurn::of(true, true).reason().is_some());
    }
}
