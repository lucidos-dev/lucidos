use crate::core::model_types::Model;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub use super::model_registry_db::{load_from_db, reload};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    Vertex,
    Anthropic,
    OpenAi,
    OpenRouter,
    XAi,
    /// INVARIANT: OpenCode's free tier on the Zen relay (`https://opencode.ai/zen/v1`),
    /// served anonymously. The only backend with no credential at all: the relay
    /// rejects an unrecognized bearer, so the request carries no `Authorization`
    /// header. Off unless the user opts in.
    OpenCodeFree,
    Local,
    /// INVARIANT: A Claude gateway: the Anthropic Messages API at an organisation's own
    /// origin, behind its device-code sign-in (`llm::claude_gateway`). Its own
    /// kind, apart from [`Self::Anthropic`], so each credential stays on its
    /// own host.
    ClaudeGateway,
    /// INVARIANT: The user's ChatGPT plan, through Sign in with ChatGPT
    /// (`llm::chatgpt_plan`). The OpenAI Responses API with a plan token in
    /// place of an API key. Its own kind, so a plan token never rides the
    /// platform key's route, and a platform key never rides this one.
    ChatGptPlan,
}

impl ProviderKind {
    pub const ALL: [Self; 9] = [
        Self::Vertex,
        Self::Anthropic,
        Self::OpenAi,
        Self::OpenRouter,
        Self::XAi,
        Self::OpenCodeFree,
        Self::Local,
        Self::ClaudeGateway,
        Self::ChatGptPlan,
    ];

