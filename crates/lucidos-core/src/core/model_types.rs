//! Chat model catalogue types and route validation.

use serde::{Deserialize, Serialize};

/// `source = 'builtin'` rows are seeded by migration: never
/// deletable (deleting one could orphan a user's saved `chat_model` pref).
pub const SOURCE_BUILTIN: &str = "builtin";
/// `source = 'user'` rows are added in Settings: fully editable and deletable.
pub const SOURCE_USER: &str = "user";

/// One way a model can be served: a backend, the id to send it, and that
/// backend's context window.
///
/// A row carries an ordered list of these, which is what lets one model row be
/// served by whichever provider the workspace has credentials for. Before it,
/// `provider` named THE backend and the same model on two backends needed two
/// rows, so it appeared in the picker twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    /// Backend that serves the model on this route:
    /// "vertex" | "anthropic" | "openai" | "openrouter" | "xai" |
    /// "opencode-free" | "local" | "claude-gateway" | "chatgpt-plan".
    pub provider: String,
    /// The id sent on the wire. `None` means the row's own id, which is the
    /// common case: a first-party Claude id is byte-identical on Vertex and on
    /// the direct Anthropic API. OpenRouter is the case that needs one, since
    /// it prefixes (`anthropic/claude-opus-5-5`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// This backend's context window in tokens. `None` = not declared, so
    /// `llm::model_registry::context_window_from_prefix` reads THIS route's
    /// wire id. That is what makes an OpenRouter route carrying no `[1m]` land
    /// on 200k without declaring anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i32>,
}

impl Route {
    /// A route on `provider` sending the row's own id, with no declared window.
    pub fn bare(provider: &str) -> Self {
        Self {
            provider: provider.to_string(),
            id: None,
            context_window: None,
        }
    }

    /// The id this route puts on the wire: its own if it declares one, else the
    /// row's. Every id-shape rule reads THIS, never the row id, or a route with
    /// a different spelling would be judged by a string it never sends.
    pub fn wire_id<'a>(&'a self, row_id: &'a str) -> &'a str {
        self.id.as_deref().unwrap_or(row_id)
    }
}

/// A chat model in the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    /// Identity and primary key (e.g. "claude-fable-5", "claude-opus-5[1m]").
    /// The default wire id too, which a [`Route`] may override.
    pub id: String,
    pub label: String,
    /// Backends that can serve this model, in priority order. Never empty, and
    /// never naming one provider twice (enforced by `model_routes_valid`).
    pub routes: Vec<Route>,
    /// The provider last picked for this model, or `None` for never picked.
    /// Honoured when its route is configured, and refused rather than
    /// substituted when it is not.
    pub preferred_provider: Option<String>,
    /// The *vision flag*: whether this model reads images. Image description
    /// refuses a model without it, and its picker hides one.
    pub vision: bool,
    /// The *default effort*: the reasoning effort this model runs at when
    /// nothing stored names one. `None` sends no effort. Seeded by migration
    /// from the provider's docs, and not editable.
    pub default_effort: Option<String>,
    /// The model this one was retired to, or `None` for a current model. The
    /// router sends its successor instead, and `retire_model` (the SQL
    /// function) moved every saved setting off it. Not editable.
    pub successor: Option<String>,
    pub sort_order: i32,
    /// [`SOURCE_BUILTIN`] or [`SOURCE_USER`].
    pub source: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl Model {
    pub fn is_builtin(&self) -> bool {
        self.source == SOURCE_BUILTIN
    }

    /// The row's first route's provider, which is what serves the model when
    /// nothing has been picked. Named on the `Model{Created,Updated}` events so
    /// the audit timeline still says which backend a model landed on.
    pub fn primary_provider(&self) -> &str {
        self.routes.first().map_or("", |r| r.provider.as_str())
    }
}

/// The editable shape of a model row, shared by the create and update paths.
#[derive(Debug, Clone)]
pub struct ModelFields {
    pub label: String,
    pub routes: Vec<Route>,
    pub preferred_provider: Option<String>,
    pub vision: bool,
    pub sort_order: i32,
}

/// Check a route list against the rules the DB CHECK also enforces, and say
/// what is wrong in a sentence rather than a constraint-violation string.
///
/// An empty list makes the model unreachable with nothing saying so. A
/// provider named twice makes "the first configured route" ambiguous. A blank
/// id or a non-positive window would send nothing or budget nothing. Provider
/// names parse strictly, through the one list `ProviderKind` holds.
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
