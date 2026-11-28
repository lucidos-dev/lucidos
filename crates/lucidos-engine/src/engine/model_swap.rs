use crate::runtime::intelligence_ladder::IntelligenceRung;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSwap {
    pub requested: IntelligenceRung,
    pub served: String,
    pub served_effort: Option<String>,
}

pub fn model_swap_reentry_text(swap: &ModelSwap, suggestions: &[IntelligenceRung]) -> String {
    let requested = &swap.requested.model;
    let served = match &swap.served_effort {
        Some(effort) => format!("{} at {effort}", swap.served),
        None => swap.served.clone(),
    };
    let rung_question = if suggestions.is_empty() {
        "1. Skip the rung question: no other model is on the Claude Code ladder.".to_string()
    } else {
        let options = suggestions
            .iter()
            .map(|p| format!("`{p}`"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "1. Which rung to use instead. Offer these rungs as the options: {options}. Apply \
             the pick by handing off on it: `lucidos successor start --narrative <file> \
             --coding-agent-model <model> --reasoning-effort <effort>`."
        )
    };
    format!(
        "Claude Code swapped {requested} for {} on this account, so the next session runs \
         {served}. Lucidos keeps {requested} on the ladder until the owner decides. Ask the \
         owner two questions, one card each.\n\n\
         {rung_question}\n\
         2. Whether to take {requested} off the Claude Code intelligence ladder. Run \
         `lucidos intelligence-ladder remove-model {requested} --surface claude-code --reason \
         \"<why>\"`. It prints a request id. Ask with that id as the only question, then rerun \
         the command once the owner allows it.\n\n\
         If the owner told you not to ask, say so in one line and end the turn.",
        swap.served
    )
}

impl crate::engine::LucidosEngine {
    /// INVARIANT: Send a live session the model-swap re-entry. The `PromptInjected`
    /// anchor starts the exchange its reply groups under.
    pub async fn send_model_swap_reentry(
        &self,
        thread_id: Uuid,
        swap: &ModelSwap,
        msg_tx: &tokio::sync::mpsc::UnboundedSender<crate::engine::AgentUserInput>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::engine::event_bus::BusEvent;
        use crate::engine::thread_events::{
            ActorMode, EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
        };
        let ladder = crate::runtime::intelligence_ladder::load(
            self.pool(),
            &self.model_routes(),
            crate::runtime::intelligence_ladder::LadderSurface::ClaudeCode,
        )
        .await;
        let refused = |model: &str| {
            crate::runtime::intelligence_ladder::is_refused(
                crate::runtime::CodingAgent::ClaudeCode,
                model,
            )
        };
        let text =
            model_swap_reentry_text(swap, &ladder.swap_suggestions(&swap.requested, refused));
        let origin = MessageOrigin::engine(EngineReason::ModelSubstituted);
        let anchor = self
            .event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::PromptInjected {
                    text: text.clone(),
                    mode: ActorMode::Engine,
                    origin: Some(origin.clone()),
                    injected_message_id: None,
                    delivered_event_id: None,
                },
                meta: EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    actor: Some(origin),
                    ..EventMeta::NONE
                },
            })
            .await?;
        msg_tx
            .send(crate::engine::AgentUserInput {
                text,
                images: None,
                origin_event_id: anchor.map(|a| a.event_id),
                kind: crate::engine::AgentInputKind::ReentryFromEngine,
                completion: None,
            })
            .map_err(|_| "Session channel closed".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swap() -> ModelSwap {
        ModelSwap {
            requested: IntelligenceRung::new("claude-fable-5-2", "low"),
            served: "claude-opus-5-5".to_string(),
            served_effort: Some("medium".to_string()),
        }
    }

    #[test]
    fn the_message_names_the_swap_the_rungs_and_both_commands() {
        let text = model_swap_reentry_text(
            &swap(),
            &[
                IntelligenceRung::new("claude-opus-5-5", "high"),
                IntelligenceRung::new("claude-sonnet-5-5", "high"),
            ],
        );
        assert!(
            text.contains("swapped claude-fable-5-2 for claude-opus-5-5"),
            "{text}"
        );
        assert!(text.contains("runs claude-opus-5-5 at medium"), "{text}");
        assert!(
            text.contains("`claude-opus-5-5=high`, `claude-sonnet-5-5=high`"),
            "{text}"
        );
        assert!(text.contains("lucidos successor start"), "{text}");
        assert!(
            text.contains(
                "lucidos intelligence-ladder remove-model claude-fable-5-2 --surface claude-code"
            ),
            "{text}"
        );
    }

    #[test]
    fn with_no_other_model_on_the_ladder_the_rung_question_is_skipped() {
        let text = model_swap_reentry_text(&swap(), &[]);
        assert!(text.contains("Skip the rung question"), "{text}");
        assert!(!text.contains("successor start"), "{text}");
        assert!(text.contains("remove-model claude-fable-5-2"), "{text}");
    }
}
