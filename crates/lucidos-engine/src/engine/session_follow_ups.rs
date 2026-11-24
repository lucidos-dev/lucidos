//! Recover inputs the coding agent did not read.

use crate::engine::AgentUserInput;

/// Drain any follow-up messages that were queued in `msg_rx` but never consumed
/// (e.g. because the CC process exited before reading them). Returns them so
/// callers can re-process them with a fresh Claude Code session instead of showing
/// a scary "interrupted" banner.
pub fn drain_lost_followups(
    msg_rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentUserInput>,
) -> Vec<AgentUserInput> {
    let lost: Vec<_> = std::iter::from_fn(|| msg_rx.try_recv().ok()).collect();
    if !lost.is_empty() {
        log_legacy!("[AgentSession] Drained {} lost follow-up(s)", lost.len());
    }
    lost
}

/// Convert drained CC follow-ups into orphaned injections for re-processing,
/// each keeping its kind. Only inputs with an `origin_event_id` are kept.
/// An unread Continue keeps its instructions as engine re-entry. Redelivered
/// non-human inputs stay owed in the store for the next session instead.
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
