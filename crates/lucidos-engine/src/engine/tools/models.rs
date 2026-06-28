//! Handler for the `manage_models` tool — the agent-facing path to the chat
//! model registry (Settings → Models). Wraps `ModelStore` and emits the same
//! `Model{Created,Updated,Deleted}` events the HTTP CRUD does, so the in-memory
//! `ModelRegistry` reloads (`spawn_models_registry_subscriber`) and the picker
//! updates without a restart. Mirrors `manage_repositories`' multi-action shape.
//!
//! Switching the *active* chat model is a preference (`chat_model`) handled by
//! `set_preference`; this tool manages which models EXIST in the picker.

use super::super::LucidosEngine;
use crate::core::models::ModelStore;
use crate::engine::event_bus::{BusEvent, SystemEvent};

/// Providers the registry accepts — kept in lockstep with
/// `api::settings::valid_provider` / `llm::model_registry::ProviderKind`.
const VALID_PROVIDERS: &[&str] = &["vertex", "anthropic", "openai", "openrouter", "local"];

impl LucidosEngine {
    pub(crate) async fn execute_manage_models(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let action = args["action"].as_str().unwrap_or("").trim();
        match action {
            "list" => self.manage_models_list().await,
            "add" => self.manage_models_add(args).await,
            "enable" => self.manage_models_set_enabled(args, true).await,
            "disable" => self.manage_models_set_enabled(args, false).await,
            "remove" => self.manage_models_remove(args).await,
            "" => Ok("Error: action is required (one of: list, add, enable, disable, remove)".to_string()),
            other => Ok(format!(
                "Error: unknown action '{}'. Use one of: list, add, enable, disable, remove.",
                other
            )),
        }
    }

    async fn manage_models_list(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let models = ModelStore::list(&self.pool).await?;
        if models.is_empty() {
            return Ok("No models registered.".to_string());
        }
        let mut out = format!("{} models in the registry:\n", models.len());
        for m in &models {
            out.push_str(&format!(
                "- {} — \"{}\" | provider: {} | {} | {}\n",
                m.id,
                m.label,
                m.provider,
                if m.enabled { "enabled" } else { "disabled" },
                if m.is_builtin() { "builtin" } else { "user" },
            ));
        }
        Ok(out)
    }

    async fn manage_models_add(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let id = args["id"].as_str().unwrap_or("").trim();
        let provider = args["provider"].as_str().unwrap_or("").trim();
        // Label defaults to the id when omitted.
        let label = match args["label"].as_str().map(str::trim) {
            Some(l) if !l.is_empty() => l,
            _ => id,
        };
        if id.is_empty() {
            return Ok("Error: id is required (the model string sent in API requests, e.g. 'z-ai/glm-5.2').".to_string());
        }
        if !VALID_PROVIDERS.contains(&provider) {
            return Ok(format!(
                "Error: provider must be one of [{}] (got '{}').",
                VALID_PROVIDERS.join(", "),
                provider
            ));
        }
        // User models sort after builtins by default (matches the HTTP create).
        let sort_order = args["sort_order"].as_i64().map(|n| n as i32).unwrap_or(1000);

        match ModelStore::create(&self.pool, id, label, provider, sort_order).await {
            Ok(model) => {
                self.event_bus
                    .emit(BusEvent::System(SystemEvent::ModelCreated {
                        id: model.id.clone(),
                        label: model.label.clone(),
                        provider: model.provider.clone(),
                        actor: None,
                    }))
                    .await?;
                Ok(format!(
                    "[ACTION COMPLETED] Model '{}' ({}) added to the registry and enabled.",
                    model.id, model.provider
                ))
            }
            // The unique-PK violation is the common case — a clearer message than
            // the raw sqlx error.
            Err(e) => Ok(format!(
                "Error: failed to add model '{}' (it may already exist — use action 'enable' instead): {}",
                id, e
            )),
        }
    }

    async fn manage_models_set_enabled(
        &self,
        args: &serde_json::Value,
        enabled: bool,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let id = args["id"].as_str().unwrap_or("").trim();
        if id.is_empty() {
            return Ok("Error: id is required.".to_string());
        }
        match ModelStore::set_enabled(&self.pool, id, enabled).await {
            Ok(true) => {
                self.event_bus
                    .emit(BusEvent::System(SystemEvent::ModelUpdated {
                        id: id.to_string(),
                        actor: None,
                    }))
                    .await?;
                Ok(format!(
                    "[ACTION COMPLETED] Model '{}' {}.",
                    id,
                    if enabled { "enabled" } else { "disabled" }
                ))
            }
            Ok(false) => Ok(format!(
                "Error: no model '{}' in the registry. Use action 'list' to see model ids.",
                id
            )),
            Err(e) => Ok(format!("Error: failed to update model '{}': {}", id, e)),
        }
    }

    async fn manage_models_remove(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let id = args["id"].as_str().unwrap_or("").trim();
        if id.is_empty() {
            return Ok("Error: id is required.".to_string());
        }
        let existing = match ModelStore::get(&self.pool, id).await? {
            Some(m) => m,
            None => {
                return Ok(format!(
                    "Error: no model '{}' in the registry. Use action 'list' to see model ids.",
                    id
                ));
            }
        };
        // Builtins can only be disabled, not deleted — deleting one could orphan a
        // saved chat_model preference (mirrors the HTTP delete guard).
        if existing.is_builtin() {
            return Ok(format!(
                "Error: '{}' is a builtin model — it can't be removed, only disabled (use action 'disable').",
                id
            ));
        }
        match ModelStore::delete(&self.pool, id).await {
            Ok(_) => {
                self.event_bus
                    .emit(BusEvent::System(SystemEvent::ModelDeleted {
                        id: id.to_string(),
                        actor: None,
                    }))
                    .await?;
                Ok(format!("[ACTION COMPLETED] Model '{}' removed from the registry.", id))
            }
            Err(e) => Ok(format!("Error: failed to remove model '{}': {}", id, e)),
        }
    }
}
