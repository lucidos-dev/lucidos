//! A Lucidos Agent *side question*: one model call beside the thread.
//!
//! It sends a turn's own prefix, the system prompt and the tools, so the prompt
//! cache serves it. Then come the thread's history and the question. It is
//! never a turn: the only thing it records here is its cost. A tool call in the
//! reply is refused and never run, as a Claude Code side question's hook does.

use crate::api::ChatImage;
use crate::engine::LucidosEngine;
use crate::llm::{ContentBlock, Message, MessageContent};
use uuid::Uuid;

use super::super::images::build_user_content_with_images;
use crate::engine::agent_session::side_question::{
    kept_reaching_for_tools, side_question_timeout_message, SIDE_QUESTION_INSTRUCTIONS,
    SIDE_QUESTION_MAX_TURNS, SIDE_QUESTION_TOOL_REFUSAL,
};

/// How the asker reads this agent in a failure.
const AGENT: &str = "The Lucidos Agent";

impl LucidosEngine {
    /// Answer a side question in a Lucidos Agent thread, from the thread's
    /// history and the agent's own model. `deadline` bounds each model call,
    /// never the capture that records what it cost.
    pub(crate) async fn answer_lucidos_side_question(
        &self,
        thread_id: Uuid,
        question: &str,
        images: &[ChatImage],
        deadline: tokio::time::Instant,
    ) -> Result<String, String> {
        let capabilities = self.read_turn_capabilities().await;
        let context_mode = super::context_mode::ContextMode::from_capabilities(&capabilities.gates);
        let turn_started_at = self.turn_started_at(thread_id).await;
        let history = self
            .load_chat_history(
                false,
                false,
                thread_id,
                question,
                turn_started_at,
                context_mode,
            )
            .await;
        let user_timezone = self.user_timezone.read().await.clone();
        let user_language = self.user_language.read().await.clone();
        let max_tool_calls = crate::core::PreferenceStore::max_tool_calls(&self.pool).await;
        let (system_prompt, _missing_preferences) = self
            .build_chat_system_prompt(
                &user_timezone,
                &user_language,
                None,
                &None,
                max_tool_calls,
                &capabilities,
            )
            .await;
        let tools = self.chat_turn_tools(&capabilities.gates).await;

        let configured = self.current_provider().configured_providers();
        let resolved = super::run::resolve_route_overrides(
            &self.pool,
            &self.model_registry,
            |kind| configured.as_ref().is_none_or(|set| set.contains(&kind)),
            None,
            Some(thread_id),
            None,
            crate::core::ResolvedModelSelection::default(),
        )
        .await;

        let framing = format!("[SIDE QUESTION]\n{SIDE_QUESTION_INSTRUCTIONS}\n[END SIDE QUESTION]");
        let text = [history.history_context.as_str(), &framing, question]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(super::context_mode::PART_SEPARATOR);
        let mut messages = if context_mode.is_on() {
            Vec::new()
        } else {
            history.resume_tool_blocks
        };
        messages.push(Message {
            role: "user".to_string(),
            content: build_user_content_with_images(
                text,
                &self.workspace_path,
                &history.history_image_hashes,
                Some(images),
            ),
        });

        let provider = self.current_provider();
        let model = resolved
            .model
            .clone()
            .unwrap_or_else(|| provider.default_model().to_string());
        let capture = crate::engine::AuxCapture::new(
            &self.event_bus,
            thread_id,
            crate::engine::ContextPurpose::SideQuestion,
        );
        for _ in 0..SIDE_QUESTION_MAX_TURNS {
            let request_chars =
                crate::engine::context::request_chars(&system_prompt, &messages, tools.defs());
            let response = tokio::time::timeout_at(
                deadline,
                provider.chat(
                    messages.clone(),
                    tools.defs().to_vec(),
                    resolved.as_selection(),
                    Some(&system_prompt),
                    None,
                ),
            )
            .await
            .map_err(|_| side_question_timeout_message(AGENT))?
            .map_err(|e| format!("{AGENT} could not answer: {e}"))?;
            capture.record(&model, request_chars, &response).await;
            if response.tool_calls.is_empty() {
                return response
                    .content
                    .map(|answer| answer.trim().to_string())
                    .filter(|answer| !answer.is_empty())
                    .ok_or_else(|| format!("{AGENT} sent an empty side answer"));
            }
            refuse_tool_calls(&mut messages, &response);
        }
        Err(kept_reaching_for_tools(AGENT))
    }
}

/// Append the reply's tool calls and a refusal for each, so the next round
/// answers instead. Nothing runs.
fn refuse_tool_calls(messages: &mut Vec<Message>, response: &crate::llm::provider::LlmResponse) {
    let mut asked: Vec<ContentBlock> = response
        .history_text()
        .map(|text| ContentBlock::Text {
            text: text.to_string(),
        })
        .into_iter()
        .collect();
    asked.extend(
        response
            .tool_calls
            .iter()
            .map(|call| ContentBlock::ToolUse {
                id: call.id.clone(),
                name: call.name.clone(),
                input: call.arguments.clone(),
                thought_signature: call.thought_signature.clone(),
            }),
    );
    messages.push(Message {
        role: "assistant".to_string(),
        content: MessageContent::Blocks(asked),
    });
    messages.push(Message {
        role: "user".to_string(),
        content: MessageContent::Blocks(
            response
                .tool_calls
                .iter()
                .map(|call| ContentBlock::ToolResult {
                    tool_use_id: call.id.clone(),
                    content: SIDE_QUESTION_TOOL_REFUSAL.to_string(),
                })
                .collect(),
        ),
    });
}

#[cfg(test)]
#[path = "side_question_tests.rs"]
mod tests;
