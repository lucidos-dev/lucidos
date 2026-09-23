//! Handler for the `manage_models` tool — the agent-facing path to the chat
//! model registry (Settings → Models). Wraps `ModelStore` and emits the same
//! `Model{Created,Updated,Deleted}` events the HTTP CRUD does, so the in-memory
//! `ModelRegistry` reloads (`spawn_models_registry_subscriber`) and the picker
//! updates without a restart. Mirrors `manage_repositories`' multi-action shape.
//!
//! Switching the *active* chat model is a preference (`chat_model`) handled by
//! `set_preference`; this tool manages which models EXIST in the picker.

use super::super::LucidosEngine;
use crate::core::models::{ModelFields, ModelStore, Route};

/// Every action the tool answers, for the refusal that lists them.
const ACTIONS: &str = "list, add, enable, disable, update, remove";

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
            "update" => self.manage_models_update(args).await,
            "remove" => self.manage_models_remove(args).await,
            "" => Ok(format!("Error: action is required (one of: {ACTIONS})")),
            other => Ok(format!(
                "Error: unknown action '{other}'. Use one of: {ACTIONS}."
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
            // Surface each route's declared context window so the agent can see
            // which are still relying on the id-shape guess. That fallback gives
            // every OpenRouter / xAI / Gemini / local id 200k whatever its real
            // window, which silently shrinks the context budget.
            let routes = m
                .routes
                .iter()
                .map(|r| {
                    let window = match r.context_window {
                        Some(w) => format!("{w} tokens"),
                        None => "window inferred from id".to_string(),
                    };
                    let preferred = match m.preferred_provider.as_deref() {
                        Some(p) if p == r.provider => " (preferred)",
                        _ => "",
                    };
                    format!(
                        "{}{} as '{}', {}",
                        r.provider,
                        preferred,
                        r.wire_id(&m.id),
                        window
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            out.push_str(&format!(
                "- {}: \"{}\" | routes: {} | {} | {}\n",
                m.id,
                m.label,
                routes,
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
        // User models sort after builtins by default (matches the HTTP create).
        // `try_from`, never `as` — a wrapping cast turns an out-of-range i64
        // into an unrelated in-range value that then passes validation and gets
        // stored (e.g. 2^32 + 5000 wraps to 5000).
        let sort_order = match args["sort_order"].as_i64().map(i32::try_from) {
            Some(Ok(n)) => n,
            Some(Err(_)) => return Ok("Error: sort_order is out of range.".to_string()),
            None => 1000,
        };
        // Omitted → infer from the id. A non-positive value is rejected rather
        // than stored: it would produce a zero context budget (trimming
        // everything) or, cast from a negative, an enormous one.
        let context_window = match args["context_window"].as_i64().map(i32::try_from) {
            Some(Ok(n)) if n > 0 => Some(n),
            None => None,
            _ => {
                return Ok(
                    "Error: context_window must be a positive number of tokens that fits in a 32-bit integer (omit it to infer from the model id)."
                        .to_string(),
                )
            }
        };

        // The same rule as `POST /api/v1/models`: `routes` wins, and `provider`
        // plus `context_window` is the single-route shorthand.
        let routes = match parse_routes(args) {
            Ok(routes) => routes,
            Err(e) => return Ok(e),
        };
        let provider = Some(provider.to_string()).filter(|p| !p.is_empty());
        let routes = match crate::api::routes_for_create(routes, provider, context_window) {
            Ok(routes) => routes,
            Err(e) => return Ok(format!("Error: {e}")),
        };

        let fields = ModelFields {
            label: label.to_string(),
            routes,
            preferred_provider: None,
            sort_order,
        };
        match ModelStore::create(&self.pool, &self.event_bus, id, &fields, None).await {
            Ok(model) => {
                Ok(format!(
                    "[ACTION COMPLETED] Model '{}' ({}) added to the registry and enabled.",
                    model.id,
                    model
                        .routes
                        .iter()
                        .map(|r| r.provider.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
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
        match ModelStore::set_enabled(&self.pool, &self.event_bus, id, enabled, None).await {
            Ok(true) => Ok(format!(
                "[ACTION COMPLETED] Model '{}' {}.",
                id,
                if enabled { "enabled" } else { "disabled" }
            )),
            Ok(false) => Ok(format!(
                "Error: no model '{}' in the registry. Use action 'list' to see model ids.",
                id
            )),
            Err(e) => Ok(format!("Error: failed to update model '{}': {}", id, e)),
        }
    }

    /// Edit a model in place, under exactly the rules `PUT /api/v1/models`
    /// applies: see [`crate::api::apply_model_update`].
    async fn manage_models_update(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let id = args["id"].as_str().unwrap_or("").trim();
        if id.is_empty() {
            return Ok("Error: id is required.".to_string());
        }
        // The args carry the PUT body's field names, so they parse as one, once
        // `routes` is settled from either of its two spellings.
        let routes = match parse_routes(args) {
            Ok(routes) => routes,
            Err(e) => return Ok(e),
        };
        let mut body = args.clone();
        if let Some(fields) = body.as_object_mut() {
            fields.insert("routes".to_string(), serde_json::json!(routes));
        }
        let edit: crate::api::UpdateModelRequest = match serde_json::from_value(body) {
            Ok(edit) => edit,
            Err(e) => return Ok(format!("Error: invalid update arguments: {e}")),
        };
        let Some(existing) = ModelStore::get(&self.pool, id).await? else {
            return Ok(format!(
                "Error: no model '{id}' in the registry. Use action 'list' to see model ids."
            ));
        };
        let (fields, enabled) = match crate::api::apply_model_update(&existing, edit) {
            Ok(applied) => applied,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        match ModelStore::update(
            &self.pool,
            &self.event_bus,
            &existing.id,
            &fields,
            enabled,
            None,
        )
        .await
        {
            Ok(_) => Ok(format!(
                "[ACTION COMPLETED] Model '{}' updated: routes {}; preferred provider {}.",
                existing.id,
                fields
                    .routes
                    .iter()
                    .map(|r| r.provider.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                fields.preferred_provider.as_deref().unwrap_or("none"),
            )),
            Err(e) => Ok(format!("Error: failed to update model '{id}': {e}")),
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
        match ModelStore::delete(&self.pool, &self.event_bus, id, None).await {
            Ok(_) => Ok(format!(
                "[ACTION COMPLETED] Model '{}' removed from the registry.",
                id
            )),
            Err(e) => Ok(format!("Error: failed to remove model '{}': {}", id, e)),
        }
    }
}

/// Read the optional `routes` argument: absent or null is `None`.
///
/// A JSON string holding the list is accepted too, since that is how the CLI
/// spells it. Anything else is refused rather than ignored: an ignored list
/// would quietly fall back to the single-route shorthand.
fn parse_routes(args: &serde_json::Value) -> Result<Option<Vec<Route>>, String> {
    let parsed = match &args["routes"] {
        serde_json::Value::Null => return Ok(None),
        serde_json::Value::String(raw) => serde_json::from_str::<Vec<Route>>(raw),
        value => serde_json::from_value::<Vec<Route>>(value.clone()),
    };
    parsed
        .map(Some)
        .map_err(|e| format!("Error: routes is malformed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::parse_routes;
    use serde_json::json;

    #[test]
    fn parse_routes_reads_absent_and_null_as_none() {
        assert_eq!(parse_routes(&json!({})), Ok(None));
        assert_eq!(parse_routes(&json!({ "routes": null })), Ok(None));
    }

    #[test]
    fn parse_routes_reads_a_list_or_its_json_string() {
        let list = json!([{ "provider": "vertex" }, { "provider": "anthropic" }]);
        let from_list = parse_routes(&json!({ "routes": list })).unwrap().unwrap();
        let from_string = parse_routes(&json!({ "routes": list.to_string() }))
            .unwrap()
            .unwrap();
        assert_eq!(from_list, from_string);
        assert_eq!(from_list.len(), 2);
    }

    /// A malformed list must be refused, never ignored in favour of `provider`.
    #[test]
    fn parse_routes_refuses_a_malformed_list() {
        let err = parse_routes(&json!({ "routes": { "provider": "vertex" } })).unwrap_err();
        assert!(err.contains("routes is malformed"), "{err}");
        let err = parse_routes(&json!({ "routes": "not json" })).unwrap_err();
        assert!(err.contains("routes is malformed"), "{err}");
    }
}
