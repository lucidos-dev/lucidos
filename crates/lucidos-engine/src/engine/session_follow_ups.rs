use crate::engine::AgentUserInput;

pub fn drain_lost_followups(
    msg_rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentUserInput>,
) -> Vec<AgentUserInput> {
    let lost: Vec<_> = std::iter::from_fn(|| msg_rx.try_recv().ok()).collect();
    if !lost.is_empty() {
        log_legacy!("[AgentSession] Drained {} lost follow-up(s)", lost.len());
    }
    lost
}

pub fn lost_followups_to_orphans(lost: Vec<AgentUserInput>) -> Vec<crate::engine::InjectedPrompt> {
    use crate::engine::thread_events::ActorMode;
    use crate::engine::{AgentInputKind, InjectedPromptKind, RedeliveredInput};
    lost.into_iter()
        .filter_map(|input| {
            let (mode, kind) = match input.kind {
                AgentInputKind::User
                | AgentInputKind::Redelivered {
                    input:
                        RedeliveredInput::Message {
                            sender: ActorMode::Human,
                        },
                    ..
                } => (ActorMode::Human, InjectedPromptKind::UserText),
                AgentInputKind::ReentryFromEngine
                | AgentInputKind::CeilingReentry
                | AgentInputKind::Continuation => {
                    (ActorMode::Agent, InjectedPromptKind::ReentryFromEngine)
                }
                AgentInputKind::Redelivered { .. } => return None,
            };
            Some(crate::engine::InjectedPrompt {
                text: input.text,
                event_id: Some(input.origin_event_id?),
                mode,
                spawning_event_id: None,
                images: input.images,
                origin: None,
                kind,
            })
        })
        .collect()
}
