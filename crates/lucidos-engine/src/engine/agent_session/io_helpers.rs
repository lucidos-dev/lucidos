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
        log!("[AgentSession] Drained {} lost follow-up(s)", lost.len());
    }
    lost
}

/// Convert drained CC follow-ups into orphaned injections for re-processing.
/// Only user follow-ups (with `origin_event_id`) are kept. Internal messages
/// (auto-harden etc.) are dropped since they have no user-visible exchange.
/// So is a redelivered input that is not a human message: only a discard
/// strands one, and the store keeps it owed for the next session.
pub(super) fn lost_followups_to_orphans(
    lost: Vec<AgentUserInput>,
) -> Vec<crate::engine::InjectedPrompt> {
    use crate::engine::thread_events::ActorMode;
    use crate::engine::{AgentInputKind, RedeliveredInput};
    lost.into_iter()
        .filter_map(|input| {
            if let AgentInputKind::Redelivered { input: kind, .. } = input.kind {
                if kind
                    != (RedeliveredInput::Message {
                        sender: ActorMode::Human,
                    })
                {
                    return None;
                }
            }
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

/// Put `input`, just received, back at the head of `msg_rx`. The caller holds
/// the `agent_sessions` lock every sender takes, so nothing slips in between.
pub(super) fn requeue_at_head(
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    msg_rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentUserInput>,
    input: AgentUserInput,
) {
    let behind: Vec<_> = std::iter::from_fn(|| msg_rx.try_recv().ok()).collect();
    // The caller holds the open receiver, so a send cannot fail.
    for input in std::iter::once(input).chain(behind) {
        let _ = msg_tx.send(input);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(text: &str) -> AgentUserInput {
        AgentUserInput {
            text: text.into(),
            images: None,
            origin_event_id: None,
            kind: crate::engine::AgentInputKind::User,
            completion: None,
        }
    }

    #[test]
    fn requeue_puts_the_input_back_ahead_of_what_queued_behind_it() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        tx.send(input("first")).unwrap();
        tx.send(input("second")).unwrap();
        let first = rx.try_recv().unwrap();
        requeue_at_head(&tx, &mut rx, first);
        let order: Vec<String> = drain_lost_followups(&mut rx)
            .into_iter()
            .map(|input| input.text)
            .collect();
        assert_eq!(order, ["first", "second"]);
    }

    #[test]
    fn drain_returns_unconsumed_followups() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        let eid = uuid::Uuid::new_v4();
        tx.send(AgentUserInput {
            text: "follow-up 1".into(),
            images: None,
            origin_event_id: Some(eid),
            kind: crate::engine::AgentInputKind::User,
            completion: None,
        })
        .unwrap();
        tx.send(AgentUserInput {
            text: "internal".into(),
            images: None,
            origin_event_id: None,
            kind: crate::engine::AgentInputKind::User,
            completion: None,
        })
        .unwrap();

        let lost = drain_lost_followups(&mut rx);
        assert_eq!(lost.len(), 2, "drain must return all pending messages");
        assert_eq!(lost[0].origin_event_id, Some(eid));
        assert!(lost[1].origin_event_id.is_none());
        assert!(rx.try_recv().is_err(), "channel must be empty after drain");
    }

    /// A discarded wake stays owed, so the next session redelivers it as a
    /// wake rather than as a message the user never sent.
    #[test]
    fn only_a_redelivered_human_message_becomes_an_orphan() {
        use crate::engine::thread_events::ActorMode;
        use crate::engine::{AgentInputKind, RedeliveredInput};
        let redelivered = |input| AgentUserInput {
            text: "late".into(),
            images: None,
            origin_event_id: Some(uuid::Uuid::new_v4()),
            kind: AgentInputKind::Redelivered {
                sent_at: chrono::Utc::now(),
                input,
            },
            completion: None,
        };
        let human = redelivered(RedeliveredInput::Message {
            sender: ActorMode::Human,
        });
        let human_id = human.origin_event_id;
        let orphans = lost_followups_to_orphans(vec![
            human,
            redelivered(RedeliveredInput::ChildReport),
            redelivered(RedeliveredInput::EnginePrompt),
            redelivered(RedeliveredInput::Message {
                sender: ActorMode::Engine,
            }),
        ]);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].event_id, human_id);
    }

    #[test]
    fn drain_empty_channel_returns_empty() {
        let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        let lost = drain_lost_followups(&mut rx);
        assert!(lost.is_empty());
    }
}
