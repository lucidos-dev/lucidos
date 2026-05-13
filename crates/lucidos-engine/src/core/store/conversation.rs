use super::types::*;
use super::EventStore;
use crate::core::ArtifactManager;
use chrono::{DateTime, Utc};
use std::path::Path;
use uuid::Uuid;

impl EventStore {
    /// Get a snapshot of the conversation at a specific event
    pub async fn get_conversation_at_event(
        &self,
        event_id: Uuid,
        workspace_path: &Path,
    ) -> Result<ConversationSnapshot, Box<dyn std::error::Error + Send + Sync>> {
        let target_event = self
            .get_event_by_id(event_id)
            .await?
            .ok_or_else(|| format!("Event not found: {}", event_id))?;

        let events = self.get_events_until(target_event.created).await?;

        let mut messages: Vec<ConversationMessage> = Vec::new();
        let mut current_steps: Vec<Step> = Vec::new();
        let mut current_images: Vec<String> = Vec::new();
        let mut last_git_commit: Option<String> = None;
        let mut pending_user_message: Option<(Uuid, DateTime<Utc>, String)> = None;

        for event in &events {
            match event.event_type.as_str() {
                "MessageReceived" | "UserMessage" => {
                    if let Some((id, ts, content)) = pending_user_message.take() {
                        messages.push(ConversationMessage {
                            role: "user".to_string(),
                            content,
                            event_id: id,
                            timestamp: ts,
                            steps: vec![],
                            images: vec![],
                        });
                    }

                    let content = event
                        .payload
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    pending_user_message = Some((event.id, event.created, content));
                    current_steps.clear();
                    current_images.clear();
                }
                "Thinking" => {
                    // Legacy payload fields — kept for old DB rows. New
                    // emissions carry these on ContextCaptured.
                    let context_tokens = event
                        .payload
                        .get("context_tokens")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize);
                    let context_messages = event
                        .payload
                        .get("context_messages")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize);
                    let trimmed = event.payload.get("trimmed").and_then(|v| v.as_bool());
                    current_steps.push(Step {
                        description: "Requesting".to_string(),
                        tool_name: None,
                        success: true,
                        context_tokens,
                        context_messages,
                        trimmed,
                        tool_called_event_id: None,
                    });
                }
                "MemorySearched" => {
                    let has_results = event
                        .payload
                        .get("results")
                        .and_then(|v| v.as_u64())
                        .is_some_and(|n| n > 0);
                    let desc = if has_results { "Memory searched" } else { "Memory: no results" };
                    current_steps.push(Step {
                        description: desc.to_string(),
                        tool_name: None,
                        success: true,
                        context_tokens: None,
                        context_messages: None,
                        trimmed: None,
                        tool_called_event_id: None,
                    });
                }
                "ToolCalled" => {
                    let (tool_name, description) = super::describe_tool_event(event);
                    current_steps.push(Step {
                        description,
                        tool_name: Some(tool_name),
                        success: true,
                        context_tokens: None,
                        context_messages: None,
                        trimmed: None,
                        tool_called_event_id: Some(event.id.to_string()),
                    });
                }
                "ToolResult" => {
                    let success = event
                        .payload
                        .get("success")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true);

                    if let Some(last_step) = current_steps.last_mut() {
                        last_step.success = success;
                    }

                    let tool_name = event
                        .payload
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if tool_name == "browser_screenshot" && success {
                        if let Some(result) = event.payload.get("result").and_then(|v| v.as_str()) {
                            if let Some(start) = result.find("screenshots/") {
                                let path_part = &result[start..];
                                let end =
                                    path_part.find(['"', '\n', ' ']).unwrap_or(path_part.len());
                                let path = &path_part[..end];
                                current_images.push(path.to_string());
                            }
                        }
                    }
                }
                "ArtifactCreated" | "ArtifactUpdated" => {
                    if let Some(commit) = event.payload.get("commit").and_then(|v| v.as_str()) {
                        last_git_commit = Some(commit.to_string());
                    }
                }
                "ResponseAborted" | "ResponseCanceled" | "ResponseGenerated"
                | "AssistantResponse" => {
                    if let Some((id, ts, content)) = pending_user_message.take() {
                        messages.push(ConversationMessage {
                            role: "user".to_string(),
                            content,
                            event_id: id,
                            timestamp: ts,
                            steps: vec![],
                            images: vec![],
                        });
                    }

                    let content = event
                        .payload
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    messages.push(ConversationMessage {
                        role: "assistant".to_string(),
                        content,
                        event_id: event.id,
                        timestamp: event.created,
                        steps: std::mem::take(&mut current_steps),
                        images: std::mem::take(&mut current_images),
                    });
                }
                "ResponseFailed" => {
                    if let Some((id, ts, content)) = pending_user_message.take() {
                        messages.push(ConversationMessage {
                            role: "user".to_string(),
                            content,
                            event_id: id,
                            timestamp: ts,
                            steps: vec![],
                            images: vec![],
                        });
                    }

                    let error = event
                        .payload
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown error");
                    messages.push(ConversationMessage {
                        role: "assistant".to_string(),
                        content: format!("[ERROR] **Error:** {}", error),
                        event_id: event.id,
                        timestamp: event.created,
                        steps: std::mem::take(&mut current_steps),
                        images: std::mem::take(&mut current_images),
                    });
                }
                _ => {}
            }
        }

        if let Some((id, ts, content)) = pending_user_message {
            messages.push(ConversationMessage {
                role: "user".to_string(),
                content,
                event_id: id,
                timestamp: ts,
                steps: vec![],
                images: vec![],
            });
        }

        let artifact_manager = ArtifactManager::new(workspace_path.to_path_buf())?;
        let artifacts = if let Some(ref commit) = last_git_commit {
            artifact_manager
                .list_artifacts_at_commit(commit)
                .unwrap_or_default()
        } else {
            artifact_manager.list_artifacts().unwrap_or_default()
        };

        Ok(ConversationSnapshot {
            at_event_id: event_id,
            point_in_time: target_event.created,
            messages,
            artifacts,
            git_commit: last_git_commit,
        })
    }
}
