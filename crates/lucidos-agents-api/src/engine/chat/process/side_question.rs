use crate::engine::chat::process::history::HistoryExt;
use crate::engine::chat::process::memory_views::MemoryViewsExt;
use crate::engine::chat::process::run::ProcessRunExt;
use crate::engine::chat::process::system_prompt::SystemPromptExt;
use crate::engine::chat::process::turn_clock::TurnClockExt;
use crate::engine::chat::rung::RungExt;
use crate::engine::tools::capabilities::CapabilitiesExt;
use crate::engine::types::ChatImage;
use crate::engine::LucidosEngine;
use crate::llm::{ContentBlock, Message, MessageContent};
use uuid::Uuid;

use super::super::images::build_user_content_with_images;
use crate::engine::side_question_policy::{
    kept_reaching_for_tools, side_question_timeout_message, SIDE_QUESTION_INSTRUCTIONS,
    SIDE_QUESTION_MAX_TURNS, SIDE_QUESTION_TOOL_REFUSAL,
};

const AGENT: &str = "The Lucidos Agent";

#[easy_ext::ext(ProcessSideQuestionExt)]
pub(crate) impl LucidosEngine {
    async fn answer_lucidos_side_question(
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
                None,
                thread_id,
                question,
                turn_started_at,
                &capabilities.gates,
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

        let overrides = self
            .running_turn_overrides(thread_id)
            .await
            .map_err(|e| format!("{AGENT} could not read its running model: {e}"))?;
        let resolved = super::run::resolve_available_route_overrides(
            &self.pool,
            &self.model_routes(),
            None,
            Some(thread_id),
            None,
            overrides,
        )
        .await;

        let provider = self.current_provider();
        let model = resolved
            .model
            .clone()
            .unwrap_or_else(|| provider.default_model().to_string());
        let context_window = self.context_window_for(&model, resolved.as_selection().provider);
        let budget_model = self.request_budget_model_for(&model, resolved.as_selection().provider);
        let views = if capabilities.gates.memory_tree {
            let surface = self.chat_surface(thread_id, false).await;
            let limit = crate::engine::context::agent_context_char_budget_for(
                &budget_model,
                context_window,
            ) / 2;
            self.turn_memory_views(thread_id, surface, &model, question, limit)
                .await
        } else {
            Default::default()
        };

        let framing = format!("[SIDE QUESTION]\n{SIDE_QUESTION_INSTRUCTIONS}\n[END SIDE QUESTION]");
        let text = [
            views.recent.as_str(),
            history.history_context.as_str(),
            &framing,
            question,
        ]
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
            content: views.lead(
                String::new(),
                build_user_content_with_images(
                    text,
                    &self.workspace_path,
                    &history.history_image_hashes,
                    Some(images),
                    &[],
                ),
            ),
        });

        let tools = self
            .chat_turn_tools(
                thread_id,
                &capabilities.gates,
                context_window,
                &budget_model,
            )
            .await;
        let capture = crate::engine::AuxCapture::new(
            &self.event_bus,
            thread_id,
            crate::engine::ContextPurpose::SideQuestion,
        );
        let capture = capture.until(deadline);
        for _ in 0..SIDE_QUESTION_MAX_TURNS {
            let response = capture
                .chat(
                    provider.as_ref(),
                    messages.clone(),
                    tools.defs().to_vec(),
                    resolved.as_selection(),
                    Some(&system_prompt),
                    None,
                )
                .await
                .map_err(|e| {
                    if e.is::<tokio::time::error::Elapsed>() {
                        side_question_timeout_message(AGENT)
                    } else {
                        format!("{AGENT} could not answer: {e}")
                    }
                })?;
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
