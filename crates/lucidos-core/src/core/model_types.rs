use serde::{Deserialize, Serialize};

/// INVARIANT: `source = 'builtin'` rows are seeded by migration: never
/// deletable (deleting one could orphan a user's saved `chat_model` pref).
pub const SOURCE_BUILTIN: &str = "builtin";
pub const SOURCE_USER: &str = "user";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// INVARIANT: This backend's context window in tokens. `None` = not declared, so
    /// `llm::model_registry::context_window_from_prefix` reads THIS route's
    /// wire id. That is what makes an OpenRouter route carrying no `[1m]` land
    /// on 200k without declaring anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i32>,
}

impl Route {
    pub fn bare(provider: &str) -> Self {
        Self {
            provider: provider.to_string(),
            id: None,
            context_window: None,
        }
    }

    pub fn wire_id<'a>(&'a self, row_id: &'a str) -> &'a str {
        self.id.as_deref().unwrap_or(row_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub label: String,
    pub routes: Vec<Route>,
    /// INVARIANT: The provider last picked for this model, or `None` for never picked.
    /// Honoured when its route is configured, and refused rather than
    /// substituted when it is not.
    pub preferred_provider: Option<String>,
    pub vision: bool,
    pub default_effort: Option<String>,
    pub successor: Option<String>,
    pub sort_order: i32,
    pub source: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl Model {
    pub fn is_builtin(&self) -> bool {
        self.source == SOURCE_BUILTIN
    }

    pub fn primary_provider(&self) -> &str {
        self.routes.first().map_or("", |r| r.provider.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct ModelFields {
    pub label: String,
    pub routes: Vec<Route>,
    pub preferred_provider: Option<String>,
    pub vision: bool,
    pub sort_order: i32,
}

pub fn validate_routes(routes: &[Route]) -> Result<(), String> {
    if routes.is_empty() {
        return Err("A model needs at least one route".to_string());
    }
    let mut seen: Vec<&str> = Vec::with_capacity(routes.len());
    for route in routes {
        let p = route.provider.as_str();
        if crate::llm::ProviderKind::from_name(p).is_none() {
            return Err(crate::llm::model_registry::unknown_provider_message(p));
        }
        if seen.contains(&p) {
            return Err(format!("Provider '{p}' appears twice in routes"));
        }
        if route.id.as_deref().is_some_and(|id| id.trim().is_empty()) {
            return Err(format!(
                "The '{p}' route's id is blank (omit it to send the model's own id)"
            ));
        }
        if route.context_window.is_some_and(|w| w <= 0) {
            return Err(format!(
                "The '{p}' route's context_window must be a positive number of tokens \
                 (omit it to infer from the id)"
            ));
        }
        seen.push(p);
    }
    Ok(())
}