    /// INVARIANT: Parse a provider name strictly: `None` for anything that is not one.
    ///
    /// Every CHOICE goes through this, never [`Self::parse`]: a request, a
    /// trigger pin, a remembered pick. A typo there must not become a Vertex
    /// pin, since a choice is honoured or refused, never substituted.
    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == s)
    }

    pub fn names() -> String {
        Self::ALL.map(|kind| kind.as_str()).join(", ")
    }

    pub fn parse(s: &str) -> Self {
        Self::from_name(s).unwrap_or(Self::Vertex)
    }

    /// INVARIANT: The `models.provider` column string — inverse of [`Self::parse`]. Used to
    /// report configured providers over `/health` in the same vocabulary the
    /// model rows (and the frontend filter) use.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Vertex => "vertex",
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::OpenRouter => "openrouter",
            Self::XAi => "xai",
            Self::OpenCodeFree => "opencode-free",
            Self::Local => "local",
            Self::ClaudeGateway => "claude-gateway",
            Self::ChatGptPlan => "chatgpt-plan",
        }
    }

    pub const fn label(&self) -> &'static str {
        match self {
            Self::Vertex => "Vertex",
            Self::Anthropic => "Anthropic",
            Self::OpenAi => "OpenAI",
            Self::OpenRouter => "OpenRouter",
            Self::XAi => "xAI",
            Self::OpenCodeFree => "OpenCode",
            Self::Local => "Local server",
            Self::ClaudeGateway => "Claude gateway",
            Self::ChatGptPlan => "ChatGPT",
        }
    }

    pub const fn is_free(self) -> bool {
        matches!(self, Self::OpenCodeFree)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEntry {
    pub provider: ProviderKind,
    pub wire_id: String,
    pub context_window: Option<usize>,
}

impl RouteEntry {
    pub fn new(provider: ProviderKind, wire_id: impl Into<String>) -> Self {
        Self {
            provider,
            wire_id: wire_id.into(),
            context_window: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRouting {
    /// INVARIANT: Backends that can serve this model, in priority order. Never empty for a
    /// row that came from the table.
    pub routes: Vec<RouteEntry>,
    pub preferred: Option<ProviderKind>,
    pub vision: bool,
    pub default_effort: Option<&'static str>,
    pub successor: Option<String>,
}

impl ModelRouting {
    pub fn single(provider: ProviderKind, wire_id: impl Into<String>) -> Self {
        Self {
            routes: vec![RouteEntry::new(provider, wire_id)],
            preferred: None,
            vision: false,
            default_effort: None,
            successor: None,
        }
    }

    fn route_on(&self, provider: ProviderKind) -> Option<&RouteEntry> {
        self.routes.iter().find(|r| r.provider == provider)
    }
}

pub type ModelRegistry = Arc<RwLock<HashMap<String, ModelRouting>>>;

pub fn empty() -> ModelRegistry {
    Arc::new(RwLock::new(HashMap::new()))
}

/// INVARIANT: Direct non-streaming Gemini 4 output ceiling. Shared with input packing
/// because these models use one window for input, thinking and answer tokens.
/// Google Cloud: /models/start/get-started-with-gemini-4.
pub(crate) fn unified_output_limit(model: &str) -> Option<usize> {
    match model {
        "gemini-4-neon" => Some(65_536),
        "gemini-4-argon" | "gemini-4-carbon" => Some(32_768),
        _ => None,
    }
}

/// INVARIANT: Last-resort context window guessed from the model-id shape, for ids with no
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
/// What still legitimately lands here: Claude Code model ids, which come from
/// its own picker and never get a `models` row, plus a legacy
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

pub fn gpt_major_version(model: &str) -> Option<u32> {
    gpt_version(model).map(|(major, _)| major)
}

pub fn gpt_version(model: &str) -> Option<(u32, u32)> {
    let rest = model.strip_prefix("gpt-")?;
    let number = |s: &str| -> String { s.chars().take_while(char::is_ascii_digit).collect() };
    let major = number(rest);
    let minor = rest[major.len()..]
        .strip_prefix('.')
        .map(number)
        .filter(|m| !m.is_empty());
    Some((
        major.parse().ok()?,
        minor.map_or(Some(0), |m| m.parse().ok())?,
    ))
}

fn declared_window(raw: Option<i32>) -> Option<usize> {
    raw.filter(|w| *w > 0).map(|w| w as usize)
}

pub fn routes_from_models(models: Vec<Model>) -> HashMap<String, ModelRouting> {
    models
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
            let routing = ModelRouting {
                routes,
                preferred,
                vision: m.vision,
                default_effort: m
                    .default_effort
                    .as_deref()
                    .and_then(crate::llm::reasoning::tier),
                successor: m.successor,
            };
            (m.id, routing)
        })
        .collect()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GenerationRule {
    #[default]
    AnyGeneration,
    NamedOrNewer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Generation {
    Named,
    Successor,
    Predecessor,
    Held { successor: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGeneration {
    pub model: String,
    pub generation: Generation,
}

pub fn resolve_generation(
    registry: &ModelRegistry,
    model: &str,
    rule: GenerationRule,
    reachable: impl Fn(&str) -> bool,
) -> ResolvedGeneration {
    let chain = successor_chain(registry, model);
    let newest = chain.last().cloned().unwrap_or_else(|| model.to_string());
    let resolved = |model: &str, generation| ResolvedGeneration {
        model: model.to_string(),
        generation,
    };
    if let Some(found) = chain.iter().rev().find(|m| reachable(m)) {
        return match found == model {
            true if chain.len() > 1 => resolved(model, Generation::Held { successor: newest }),
            true => resolved(model, Generation::Named),
            false => resolved(found, Generation::Successor),
        };
    }
    if rule == GenerationRule::AnyGeneration {
        if let Some(found) = predecessors(registry, &chain).find(|m| reachable(m)) {
            return resolved(&found, Generation::Predecessor);
        }
    }
    match newest == model {
        true => resolved(model, Generation::Named),
        false => resolved(&newest, Generation::Successor),
    }
}

fn successor_chain(registry: &ModelRegistry, model: &str) -> Vec<String> {
    let mut chain = vec![model.to_string()];
    while let Some(next) = chain
        .last()
        .and_then(|current| routing_for(registry, current))
        .and_then(|routing| routing.successor)
        .filter(|next| !chain.contains(next))
    {
        chain.push(next);
    }
    chain
}

/// INVARIANT: The models retired into `chain`'s first model, nearest generation first.
/// Within one generation, ids run in descending order, so the order is fixed.
/// A model already met is skipped, so a cycle ends.
fn predecessors(registry: &ModelRegistry, chain: &[String]) -> impl Iterator<Item = String> {
    let retired: Vec<(String, String)> = registry
        .read()
        .map(|map| {
            map.iter()
                .filter_map(|(id, routing)| Some((id.clone(), routing.successor.clone()?)))
                .collect()
        })
        .unwrap_or_default();
    let mut seen: Vec<String> = chain.to_vec();
    let mut generation: Vec<String> = chain.first().cloned().into_iter().collect();
    let mut found = Vec::new();
    while !generation.is_empty() {
        let mut older: Vec<String> = retired
            .iter()
            .filter(|(id, successor)| generation.contains(successor) && !seen.contains(id))
            .map(|(id, _)| id.clone())
            .collect();
        older.sort_unstable_by(|a, b| b.cmp(a));
        older.dedup();
        seen.extend(older.iter().cloned());
        found.extend(older.iter().cloned());
        generation = older;
    }
    found.into_iter()
}

pub fn reaches(
    registry: &ModelRegistry,
    model: &str,
    chosen: Option<ProviderKind>,
    is_configured: impl Fn(ProviderKind) -> bool,
    refused: impl Fn(ProviderKind, &str) -> bool,
) -> bool {
    resolve_route(registry, model, chosen, is_configured)
        .is_ok_and(|route| !refused(route.provider, model))
}

pub fn routing_for(registry: &ModelRegistry, model: &str) -> Option<ModelRouting> {
    registry.read().ok().and_then(|map| map.get(model).cloned())
}

pub fn vision_flag(registry: &ModelRegistry, model: &str) -> Option<bool> {
    routing_for(registry, model).map(|routing| routing.vision)
}

pub fn reads_images(registry: &ModelRegistry, model: &str) -> bool {
    vision_flag(registry, model) == Some(true)
}

pub fn default_effort(registry: &ModelRegistry, model: &str) -> Option<&'static str> {
    routing_for(registry, model).and_then(|routing| routing.default_effort)
}

pub fn unknown_provider_message(name: &str) -> String {
    format!(
        "Unknown provider '{name}'. Use one of: {}",
        ProviderKind::names()
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unconfigured(pub ProviderKind);

/// INVARIANT: Resolve which backend serves `model`, and what id to send it.
///
/// **An explicit choice is honoured or refused, never substituted.** `chosen`
/// is the caller's own pick and outranks the row's stored one. If the picked
/// backend is not configured, this returns [`Unconfigured`]. The caller then
/// says how to set it up, rather than sending the turn to another vendor at
/// another price under other terms.
///
/// With no choice on either side, the first configured route wins.
///
/// With no configured route at all, the error names the row's first route that
/// is not the ChatGPT plan. That keeps the message pointing at the backend the
/// model actually wants. The plan route leads only so a signed-in plan wins.
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
    // INVARIANT: A pick the row cannot serve is STALE, not a refusal: the model moved off
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
                .iter()
                .find(|r| r.provider != ProviderKind::ChatGptPlan)
                .or(routing.routes.first())
                .map_or_else(|| prefix_heuristic(model), |r| r.provider),
        )),
    }
}

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

pub fn provider_kind_for(registry: &ModelRegistry, model: &str) -> ProviderKind {
    resolve_route(registry, model, None, |_| true)
        .map_or_else(|_| prefix_heuristic(model), |r| r.provider)
}

/// INVARIANT: Resolve the context window for a model id.
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

    fn registry(pairs: &[(&str, ProviderKind)]) -> ModelRegistry {
        Arc::new(RwLock::new(
            pairs
                .iter()
                .map(|(id, provider)| (id.to_string(), ModelRouting::single(*provider, *id)))
                .collect(),
        ))
    }

    fn registry_of(pairs: &[(&str, ModelRouting)]) -> ModelRegistry {
        Arc::new(RwLock::new(
            pairs
                .iter()
                .map(|(id, routing)| (id.to_string(), routing.clone()))
                .collect(),
        ))
    }

    fn retired_to(id: &str, successor: &str) -> ModelRouting {
        ModelRouting {
            successor: Some(successor.to_string()),
            ..ModelRouting::single(ProviderKind::Vertex, id)
        }
    }

    fn all_but<'a>(refused: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |model| !refused.contains(&model)
    }

    fn sent(reg: &ModelRegistry, model: &str, rule: GenerationRule, refused: &[&str]) -> String {
        resolve_generation(reg, model, rule, all_but(refused)).model
    }

    fn haiku() -> ModelRegistry {
        registry_of(&[
            (
                "claude-haiku-4-5",
                retired_to("claude-haiku-4-5", "claude-haiku-5-5"),
            ),
            (
                "claude-haiku-5-5",
                ModelRouting::single(ProviderKind::Vertex, "claude-haiku-5-5"),
            ),
        ])
    }

    #[test]
    fn a_retired_model_is_sent_as_the_end_of_its_successor_chain() {
        let reg = registry_of(&[
            ("old", retired_to("old", "middle")),
            ("middle", retired_to("middle", "new")),
            ("new", ModelRouting::single(ProviderKind::Vertex, "new")),
        ]);
        let any = GenerationRule::AnyGeneration;
        assert_eq!(sent(&reg, "old", any, &[]), "new");
        assert_eq!(sent(&reg, "middle", any, &[]), "new");
        assert_eq!(sent(&reg, "new", any, &[]), "new");
        assert_eq!(sent(&reg, "unknown", any, &[]), "unknown");
        assert_eq!(
            resolve_generation(&reg, "old", any, all_but(&[])).generation,
            Generation::Successor
        );
        assert_eq!(
            resolve_generation(&reg, "new", any, all_but(&[])).generation,
            Generation::Named
        );
    }

    #[test]
    fn a_retired_model_is_held_while_its_successor_is_refused() {
        let resolved = resolve_generation(
            &haiku(),
            "claude-haiku-4-5",
            GenerationRule::AnyGeneration,
            all_but(&["claude-haiku-5-5"]),
        );
        assert_eq!(
            resolved,
            ResolvedGeneration {
                model: "claude-haiku-4-5".to_string(),
                generation: Generation::Held {
                    successor: "claude-haiku-5-5".to_string()
                },
            }
        );
    }

    #[test]
    fn only_a_stored_pick_steps_back_to_a_predecessor() {
        let reg = haiku();
        let refused = ["claude-haiku-5-5"];
        let resolved = resolve_generation(
            &reg,
            "claude-haiku-5-5",
            GenerationRule::AnyGeneration,
            all_but(&refused),
        );
        assert_eq!(resolved.model, "claude-haiku-4-5");
        assert_eq!(resolved.generation, Generation::Predecessor);
        let default = resolve_generation(
            &reg,
            "claude-haiku-5-5",
            GenerationRule::NamedOrNewer,
            all_but(&refused),
        );
        assert_eq!(default.model, "claude-haiku-5-5");
        assert_eq!(default.generation, Generation::Named);
    }

    #[test]
    fn a_multi_hop_chain_picks_the_newest_reachable_generation() {
        let reg = registry_of(&[
            ("a", retired_to("a", "b")),
            ("b", retired_to("b", "c")),
            ("c", ModelRouting::single(ProviderKind::Vertex, "c")),
        ]);
        let any = GenerationRule::AnyGeneration;
        assert_eq!(sent(&reg, "a", any, &["c"]), "b");
        assert_eq!(sent(&reg, "a", any, &["c", "b"]), "a");
        assert_eq!(sent(&reg, "c", any, &["c"]), "b");
        assert_eq!(sent(&reg, "c", any, &["c", "b"]), "a");
        assert_eq!(sent(&reg, "a", any, &["a", "b", "c"]), "c");
        assert_eq!(sent(&reg, "b", any, &["a", "b", "c"]), "c");
    }

    #[test]
    fn predecessors_come_nearest_first_in_a_fixed_order() {
        let reg = registry_of(&[
            ("x-1", retired_to("x-1", "x-2")),
            ("y-1", retired_to("y-1", "x-2")),
            ("x-2", retired_to("x-2", "x-3")),
            ("x-3", ModelRouting::single(ProviderKind::Vertex, "x-3")),
        ]);
        let any = GenerationRule::AnyGeneration;
        assert_eq!(sent(&reg, "x-3", any, &["x-3"]), "x-2");
        assert_eq!(sent(&reg, "x-3", any, &["x-3", "x-2"]), "y-1");
        assert_eq!(sent(&reg, "x-3", any, &["x-3", "x-2", "y-1"]), "x-1");
    }

    #[test]
    fn a_successor_cycle_ends_on_the_last_new_model() {
        let reg = registry_of(&[("a", retired_to("a", "b")), ("b", retired_to("b", "a"))]);
        let any = GenerationRule::AnyGeneration;
        assert_eq!(sent(&reg, "a", any, &[]), "b");
        assert_eq!(sent(&reg, "b", any, &[]), "a");
        assert_eq!(sent(&reg, "a", any, &["b"]), "a");
        assert_eq!(sent(&reg, "a", any, &["a", "b"]), "b");
    }

    #[test]
    fn reach_reads_the_resolved_route_and_its_refusals() {
        let reg = registry_of(&[("claude-opus-5-5", dual_routed("claude-opus-5-5"))]);
        let vertex = only(&[ProviderKind::Vertex]);
        let none = |_: ProviderKind, _: &str| false;
        let vertex_refused = |p: ProviderKind, _: &str| p == ProviderKind::Vertex;
        assert!(reaches(&reg, "claude-opus-5-5", None, &vertex, none));
        assert!(!reaches(
            &reg,
            "claude-opus-5-5",
            None,
            &vertex,
            vertex_refused
        ));
        assert!(reaches(
            &reg,
            "claude-opus-5-5",
            None,
            only(&[ProviderKind::Anthropic]),
            vertex_refused
        ));
        assert!(!reaches(&reg, "claude-opus-5-5", None, only(&[]), none));
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

    fn all() -> impl Fn(ProviderKind) -> bool {
        |_| true
    }

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
        assert_eq!(
            ProviderKind::parse("claude-gateway"),
            ProviderKind::ClaudeGateway
        );
        assert_eq!(ProviderKind::parse("vertex"), ProviderKind::Vertex);
        assert_eq!(ProviderKind::parse("something-new"), ProviderKind::Vertex);
    }

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
            ProviderKind::ClaudeGateway,
        ] {
            assert_eq!(ProviderKind::parse(kind.as_str()), kind, "{kind:?}");
        }
    }

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

    #[test]
    fn an_unseeded_grok_id_takes_the_vertex_fallback_not_a_shape_guess() {
        assert_eq!(
            provider_kind_for(&empty(), "grok-4.6"),
            ProviderKind::Vertex
        );
    }

    #[test]
    fn registry_routes_openrouter_and_local_by_exact_hit() {
        let reg = registry(&[
            ("z-ai/glm-5.2", ProviderKind::OpenRouter),
            ("llama3.1", ProviderKind::Local),
        ]);
        assert_eq!(
            provider_kind_for(&reg, "z-ai/glm-5.2"),
            ProviderKind::OpenRouter
        );
        assert_eq!(provider_kind_for(&reg, "llama3.1"), ProviderKind::Local);
        assert_eq!(
            provider_kind_for(&empty(), "z-ai/glm-5.2"),
            ProviderKind::Vertex
        );
    }

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
        let reg = registry(&[("claude-opus-4-6", ProviderKind::Anthropic)]);
        assert_eq!(
            provider_kind_for(&reg, "claude-opus-4-6"),
            ProviderKind::Anthropic
        );
    }

    #[test]
    fn miss_falls_back_to_prefix_heuristic() {
        let reg = empty();
        assert_eq!(provider_kind_for(&reg, "gpt-5.4"), ProviderKind::OpenAi);
        assert_eq!(
            provider_kind_for(&reg, "claude-fable-5[1m]"),
            ProviderKind::Anthropic
        );
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
            provider_kind_for(&reg, "claude-sonnet-5-5"),
            ProviderKind::Vertex
        );
        assert_eq!(
            provider_kind_for(&reg, "claude-sonnet-5-5[1m]"),
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
        assert_eq!(context_window_from_prefix("claude-sonnet-5"), 200_000);
        assert_eq!(context_window_from_prefix("gpt-5"), 400_000);
        assert_eq!(context_window_from_prefix("unknown-model"), 200_000);
    }

    #[test]
    fn prefix_map_under_reports_where_it_has_no_rule() {
        for id in [
            "moonshotai/kimi-k3",
            "z-ai/glm-5.2",
            "grok-4.20",
            "gemini-3.1-pro-preview",
            "gemini-3.8-flash",
        ] {
            assert_eq!(
                context_window_from_prefix(id),
                200_000,
                "{id} falls back to 200k — its real window must come from the registry"
            );
        }

        for id in ["gpt-5.5", "gpt-5.5-pro", "gpt-5.6-sol", "gpt-6-astra"] {
            assert_eq!(
                context_window_from_prefix(id),
                400_000,
                "{id} guesses 400k, and its real window must come from the registry"
            );
        }
    }

    #[test]
    fn gpt_version_reads_major_and_minor() {
        for (id, expected) in [
            ("gpt-5.6-sol", Some((5, 6))),
            ("gpt-5.3-codex", Some((5, 3))),
            ("gpt-6-astra", Some((6, 0))),
            ("gpt-4o", Some((4, 0))),
            ("gpt-", None),
            ("claude-opus-5", None),
        ] {
            assert_eq!(gpt_version(id), expected, "{id}");
        }
    }

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
            ("gpt-oss", None),
            ("gpt-", None),
            ("claude-opus-5", None),
            ("z-ai/glm-5.2", None),
        ] {
            assert_eq!(gpt_major_version(id), expected, "{id}");
        }
    }

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

    fn dual_routed(id: &str) -> ModelRouting {
        ModelRouting {
            routes: vec![
                RouteEntry::new(ProviderKind::Vertex, id),
                RouteEntry::new(ProviderKind::Anthropic, id),
            ],
            preferred: None,
            vision: false,
            default_effort: None,
            successor: None,
        }
    }

    #[test]
    fn only_a_flagged_row_reads_images() {
        let registry = empty();
        registry.write().unwrap().insert(
            "flagged".to_string(),
            ModelRouting {
                vision: true,
                ..dual_routed("flagged")
            },
        );
        registry
            .write()
            .unwrap()
            .insert("unflagged".to_string(), dual_routed("unflagged"));
        assert!(reads_images(&registry, "flagged"));
        assert!(!reads_images(&registry, "unflagged"));
        assert!(!reads_images(&registry, "claude-opus-5-5"));
    }

    #[test]
    fn the_vision_flag_is_unknown_without_a_row() {
        let registry = empty();
        registry.write().unwrap().insert(
            "flagged".to_string(),
            ModelRouting {
                vision: true,
                ..dual_routed("flagged")
            },
        );
        registry
            .write()
            .unwrap()
            .insert("unflagged".to_string(), dual_routed("unflagged"));
        assert_eq!(vision_flag(&registry, "flagged"), Some(true));
        assert_eq!(vision_flag(&registry, "unflagged"), Some(false));
        assert_eq!(vision_flag(&registry, "claude-opus-5-5"), None);
    }

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

    #[test]
    fn a_pick_is_honoured_or_refused_and_never_swapped() {
        let mut routing = dual_routed("claude-opus-5");
        routing.preferred = Some(ProviderKind::Anthropic);
        let reg = registry_of(&[("claude-opus-5", routing)]);

        let route = resolve_route(
            &reg,
            "claude-opus-5",
            None,
            only(&[ProviderKind::Anthropic]),
        )
        .expect("the picked backend serves it");
        assert_eq!(route.provider, ProviderKind::Anthropic);

        assert_eq!(
            resolve_route(&reg, "claude-opus-5", None, only(&[ProviderKind::Vertex])),
            Err(Unconfigured(ProviderKind::Anthropic))
        );

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

    #[test]
    fn an_unconfigured_gpt_model_names_the_api_route_not_the_plan() {
        let routing = ModelRouting {
            routes: vec![
                RouteEntry::new(ProviderKind::ChatGptPlan, "gpt-x"),
                RouteEntry::new(ProviderKind::OpenAi, "gpt-x"),
            ],
            ..dual_routed("gpt-x")
        };
        let reg = registry_of(&[("gpt-x", routing)]);
        assert_eq!(
            resolve_route(&reg, "gpt-x", None, only(&[])),
            Err(Unconfigured(ProviderKind::OpenAi))
        );
        let route = resolve_route(
            &reg,
            "gpt-x",
            None,
            only(&[ProviderKind::ChatGptPlan, ProviderKind::OpenAi]),
        )
        .expect("the plan serves it");
        assert_eq!(route.provider, ProviderKind::ChatGptPlan);
    }

    #[test]
    fn a_pick_the_row_cannot_serve_falls_through_rather_than_refusing() {
        let mut routing = ModelRouting::single(ProviderKind::Vertex, "gemini-3.8-flash");
        routing.preferred = Some(ProviderKind::Anthropic);
        let reg = registry_of(&[("gemini-3.8-flash", routing)]);
        let route = resolve_route(
            &reg,
            "gemini-3.8-flash",
            None,
            only(&[ProviderKind::Vertex]),
        )
        .expect("the row's own route serves it");
        assert_eq!(route.provider, ProviderKind::Vertex);
    }

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

    #[test]
    fn no_configured_route_names_the_rows_own_first_choice() {
        let reg = registry_of(&[("claude-opus-5-5", dual_routed("claude-opus-5-5"))]);
        assert_eq!(
            resolve_route(&reg, "claude-opus-5-5", None, only(&[])),
            Err(Unconfigured(ProviderKind::Vertex))
        );
    }

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
                vision: false,
                default_effort: None,
                successor: None,
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
        assert_eq!(
            route_on(&empty(), "gpt-5.5", ProviderKind::OpenAi).map(|r| r.wire_id),
            Some("gpt-5.5".to_string())
        );
        assert_eq!(route_on(&empty(), "gpt-5.5", ProviderKind::Vertex), None);
    }

    #[test]
    fn declared_context_window_wins_over_the_prefix_fallback() {
        let reg = registry_with_windows(&[("moonshotai/kimi-k3", Some(1_048_576))]);
        assert_eq!(
            context_window_for(&reg, "moonshotai/kimi-k3", None, all()),
            1_048_576
        );
    }

    #[test]
    fn undeclared_window_falls_back_to_the_prefix_map() {
        let reg = registry_with_windows(&[("moonshotai/kimi-k3", None)]);
        assert_eq!(
            context_window_for(&reg, "moonshotai/kimi-k3", None, all()),
            200_000
        );
        assert_eq!(
            context_window_for(&reg, "claude-opus-4-7[1m]", None, all()),
            1_000_000
        );
        assert_eq!(context_window_for(&reg, "gpt-5.5", None, all()), 400_000);
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

    #[test]
    fn declared_window_can_shrink_as_well_as_grow() {
        let reg = registry_with_windows(&[("claude-opus-4-7", Some(32_000))]);
        assert_eq!(
            context_window_for(&reg, "claude-opus-4-7", None, all()),
            32_000
        );
    }

    #[test]
    fn non_positive_declared_window_is_treated_as_undeclared() {
        assert_eq!(declared_window(Some(0)), None);
        assert_eq!(declared_window(Some(-1)), None);
        assert_eq!(declared_window(None), None);
        assert_eq!(declared_window(Some(1_048_576)), Some(1_048_576));
    }
}
