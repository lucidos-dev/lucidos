//! In-memory model routing map, projected from the `models` table.
//!
//! `RoutingProvider` consults this to pick which backend (Vertex / direct
//! Anthropic / OpenAI) serves a given model id, and the context trimmer
//! consults it for the model's declared context window. The map is
//! hot-swappable: the engine's `spawn_models_registry_subscriber` reloads it
//! whenever a `Model*` event fires, so adding a model, re-providering it, or
//! correcting its context window in Settings takes effect without a restart.

use crate::core::ModelStore;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Which provider backend serves a model. `OpenAi`, `OpenRouter`, `XAi`,
/// `OpenCodeFree` and `Local` all speak the OpenAI Chat Completions wire format
/// but are distinct backends (different base URL / key / headers). Each maps to
/// its own provider instance in [`crate::llm::routing::RoutingProvider`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Vertex,
    Anthropic,
    OpenAi,
    /// OpenRouter (`https://openrouter.ai/api/v1`) — e.g. GLM 5.2.
    OpenRouter,
    /// xAI direct (`https://api.x.ai/v1`), the Grok family. Ids are bare here
    /// (`grok-4.6`); the same models via OpenRouter carry its `x-ai/` prefix and
    /// are separate rows on [`Self::OpenRouter`].
    XAi,
    /// OpenCode's free tier on the Zen relay (`https://opencode.ai/zen/v1`),
    /// served anonymously. The only backend with no credential at all: the relay
    /// rejects an unrecognized bearer, so the request carries no `Authorization`
    /// header. Off unless the user opts in.
    OpenCodeFree,
    /// A generic OpenAI-compatible local server (Ollama / LM Studio / vLLM /
    /// llama.cpp), base URL configurable.
    Local,
}

impl ProviderKind {
    /// Every backend, in the order `/health` reports them. One list, so a new
    /// variant cannot reach the router while missing from what `/health` says
    /// is configured, which is what the picker filters against.
    pub const ALL: [Self; 7] = [
        Self::Vertex,
        Self::Anthropic,
        Self::OpenAi,
        Self::OpenRouter,
        Self::XAi,
        Self::OpenCodeFree,
        Self::Local,
    ];

    /// Parse a provider name strictly: `None` for anything that is not one.
    ///
    /// Every CHOICE goes through this, never [`Self::parse`]: a request, a
    /// trigger pin, a remembered pick. A typo there must not become a Vertex
    /// pin, since a choice is honoured or refused, never substituted.
    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == s)
    }

    /// Every backend's name, comma separated, for an error that lists them.
    pub fn names() -> String {
        Self::ALL.map(|kind| kind.as_str()).join(", ")
    }

    /// Parse a stored route's provider. An unknown string falls back to Vertex,
    /// the historical default for every non-`gpt-` model. So a row a newer
    /// engine wrote still routes somewhere rather than erroring.
    pub fn parse(s: &str) -> Self {
        Self::from_name(s).unwrap_or(Self::Vertex)
    }

    /// The `models.provider` column string — inverse of [`Self::parse`]. Used to
    /// report configured providers over `/health` in the same vocabulary the
    /// model rows (and the frontend filter) use.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Vertex => "vertex",
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::OpenRouter => "openrouter",
            Self::XAi => "xai",
            Self::OpenCodeFree => "opencode-free",
            Self::Local => "local",
        }
    }
}

/// One way a model can be served, as the registry holds it: a backend, the id
/// that goes on the wire, and that backend's window.
///
/// The wire id is already resolved against the row id, so nothing downstream
/// has to remember the defaulting rule. Every id-shape rule reads this id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEntry {
    pub provider: ProviderKind,
    pub wire_id: String,
    /// Context window in tokens as declared on this route. `None` = not
    /// declared, so [`context_window_for`] falls back to the id-shape guess in
    /// [`context_window_from_prefix`], read from [`Self::wire_id`].
    pub context_window: Option<usize>,
}

impl RouteEntry {
    /// A route on `provider` sending `wire_id`, declaring no window.
    pub fn new(provider: ProviderKind, wire_id: impl Into<String>) -> Self {
        Self {
            provider,
            wire_id: wire_id.into(),
            context_window: None,
        }
    }
}

/// What the registry knows about one model id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRouting {
    /// Backends that can serve this model, in priority order. Never empty for a
    /// row that came from the table.
    pub routes: Vec<RouteEntry>,
    /// The provider last picked for this model, or `None` for never picked.
    pub preferred: Option<ProviderKind>,
}

impl ModelRouting {
    /// A single-route model, the shape most rows have.
    pub fn single(provider: ProviderKind, wire_id: impl Into<String>) -> Self {
        Self {
            routes: vec![RouteEntry::new(provider, wire_id)],
            preferred: None,
        }
    }

    fn route_on(&self, provider: ProviderKind) -> Option<&RouteEntry> {
        self.routes.iter().find(|r| r.provider == provider)
    }
}

