//! Strict chat resolution for saved owner content. Ordinary routing tolerates
//! stale registry picks; accepting a saved draft must never silently replace one.

use super::SavedDraftSnapshot;
use crate::core::{ModelDefaults, PreferenceStore, ResolvedModelSelection};
use crate::llm::model_registry::{resolve_route, ProviderKind};
use sqlx::PgPool;
use uuid::Uuid;

impl SavedDraftSnapshot {
    /// Resolve a composing draft with `thread_id=None`, or an active thread
    /// with its id so the same per-thread memory as the composer applies.
    /// The submission service validates channel/destination before calling this.
    pub async fn resolve_chat_selection(
        &self,
        pool: &PgPool,
        defaults: ModelDefaults<'_>,
        thread_id: Option<Uuid>,
        is_configured: impl Fn(ProviderKind) -> bool,
    ) -> Result<ResolvedModelSelection, Box<dyn std::error::Error + Send + Sync>> {
        // Freeze the registry across default resolution and route validation.
        // A failed read must not become the ordinary router's prefix fallback.
        let registry = std::sync::Arc::new(std::sync::RwLock::new(
            defaults
                .registry
                .read()
                .map_err(|_| "draft_model_registry_unavailable")?
                .clone(),
        ));
        let selection = self.selection();
        let mut resolved = PreferenceStore::try_resolve_chat_overrides_for_thread(
            pool,
            ModelDefaults {
                registry: &registry,
                fallback_model: defaults.fallback_model,
            },
            thread_id,
            None,
            ResolvedModelSelection {
                model: selection.model.clone(),
                reasoning_effort: selection.reasoning_effort.clone(),
                provider: selection.provider.clone(),
            },
        )
        .await?;
        let model = resolved.model.as_deref().unwrap_or(defaults.fallback_model);
        let routing = registry
            .read()
            .map_err(|_| "draft_model_registry_unavailable")?
            .get(model)
            .cloned()
            .ok_or("draft_model_unsupported")?;
        if routing.successor.is_some() || routing.routes.is_empty() {
            return Err("draft_model_unsupported".into());
        }
        let chosen = resolved
            .provider
            .as_deref()
            .map(|provider| {
                let kind = ProviderKind::from_name(provider).ok_or("draft_provider_unsupported")?;
                if !routing.routes.iter().any(|route| route.provider == kind) {
                    return Err("draft_provider_unsupported");
                }
                Ok(kind)
            })
            .transpose()?;
        let route = resolve_route(&registry, model, chosen, is_configured)
            .map_err(|_| "draft_provider_unconfigured")?;
        let effort = resolved.reasoning_effort.as_deref().and_then(|effort| {
            crate::llm::reasoning::clamp_effort(effort, route.provider, &route.wire_id)
        });
        if selection.reasoning_effort.is_some() && effort != selection.reasoning_effort.as_deref() {
            return Err("draft_reasoning_effort_unsupported".into());
        }
        resolved.reasoning_effort = effort.map(str::to_owned);
        resolved.model = Some(model.to_owned());
        resolved.provider = Some(route.provider.as_str().to_owned());
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::model_registry::{empty, ModelRouting};
    use crate::test_support::{setup_test_db, teardown_test_db};
    use serde_json::json;

    #[tokio::test]
    async fn saved_chat_picks_are_honored_or_refused_before_acceptance() {
        let (pool, db) = setup_test_db().await;
        let registry = empty();
        registry.write().unwrap().insert(
            "test-model".into(),
            ModelRouting::single(ProviderKind::OpenAi, "gpt-5"),
        );
        for (selection, configured, expected) in [
            (
                json!({"model":"test-model","provider":"openai","reasoningEffort":"high"}),
                true,
                None,
            ),
            (json!({"model":"test-model"}), true, None),
            (
                json!({"model":"missing-model"}),
                true,
                Some("draft_model_unsupported"),
            ),
            (
                json!({"model":"test-model","provider":"typo"}),
                true,
                Some("draft_provider_unsupported"),
            ),
            (
                json!({"model":"test-model","provider":"vertex"}),
                true,
                Some("draft_provider_unsupported"),
            ),
            (
                json!({"model":"test-model"}),
                false,
                Some("draft_provider_unconfigured"),
            ),
            (
                json!({"model":"test-model","reasoningEffort":"typo"}),
                true,
                Some("draft_reasoning_effort_unsupported"),
            ),
        ] {
            let snapshot =
                SavedDraftSnapshot::try_from(crate::core::draft_submission::SavedDraftRow::new(
                    0,
                    (
                        0,
                        0,
                        "saved".into(),
                        json!([]),
                        Some("lucidos".into()),
                        Some(selection),
                    ),
                ))
                .unwrap();
            let result = snapshot
                .resolve_chat_selection(
                    &pool,
                    ModelDefaults {
                        registry: &registry,
                        fallback_model: "test-model",
                    },
                    None,
                    |_| configured,
                )
                .await;
            match expected {
                Some(error) => assert_eq!(result.unwrap_err().to_string(), error),
                None => {
                    let result = result.unwrap();
                    assert_eq!(result.model.as_deref(), Some("test-model"));
                    assert_eq!(result.provider.as_deref(), Some("openai"));
                    if snapshot.selection().reasoning_effort.is_some() {
                        assert_eq!(
                            result.reasoning_effort,
                            snapshot.selection().reasoning_effort
                        );
                    }
                }
            }
        }
        registry
            .write()
            .unwrap()
            .get_mut("test-model")
            .unwrap()
            .successor = Some("replacement-model".into());
        let snapshot =
            SavedDraftSnapshot::try_from(crate::core::draft_submission::SavedDraftRow::new(
                0,
                (
                    0,
                    0,
                    "saved".into(),
                    json!([]),
                    Some("lucidos".into()),
                    None,
                ),
            ))
            .unwrap();
        assert_eq!(
            snapshot
                .resolve_chat_selection(
                    &pool,
                    ModelDefaults {
                        registry: &registry,
                        fallback_model: "test-model"
                    },
                    None,
                    |_| true
                )
                .await
                .unwrap_err()
                .to_string(),
            "draft_model_unsupported"
        );
        pool.close().await;
        teardown_test_db(&db).await;
    }
}
