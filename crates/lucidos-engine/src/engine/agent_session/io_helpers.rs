use crate::engine::AgentUserInput;

/// Drain any follow-up messages that were queued in `msg_rx` but never consumed
/// (e.g. because the CC process exited before reading them). Returns them so
/// callers can re-process them with a fresh Claude Code session instead of showing
/// a scary "interrupted" banner.
pub(super) fn drain_lost_followups(
    msg_rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentUserInput>,
) -> Vec<AgentUserInput> {
    let lost: Vec<_> = std::iter::from_fn(|| msg_rx.try_recv().ok()).collect();
    if !lost.is_empty() {
        log!("[ClaudeCode] Drained {} lost follow-up(s)", lost.len());
    }
    lost
}

/// Convert drained CC follow-ups into orphaned injections for re-processing.
/// Only user follow-ups (with `origin_event_id`) are kept — internal messages
/// (auto-harden etc.) are dropped since they have no user-visible exchange.
pub(super) fn lost_followups_to_orphans(
    lost: Vec<AgentUserInput>,
) -> Vec<crate::engine::InjectedPrompt> {
    lost.into_iter()
        .filter_map(|input| {
            let eid = input.origin_event_id?;
            Some(crate::engine::InjectedPrompt {
                text: input.text,
                event_id: Some(eid),
                mode: crate::engine::thread_events::ActorMode::Human,
                spawning_event_id: None,
                images: input.images,
                origin: None,
                kind: crate::engine::InjectedPromptKind::UserText,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_returns_pending_followups() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        let eid = uuid::Uuid::new_v4();
        tx.send(AgentUserInput {
            text: "follow-up 1".into(),
            images: None,
            origin_event_id: Some(eid),
            kind: crate::engine::AgentInputKind::User,
        })
        .unwrap();
        tx.send(AgentUserInput {
            text: "internal".into(),
            images: None,
            origin_event_id: None,
            kind: crate::engine::AgentInputKind::User,
        })
        .unwrap();

        let lost = drain_lost_followups(&mut rx);
        assert_eq!(lost.len(), 2, "drain must return all pending messages");
        assert_eq!(lost[0].origin_event_id, Some(eid));
        assert!(lost[1].origin_event_id.is_none());
        assert!(rx.try_recv().is_err(), "channel must be empty after drain");
    }

    #[test]
    fn drain_empty_channel_returns_empty() {
        let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        let lost = drain_lost_followups(&mut rx);
        assert!(lost.is_empty());
    }
}