/// Shared, hot-swappable model routing map. Cloned into `RoutingProvider` and
/// held by the engine; the reload subscriber swaps the inner map on `Model*`
/// events.
pub type ModelRegistry = Arc<RwLock<HashMap<String, ModelRouting>>>;

/// Build an empty registry handle (before the first DB load, and in tests).
pub fn empty() -> ModelRegistry {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Last-resort context window guessed from the model-id shape, for ids with no
/// registry row at all.
///
/// **Prefer the declared window** — call [`context_window_for`], which consults
/// the `models` registry first and only lands here on a miss.
///
/// What the rules mean, and where they fall short:
///
/// - `[1m]` → 1M is correct: the suffix is Lucidos's own marker for "request 1M
///   mode", and `build_claude_request` attaches the `context-1m-2025-08-07` beta
///   for exactly those ids.
/// - `claude-` → 200k is correct for most bare ids. A bare id sends no 1M
///   beta, so 200k is the window of the request the engine actually makes. The
///   exception is a family whose DEFAULT window is 1M (Opus 5, Opus 5.5, Fable
///   5.x): their bare builtin rows declare 1M, so they never reach this guess.
/// - `gpt-` major 5 or newer → 400k **understates** GPT-5.5, GPT-5.6 and GPT-6
///   Astra, which are all 1,050,000. The OpenAI path has no context opt-in, so
///   the full window applies to every request, and those rows declare it.
/// - There is **no rule at all** for OpenRouter / xAI / Gemini / local ids, so they
///   take the bare 200k default — kimi-k3 (1,048,576 real) was budgeted at 200k
///   and the trim loop evicted context at ~8% of the true window. That is the
///   gap the `context_window` column was added to close.
///
/// Every guess here errs low on purpose. Under-reporting only trims context
/// earlier than necessary; over-reporting makes the engine pack a prompt the
/// provider then rejects.
///
/// What still legitimately lands here: Claude Code model ids, which live in
/// `runtime/cc_menu_options.json` and never get a `models` row, plus a legacy
/// `chat_model` preference naming a model the user has since deleted.
///
/// Lives here rather than in `engine::context` because it is model-id
/// knowledge, and `llm/` must not depend on `crate::engine` (enforced by
/// `llm::validate::tests::llm_does_not_depend_on_engine`).
pub fn context_window_from_prefix(model: &str) -> usize {
    if model.contains("[1m]") {
        return 1_000_000;
    }
    if model.starts_with("claude-") {
        return 200_000;
    }
    if gpt_major_version(model).is_some_and(|major| major >= 5) {
        return 400_000;
    }
    200_000
}

/// The major version in a `gpt-` model id: 5 for `gpt-5` and `gpt-5.6-sol`, 6
/// for `gpt-6-astra`, 4 for `gpt-4o`. `None` when the id names no GPT version.
///
/// Two id-shape rules read this instead of matching the literal `gpt-5`: the
/// Responses-API split in [`crate::llm::openai`] and the window guess above.
/// Both used to say `starts_with("gpt-5")`, so `gpt-6-astra` routed to Chat
/// Completions and was budgeted at 200k. Reading the number means the next
/// family needs no edit at either site.
///
/// Each site keeps its own threshold, because they answer different questions.
pub fn gpt_major_version(model: &str) -> Option<u32> {
    let rest = model.strip_prefix("gpt-")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Normalize a stored `models.context_window` into a usable window.
///
/// A non-positive value is treated as undeclared. The API rejects those, but the
/// column is plain nullable SQL — a hand-edited or migrated row must not be able
/// to produce a zero (or, once cast, an enormous wrapped) budget.
fn declared_window(raw: Option<i32>) -> Option<usize> {
    raw.filter(|w| *w > 0).map(|w| w as usize)
}

/// Load the routing map from the `models` table — all rows, enabled or not,
/// since routing must still resolve a model that was disabled after a user saved
/// it as their `chat_model`. On a DB error, return an empty map and log; routing
/// then degrades to the prefix heuristic rather than failing every call.
pub async fn load_from_db(pool: &PgPool) -> HashMap<String, ModelRouting> {
    match ModelStore::list(pool).await {
        Ok(models) => models
            .into_iter()
            .map(|m| {
                let routes = m
                    .routes
                    .iter()
                    .map(|r| RouteEntry {
                        provider: ProviderKind::parse(&r.provider),
                        wire_id: r.wire_id(&m.id).to_string(),
                        context_window: declared_window(r.context_window),
                    })
                    .collect();
                let preferred = m
                    .preferred_provider
                    .as_deref()
                    .and_then(ProviderKind::from_name);
                (m.id, ModelRouting { routes, preferred })
            })
            .collect(),
        Err(e) => {
            crate::log!(
                "[ModelRegistry] Failed to load models table; routing falls back to prefix heuristic: {}",
                e
            );
            HashMap::new()
        }
    }
}

/// Look up a model id in the registry, if the lock is readable.
fn routing_for(registry: &ModelRegistry, model: &str) -> Option<ModelRouting> {
    registry.read().ok().and_then(|map| map.get(model).cloned())
}

/// The refusal for a provider name that names no backend.
pub fn unknown_provider_message(name: &str) -> String {
    format!(
        "Unknown provider '{name}'. Use one of: {}",
        ProviderKind::names()
    )
}

/// Which backend an unconfigured choice named, so the caller can say what to
/// set up. Returned instead of a route, never in place of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unconfigured(pub ProviderKind);

/// Resolve which backend serves `model`, and what id to send it.
///
/// **An explicit choice is honoured or refused, never substituted.** `chosen`
/// is the caller's own pick and outranks the row's stored one. If the picked
/// backend is not configured, this returns [`Unconfigured`]. The caller then
/// says how to set it up, rather than sending the turn to another vendor at
/// another price under other terms.
///
/// With no choice on either side, the first configured route wins.
///
/// With no configured route at all, the error names the row's FIRST route. That
/// keeps the message pointing at the backend the model actually wants.
///
/// With no row at all the prefix heuristic answers, as it always has. That
/// covers an unknown id, a saved preference for a deleted model, and a poisoned
/// lock.
pub fn resolve_route(
    registry: &ModelRegistry,
    model: &str,
    chosen: Option<ProviderKind>,
    is_configured: impl Fn(ProviderKind) -> bool,
) -> Result<RouteEntry, Unconfigured> {
    let Some(routing) = routing_for(registry, model) else {
        let guess = prefix_heuristic(model);
        return match is_configured(guess) {
            true => Ok(RouteEntry::new(guess, model)),
            false => Err(Unconfigured(guess)),
        };
    };
    // A pick the row cannot serve is STALE, not a refusal: the model moved off
    // that backend. It yields to the row's own preference, rather than skipping
    // it and reporting a provider this model never had.
    let pick = chosen
        .and_then(|p| routing.route_on(p))
        .or_else(|| routing.preferred.and_then(|p| routing.route_on(p)));
    if let Some(route) = pick {
        return match is_configured(route.provider) {
            true => Ok(route.clone()),
            false => Err(Unconfigured(route.provider)),
        };
    }
    match routing.routes.iter().find(|r| is_configured(r.provider)) {
        Some(route) => Ok(route.clone()),
        None => Err(Unconfigured(
            routing
                .routes
                .first()
                .map_or_else(|| prefix_heuristic(model), |r| r.provider),
        )),
    }
}

/// The route serving `model` on `provider`, if this model has one at all.
///
/// Answers "can this backend serve this model", which is a different question
/// from [`resolve_route`]'s "which backend will". The web-search chain asks it:
/// it may run on a provider the user is not chatting with, and handing that
/// provider the chat model's id would be a hard rejection.
///
/// A model with no row falls back to the prefix heuristic. So an id the table
/// has never seen answers exactly as it did before routes existed.
pub fn route_on(
    registry: &ModelRegistry,
    model: &str,
    provider: ProviderKind,
) -> Option<RouteEntry> {
    match routing_for(registry, model) {
        Some(routing) => routing.route_on(provider).cloned(),
        None => (prefix_heuristic(model) == provider).then(|| RouteEntry::new(provider, model)),
    }
}

/// The provider that would serve `model`, ignoring what is configured: the
/// row's preferred route if it names one, else its first, else the prefix
/// heuristic.
///
/// For "which backend will this turn actually reach", call [`resolve_route`]
/// with the router's configured set. This answers the weaker question, for
/// callers that hold no such set. Deriving a model's reasoning tiers for the
/// registry listing is one, and the eval harness naming a run's provider is
/// the other.
pub fn provider_kind_for(registry: &ModelRegistry, model: &str) -> ProviderKind {
    resolve_route(registry, model, None, |_| true)
        .map_or_else(|_| prefix_heuristic(model), |r| r.provider)
}

/// Resolve the context window for a model id.
///
/// The resolved route's declared window wins. Undeclared falls back to the
/// id-shape guess, read from that route's WIRE id.
///
/// Reading the route's id keeps the budget honest across backends. A `[1m]` row
/// reached through a route whose id carries no suffix sent no 1M beta. So it
/// must be budgeted at 200k, however the row itself is spelled.
///
/// A declared window is also the kimi-k3 fix. The prefix map has no rule for an
/// OpenRouter, xAI, Gemini or local id and hands them all 200k. That evicted a
/// 1,048,576-token model's context at roughly 8% of its real window.
///
/// `is_configured` decides which route answers. A caller with no view of the
/// configured set passes `|_| true`, which reads the row's own first choice.
pub fn context_window_for(
    registry: &ModelRegistry,
    model: &str,
    chosen: Option<ProviderKind>,
    is_configured: impl Fn(ProviderKind) -> bool,
) -> usize {
    match resolve_route(registry, model, chosen, is_configured) {
        Ok(route) => route
            .context_window
            .unwrap_or_else(|| context_window_from_prefix(&route.wire_id)),
        Err(_) => context_window_from_prefix(model),
    }
}

/// Last-resort provider guess from the model-string shape. Mirrors the
/// pre-registry routing rule (`gpt-` → OpenAI, else Vertex) extended so direct
/// Anthropic models (Fable 5) still route when absent from the table.
fn prefix_heuristic(model: &str) -> ProviderKind {
    if model.starts_with("gpt-") {
        ProviderKind::OpenAi
    } else if model.contains("claude-fable") {
        ProviderKind::Anthropic
    } else {
        ProviderKind::Vertex
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Single-route rows, the shape most of the registry has.
    fn registry(pairs: &[(&str, ProviderKind)]) -> ModelRegistry {
        Arc::new(RwLock::new(
            pairs
                .iter()
                .map(|(id, provider)| (id.to_string(), ModelRouting::single(*provider, *id)))
                .collect(),
        ))
    }

    /// Rows whose routes are spelled out, for the multi-route cases.
    fn registry_of(pairs: &[(&str, ModelRouting)]) -> ModelRegistry {
        Arc::new(RwLock::new(
            pairs
                .iter()
                .map(|(id, routing)| (id.to_string(), routing.clone()))
                .collect(),
        ))
    }

    fn registry_with_windows(pairs: &[(&str, Option<usize>)]) -> ModelRegistry {
        Arc::new(RwLock::new(
            pairs
                .iter()
                .map(|(id, window)| {
                    let mut routing = ModelRouting::single(ProviderKind::OpenRouter, *id);
                    routing.routes[0].context_window = *window;
                    (id.to_string(), routing)
                })
                .collect(),
        ))
    }

    /// Every provider is configured, which is what the no-configured-set
    /// callers (`provider_kind_for`, the reasoning-effort derivation) assume.
    fn all() -> impl Fn(ProviderKind) -> bool {
        |_| true
    }

    /// Only these providers are configured.
    fn only(kinds: &[ProviderKind]) -> impl Fn(ProviderKind) -> bool + '_ {
        move |kind| kinds.contains(&kind)
    }

    #[test]
    fn parse_maps_known_providers_and_defaults_to_vertex() {
        assert_eq!(ProviderKind::parse("anthropic"), ProviderKind::Anthropic);
        assert_eq!(ProviderKind::parse("openai"), ProviderKind::OpenAi);
        assert_eq!(ProviderKind::parse("openrouter"), ProviderKind::OpenRouter);
        assert_eq!(ProviderKind::parse("xai"), ProviderKind::XAi);
        assert_eq!(
            ProviderKind::parse("opencode-free"),
            ProviderKind::OpenCodeFree
        );
        assert_eq!(ProviderKind::parse("local"), ProviderKind::Local);
        assert_eq!(ProviderKind::parse("vertex"), ProviderKind::Vertex);
        assert_eq!(ProviderKind::parse("something-new"), ProviderKind::Vertex);
    }

    /// `parse` and `as_str` are inverses for every variant. The column value is
    /// the name root every layer shares. A half-added variant would route a
    /// stored row to the Vertex fallback instead.
    #[test]
    fn as_str_round_trips_through_parse() {
        for kind in [
            ProviderKind::Vertex,
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::OpenRouter,
            ProviderKind::XAi,
            ProviderKind::OpenCodeFree,
            ProviderKind::Local,
        ] {
            assert_eq!(ProviderKind::parse(kind.as_str()), kind, "{kind:?}");
        }
    }

    /// Grok reaches Lucidos two ways at once, and the registry keys on the FULL
    /// id. So the bare xAI id and OpenRouter's prefixed one are separate rows on
    /// separate backends. Adding the xAI provider must not re-route the
    /// OpenRouter row a user already has in their picker.
    #[test]
    fn bare_and_prefixed_grok_ids_route_to_different_providers() {
        let reg = registry(&[
            ("grok-4.6", ProviderKind::XAi),
            ("x-ai/grok-4.6", ProviderKind::OpenRouter),
        ]);
        assert_eq!(provider_kind_for(&reg, "grok-4.6"), ProviderKind::XAi);
        assert_eq!(
            provider_kind_for(&reg, "x-ai/grok-4.6"),
            ProviderKind::OpenRouter,
            "the OpenRouter route for Grok must survive the xAI provider"
        );
    }

    /// A bare Grok id has no prefix rule, deliberately: the four shipped ids are
    /// seeded builtins that cannot be deleted, and a user-added Grok row
    /// declares its own provider. Routing on an id shape is the mistake
    /// `llm/reasoning.rs` documents at length.
    #[test]
    fn an_unseeded_grok_id_takes_the_vertex_fallback_not_a_shape_guess() {
        assert_eq!(
            provider_kind_for(&empty(), "grok-4.6"),
            ProviderKind::Vertex
        );
    }

    #[test]
    fn registry_routes_openrouter_and_local_by_exact_hit() {
        // OpenRouter / local ids have no prefix heuristic — the registry exact
        // hit is authoritative (the seeded GLM 5.2 builtin is never deletable,
        // so its mapping is always present).
        let reg = registry(&[
            ("z-ai/glm-5.2", ProviderKind::OpenRouter),
            ("llama3.1", ProviderKind::Local),
        ]);
        assert_eq!(
            provider_kind_for(&reg, "z-ai/glm-5.2"),
            ProviderKind::OpenRouter
        );
        assert_eq!(provider_kind_for(&reg, "llama3.1"), ProviderKind::Local);
        // Absent from the table → prefix heuristic, which has no rule for these
        // shapes, so it falls back to Vertex (documented limitation).
        assert_eq!(
            provider_kind_for(&empty(), "z-ai/glm-5.2"),
            ProviderKind::Vertex
        );
    }

    /// The keyless free ids carry no shared shape, so the registry row is the
    /// only thing that routes them. A missing row must not land them on a
    /// provider that would need a credential.
    #[test]
    fn free_tier_ids_route_by_exact_hit_only() {
        let reg = registry(&[
            ("laguna-s-2.1-free", ProviderKind::OpenCodeFree),
            ("x-preview-f-free", ProviderKind::OpenCodeFree),
        ]);
        assert_eq!(
            provider_kind_for(&reg, "laguna-s-2.1-free"),
            ProviderKind::OpenCodeFree
        );
        assert_eq!(
            provider_kind_for(&reg, "x-preview-f-free"),
            ProviderKind::OpenCodeFree
        );
        // No `-free` suffix rule: an id absent from the table takes the same
        // documented Vertex fallback every unshaped id takes.
        assert_eq!(
            provider_kind_for(&empty(), "laguna-s-2.1-free"),
            ProviderKind::Vertex
        );
    }

    #[test]
    fn exact_registry_hit_wins() {
        let reg = registry(&[
            ("claude-fable-5", ProviderKind::Anthropic),
            ("claude-fable-5-1", ProviderKind::Anthropic),
            ("claude-opus-4-8", ProviderKind::Vertex),
            ("claude-sonnet-5", ProviderKind::Vertex),
            ("gpt-5.5", ProviderKind::OpenAi),
        ]);
        assert_eq!(
            provider_kind_for(&reg, "claude-fable-5"),
            ProviderKind::Anthropic
        );
        assert_eq!(
            provider_kind_for(&reg, "claude-fable-5-1"),
            ProviderKind::Anthropic
        );
        assert_eq!(
            provider_kind_for(&reg, "claude-sonnet-5"),
            ProviderKind::Vertex
        );
        assert_eq!(
            provider_kind_for(&reg, "claude-opus-4-8"),
            ProviderKind::Vertex
        );
        assert_eq!(provider_kind_for(&reg, "gpt-5.5"), ProviderKind::OpenAi);
    }

    #[test]
    fn registry_can_override_prefix_heuristic() {
        // A user could put a normally-Vertex Claude model onto the direct
        // Anthropic provider; the table wins over the heuristic.
        let reg = registry(&[("claude-opus-4-6", ProviderKind::Anthropic)]);
        assert_eq!(
            provider_kind_for(&reg, "claude-opus-4-6"),
            ProviderKind::Anthropic
        );
    }

    #[test]
    fn miss_falls_back_to_prefix_heuristic() {
        let reg = empty();
        // Legacy saved prefs / ids not in the table still route.
        assert_eq!(provider_kind_for(&reg, "gpt-5.4"), ProviderKind::OpenAi);
        assert_eq!(
            provider_kind_for(&reg, "claude-fable-5[1m]"),
            ProviderKind::Anthropic
        );
        // The heuristic matches `claude-fable`, so a new Fable generation
        // reaches the direct Anthropic provider without a new arm.
        assert_eq!(
            provider_kind_for(&reg, "claude-fable-5-1"),
            ProviderKind::Anthropic
        );
        assert_eq!(
            provider_kind_for(&reg, "claude-fable-5-1[1m]"),
            ProviderKind::Anthropic
        );
        assert_eq!(
            provider_kind_for(&reg, "claude-opus-4-7[1m]"),
            ProviderKind::Vertex
        );
        // The Opus and Sonnet rows are absent from the table here. The prefix
        // heuristic routes any non-fable `claude-*` to Vertex, matching the
        // seeded provider.
        assert_eq!(
            provider_kind_for(&reg, "claude-opus-5"),
            ProviderKind::Vertex
        );
        assert_eq!(
            provider_kind_for(&reg, "claude-opus-5-5"),
            ProviderKind::Vertex
        );
        assert_eq!(
            provider_kind_for(&reg, "claude-opus-5-5[1m]"),
            ProviderKind::Vertex
        );
        assert_eq!(
            provider_kind_for(&reg, "claude-sonnet-5"),
            ProviderKind::Vertex
        );
        assert_eq!(
            provider_kind_for(&reg, "claude-sonnet-5[1m]"),
            ProviderKind::Vertex
        );
        assert_eq!(
            provider_kind_for(&reg, "gemini-3-flash-preview"),
            ProviderKind::Vertex
        );
    }

    #[test]
    fn context_window_for_known_models() {
        assert_eq!(context_window_from_prefix("claude-opus-4-7[1m]"), 1_000_000);
        assert_eq!(context_window_from_prefix("claude-opus-4-7"), 200_000);
        assert_eq!(context_window_from_prefix("claude-sonnet-4-6"), 200_000);
        // Sonnet 5 stays 200k HERE even though a Claude Code session on it runs
        // 1M. This map answers for the request LUCIDOS makes, which sends no
        // 1M beta for a bare id. The coding-agent answer is declared on the
        // backend's own picker row (`runtime::coding_agent_context_window`).
        assert_eq!(context_window_from_prefix("claude-sonnet-5"), 200_000);
        assert_eq!(context_window_from_prefix("gpt-5"), 400_000);
        assert_eq!(context_window_from_prefix("unknown-model"), 200_000);
    }

    /// Where the prefix map falls short, pinned so the limitation stays visible
    /// and nobody "fixes" it by bolting more prefixes on — the fix is to declare
    /// the window on the row.
    ///
    /// Every guess errs low on purpose: under-reporting only trims early, while
    /// over-reporting makes the engine pack a prompt the provider rejects.
    #[test]
    fn prefix_map_under_reports_where_it_has_no_rule() {
        // No rule for OpenRouter / xAI / Gemini / local ids → bare 200k default.
        for id in [
            "moonshotai/kimi-k3",
            "z-ai/glm-5.2",
            "grok-4.20",
            "gemini-3.1-pro-preview",
            "gemini-3.5-flash",
        ] {
            assert_eq!(
                context_window_from_prefix(id),
                200_000,
                "{id} falls back to 200k — its real window must come from the registry"
            );
        }

        // Major 5 and up → 400k. But 5.5, 5.6 and GPT-6 Astra are really
        // 1,050,000, and the OpenAI path has no context opt-in to gate that
        // behind.
        //
        // Astra is the regression. The rule used to read `starts_with("gpt-5")`,
        // so a `gpt-6-` id fell to the bare 200k default. The trim loop then
        // evicted context at roughly a fifth of the true window.
        for id in ["gpt-5.5", "gpt-5.5-pro", "gpt-5.6-sol", "gpt-6-astra"] {
            assert_eq!(
                context_window_from_prefix(id),
                400_000,
                "{id} guesses 400k, and its real window must come from the registry"
            );
        }
    }

    /// The version parser both id-shape rules read. It answers the number, so a
    /// family after Astra needs no edit at either call site.
    #[test]
    fn gpt_major_version_reads_the_number_not_the_spelling() {
        for (id, expected) in [
            ("gpt-5", Some(5)),
            ("gpt-5.5-pro", Some(5)),
            ("gpt-5.6-sol", Some(5)),
            ("gpt-6-astra", Some(6)),
            ("gpt-7-whatever", Some(7)),
            ("gpt-4o", Some(4)),
            ("gpt-3.5-turbo", Some(3)),
            // Not a GPT id, or a GPT id naming no version.
            ("gpt-oss", None),
            ("gpt-", None),
            ("claude-opus-5", None),
            ("z-ai/glm-5.2", None),
        ] {
            assert_eq!(gpt_major_version(id), expected, "{id}");
        }
    }

    /// A declared window still wins over the widened guess. Astra's row carries
    /// the real 1,050,000, and 400k is what an id with no row falls back to.
    #[test]
    fn astras_declared_window_beats_the_widened_prefix_guess() {
        let reg = registry_with_windows(&[("gpt-6-astra", Some(1_050_000))]);
        assert_eq!(
            context_window_for(&reg, "gpt-6-astra", None, all()),
            1_050_000
        );
        assert_eq!(
            context_window_for(&empty(), "gpt-6-astra", None, all()),
            400_000
        );
    }

    /// The `[1m]`-vs-bare split mirrors what the engine actually requests.
    /// `build_claude_request` attaches the 1M beta only for a `[1m]` id. So a
    /// bare id of a family whose default window is 200k genuinely runs at 200k.
    /// Pinned so nobody "corrects" those to 1M and starts building prompts the
    /// API rejects. The 1M-default families declare their window on their rows.
    #[test]
    fn bare_claude_ids_are_correctly_200k_because_they_send_no_1m_beta() {
        for base in ["claude-opus-4-8", "claude-sonnet-4-6"] {
            assert_eq!(
                context_window_from_prefix(base),
                200_000,
                "{base} sends no 1M beta, so 200k is the real request window"
            );
            assert_eq!(
                context_window_from_prefix(&format!("{base}[1m]")),
                1_000_000,
                "{base}[1m] requests 1M mode, so it gets the 1M window"
            );
        }
    }

    /// A Claude row served by both Vertex and the direct Anthropic API, which
    /// is what the seed ships.
    fn dual_routed(id: &str) -> ModelRouting {
        ModelRouting {
            routes: vec![
                RouteEntry::new(ProviderKind::Vertex, id),
                RouteEntry::new(ProviderKind::Anthropic, id),
            ],
            preferred: None,
        }
    }

    /// The reported bug, at the layer that fixes it. A workspace holding only
    /// an Anthropic key reaches Opus, instead of having it silently hidden.
    ///
    /// Vertex is listed first, so a workspace holding both keeps Vertex.
    #[test]
    fn a_dual_routed_model_resolves_to_whichever_backend_is_configured() {
        let reg = registry_of(&[("claude-opus-5-5", dual_routed("claude-opus-5-5"))]);
        for (configured, expected) in [
            (vec![ProviderKind::Anthropic], ProviderKind::Anthropic),
            (vec![ProviderKind::Vertex], ProviderKind::Vertex),
            (
                vec![ProviderKind::Vertex, ProviderKind::Anthropic],
                ProviderKind::Vertex,
            ),
        ] {
            let route = resolve_route(&reg, "claude-opus-5-5", None, only(&configured))
                .expect("a configured route serves it");
            assert_eq!(route.provider, expected, "{configured:?}");
            assert_eq!(route.wire_id, "claude-opus-5-5");
        }
    }

    /// Honoured or refused, never substituted.
    ///
    /// The refusal names the PICKED backend, not the one that happens to be
    /// configured, so the message says what to set up. Silently falling through
    /// would move a turn to another vendor at another price under other terms.
    #[test]
    fn a_pick_is_honoured_or_refused_and_never_swapped() {
        let mut routing = dual_routed("claude-opus-5");
        routing.preferred = Some(ProviderKind::Anthropic);
        let reg = registry_of(&[("claude-opus-5", routing)]);

        // Configured: honoured, even though Vertex is listed first.
        let route = resolve_route(
            &reg,
            "claude-opus-5",
            None,
            only(&[ProviderKind::Anthropic]),
        )
        .expect("the picked backend serves it");
        assert_eq!(route.provider, ProviderKind::Anthropic);

        // Parked: refused, naming Anthropic rather than running on Vertex.
        assert_eq!(
            resolve_route(&reg, "claude-opus-5", None, only(&[ProviderKind::Vertex])),
            Err(Unconfigured(ProviderKind::Anthropic))
        );

        // The turn's own pick outranks the row's stored one, and is refused the
        // same way.
        assert_eq!(
            resolve_route(
                &reg,
                "claude-opus-5",
                Some(ProviderKind::Vertex),
                only(&[ProviderKind::Anthropic])
            ),
            Err(Unconfigured(ProviderKind::Vertex))
        );
    }

    /// A pick the row cannot serve is STALE, not a refusal: the model moved off
    /// that backend. Reporting a provider this model never had would send the
    /// user to configure something that still would not serve it.
    #[test]
    fn a_pick_the_row_cannot_serve_falls_through_rather_than_refusing() {
        let mut routing = ModelRouting::single(ProviderKind::Vertex, "gemini-3.5-flash");
        routing.preferred = Some(ProviderKind::Anthropic);
        let reg = registry_of(&[("gemini-3.5-flash", routing)]);
        let route = resolve_route(
            &reg,
            "gemini-3.5-flash",
            None,
            only(&[ProviderKind::Vertex]),
        )
        .expect("the row's own route serves it");
        assert_eq!(route.provider, ProviderKind::Vertex);
    }

    /// A stale turn pick yields to the row's own preference, rather than
    /// skipping it for the first configured route.
    #[test]
    fn a_stale_turn_pick_yields_to_the_rows_preference() {
        let mut routing = dual_routed("claude-sonnet-5");
        routing.preferred = Some(ProviderKind::Anthropic);
        let reg = registry_of(&[("claude-sonnet-5", routing)]);
        let route = resolve_route(
            &reg,
            "claude-sonnet-5",
            Some(ProviderKind::XAi),
            only(&[ProviderKind::Vertex, ProviderKind::Anthropic]),
        )
        .expect("the preferred route serves it");
        assert_eq!(route.provider, ProviderKind::Anthropic);
    }

    /// A choice is parsed strictly, so a typo is no choice at all rather than
    /// a Vertex pin. A stored route still falls back, so it keeps routing.
    #[test]
    fn a_choice_parses_strictly_and_a_stored_route_leniently() {
        assert_eq!(
            ProviderKind::from_name("anthropic"),
            Some(ProviderKind::Anthropic)
        );
        assert_eq!(ProviderKind::from_name("Anthropic"), None);
        assert_eq!(ProviderKind::from_name(""), None);
        assert_eq!(ProviderKind::parse("Anthropic"), ProviderKind::Vertex);
    }

    /// With nothing configured the error names the row's FIRST route, which is
    /// the backend the model actually wants.
    #[test]
    fn no_configured_route_names_the_rows_own_first_choice() {
        let reg = registry_of(&[("claude-opus-5-5", dual_routed("claude-opus-5-5"))]);
        assert_eq!(
            resolve_route(&reg, "claude-opus-5-5", None, only(&[])),
            Err(Unconfigured(ProviderKind::Vertex))
        );
    }

    /// The budget reads the ROUTE's wire id, not the row's.
    ///
    /// An OpenRouter route on a `[1m]` row sends an id carrying no suffix, so
    /// it requested no 1M beta and must be budgeted at 200k. Budgeting the row
    /// would have the packer build a prompt that backend rejects.
    #[test]
    fn the_window_follows_the_route_not_the_row() {
        let reg = registry_of(&[(
            "claude-opus-5-5[1m]",
            ModelRouting {
                routes: vec![
                    RouteEntry {
                        provider: ProviderKind::Vertex,
                        wire_id: "claude-opus-5-5[1m]".to_string(),
                        context_window: Some(1_000_000),
                    },
                    RouteEntry::new(ProviderKind::OpenRouter, "anthropic/claude-opus-5-5"),
                ],
                preferred: None,
            },
        )]);
        let id = "claude-opus-5-5[1m]";
        assert_eq!(
            context_window_for(&reg, id, None, only(&[ProviderKind::Vertex])),
            1_000_000
        );
        assert_eq!(
            context_window_for(&reg, id, None, only(&[ProviderKind::OpenRouter])),
            200_000,
            "the undeclared route falls back on its OWN id, which has no [1m]"
        );
    }

    /// `route_on` asks whether a backend CAN serve a model, which is the
    /// web-search chain's question. A row with no route there answers `None`,
    /// and a row with no entry at all falls back to the prefix heuristic.
    #[test]
    fn route_on_answers_whether_a_backend_can_serve_the_model() {
        let reg = registry_of(&[("claude-opus-5-5", dual_routed("claude-opus-5-5"))]);
        assert_eq!(
            route_on(&reg, "claude-opus-5-5", ProviderKind::Anthropic).map(|r| r.wire_id),
            Some("claude-opus-5-5".to_string())
        );
        assert_eq!(
            route_on(&reg, "claude-opus-5-5", ProviderKind::OpenAi),
            None
        );
        // No row: the heuristic answers, exactly as it did before routes.
        assert_eq!(
            route_on(&empty(), "gpt-5.5", ProviderKind::OpenAi).map(|r| r.wire_id),
            Some("gpt-5.5".to_string())
        );
        assert_eq!(route_on(&empty(), "gpt-5.5", ProviderKind::Vertex), None);
    }

    /// The whole point of the `context_window` column: a declared window wins,
    /// so kimi-k3 stops being budgeted as a 200k model.
    #[test]
    fn declared_context_window_wins_over_the_prefix_fallback() {
        let reg = registry_with_windows(&[("moonshotai/kimi-k3", Some(1_048_576))]);
        assert_eq!(
            context_window_for(&reg, "moonshotai/kimi-k3", None, all()),
            1_048_576
        );
    }

    /// An undeclared window, an unknown id, and an empty registry all fall
    /// through to the id-shape guess with exactly today's results — the
    /// back-compat half of the change.
    #[test]
    fn undeclared_window_falls_back_to_the_prefix_map() {
        let reg = registry_with_windows(&[("moonshotai/kimi-k3", None)]);
        // Declared-as-None is the same as absent.
        assert_eq!(
            context_window_for(&reg, "moonshotai/kimi-k3", None, all()),
            200_000
        );
        // Not in the table at all.
        assert_eq!(
            context_window_for(&reg, "claude-opus-4-7[1m]", None, all()),
            1_000_000
        );
        assert_eq!(context_window_for(&reg, "gpt-5.5", None, all()), 400_000);
        // Empty registry — every id takes the prefix map.
        let none = empty();
        assert_eq!(
            context_window_for(&none, "claude-opus-4-7", None, all()),
            200_000
        );
        assert_eq!(
            context_window_for(&none, "claude-opus-4-7[1m]", None, all()),
            1_000_000
        );
        assert_eq!(
            context_window_for(&none, "unknown-model", None, all()),
            200_000
        );
    }

    /// A declared window can also be SMALLER than the id-shape guess — a
    /// `claude-`-prefixed proxy or fine-tune served with a 32k window must be
    /// able to say so, or the budget over-promises and the request 400s.
    #[test]
    fn declared_window_can_shrink_as_well_as_grow() {
        let reg = registry_with_windows(&[("claude-opus-4-7", Some(32_000))]);
        assert_eq!(
            context_window_for(&reg, "claude-opus-4-7", None, all()),
            32_000
        );
    }

    /// A hand-edited zero / negative row must not reach the map — otherwise it
    /// would produce a zero budget (trimming everything) or, cast from a
    /// negative i32, an enormous one. `declared_window` drops them so the
    /// prefix map takes over.
    #[test]
    fn non_positive_declared_window_is_treated_as_undeclared() {
        assert_eq!(declared_window(Some(0)), None);
        assert_eq!(declared_window(Some(-1)), None);
        assert_eq!(declared_window(None), None);
        assert_eq!(declared_window(Some(1_048_576)), Some(1_048_576));
    }
}
