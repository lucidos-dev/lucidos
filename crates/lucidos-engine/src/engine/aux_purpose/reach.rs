//! Whether a model is *reachable*: a configured provider serves it, and that
//! provider has not answered that it lacks the model within the window. The
//! second half is how an unset default leaves a *not-served model* (ADR 0403).
//!
//! The window is the router's [`NotServedLedger`], so default selection,
//! Settings and the router resolve one model from one view.

use std::time::Duration;

use sqlx::PgPool;

use crate::llm::model_registry::{
    reaches, resolve_generation, resolve_route, GenerationRule, ModelRegistry, ProviderKind,
    ResolvedGeneration, RouteEntry,
};
use crate::llm::not_served::{NotServedLedger, NOT_SERVED_WINDOW};
use crate::llm::LlmProvider;

/// The event that records one not-found answer.
pub(crate) const NOT_SERVED_EVENT: &str = "ModelNotServedObserved";

/// Seed `ledger` with the not-found answers recorded within the window, so a
/// restart keeps them. Ages come from the database clock (ADR 0053).
///
/// A failed read is unknown, not "nothing is refused": it seeds nothing, so
/// the call goes out and a refused model answers for itself.
pub(crate) async fn seed_not_served(pool: &PgPool, ledger: &NotServedLedger) {
    let rows: Result<Vec<(String, String, i64)>, _> = sqlx::query_as(
        "SELECT payload->'data'->>'provider', payload->'data'->>'model', \
         EXTRACT(EPOCH FROM now() - max(created))::bigint FROM events \
         WHERE event_type = $1 AND created >= now() - make_interval(secs => $2) \
         GROUP BY 1, 2",
    )
    .bind(NOT_SERVED_EVENT)
    .bind(NOT_SERVED_WINDOW.as_secs() as f64)
    .fetch_all(pool)
    .await;
    match rows {
        Ok(rows) => {
            for (provider, model, age) in rows {
                let age = Duration::from_secs(age.max(0) as u64);
                ledger.seed(ProviderKind::parse(&provider), &model, age);
            }
        }
        Err(e) => crate::log!("[AuxPurpose] could not read not-served models: {}", e),
    }
}

/// What decides reachability for one selection: the configured providers,
/// and the models they recently answered not-found for.
#[derive(Clone, Debug)]
pub(crate) struct Reach {
    configured: Vec<ProviderKind>,
    not_served: NotServedLedger,
}

impl Reach {
    pub(crate) fn new(configured: Vec<ProviderKind>, not_served: NotServedLedger) -> Self {
        Self {
            configured,
            not_served,
        }
    }

    /// What `router` reaches: its providers, and its ledger. A router with no
    /// ledger gets one seeded from the events.
    pub(crate) async fn of(pool: &PgPool, router: &dyn LlmProvider) -> Self {
        // An unfiltered router (`None`) answers for every provider.
        let configured = router
            .configured_providers()
            .unwrap_or_else(|| ProviderKind::ALL.to_vec());
        let not_served = match router.not_served_ledger() {
            Some(ledger) => ledger,
            None => {
                let ledger = NotServedLedger::default();
                seed_not_served(pool, &ledger).await;
                ledger
            }
        };
        Self::new(configured, not_served)
    }

    /// `kinds` configured, and nothing answered not-found.
    #[cfg(test)]
    pub(crate) fn configured(kinds: &[ProviderKind]) -> Self {
        Self::new(kinds.to_vec(), NotServedLedger::default())
    }

    /// `kinds` configured, and `refused` answered not-found just now.
    #[cfg(test)]
    pub(crate) fn refusing(kinds: &[ProviderKind], refused: &[(ProviderKind, &str)]) -> Self {
        let ledger = NotServedLedger::default();
        for (provider, model) in refused {
            ledger.mark(*provider, model);
        }
        Self::new(kinds.to_vec(), ledger)
    }

    pub(crate) fn is_configured(&self, kind: ProviderKind) -> bool {
        self.configured.contains(&kind)
    }

    /// The route a call on `model` takes, the way the router resolves it.
    pub(crate) fn route(&self, registry: &ModelRegistry, model: &str) -> Option<RouteEntry> {
        resolve_route(registry, model, None, |kind| self.is_configured(kind)).ok()
    }

    /// Whether `model`'s route answered not-found within the window.
    pub(crate) fn answered_not_found(&self, registry: &ModelRegistry, model: &str) -> bool {
        self.route(registry, model)
            .is_some_and(|route| self.not_served.refused(route.provider, model))
    }

    /// Whether `model` itself, as sent, can be expected to answer.
    fn reaches(&self, registry: &ModelRegistry, model: &str) -> bool {
        reaches(
            registry,
            model,
            None,
            |kind| self.is_configured(kind),
            |provider, model| self.not_served.refused(provider, model),
        )
    }

    /// The model a call naming `model` is sent as under `rule`, as the router
    /// resolves it.
    pub(crate) fn resolve(
        &self,
        registry: &ModelRegistry,
        model: &str,
        rule: GenerationRule,
    ) -> ResolvedGeneration {
        resolve_generation(registry, model, rule, |m| self.reaches(registry, m))
    }

    /// Whether a call naming `model` under `rule` can be expected to answer.
    pub(crate) fn serves(
        &self,
        registry: &ModelRegistry,
        model: &str,
        rule: GenerationRule,
    ) -> bool {
        self.reaches(registry, &self.resolve(registry, model, rule).model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use serde_json::json;

    async fn observe(pool: &PgPool, provider: &str, model: &str, age_secs: i64) {
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id, created) \
             VALUES ($1, $2, $3, 'ops', 'global', now() - make_interval(secs => $4))",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(NOT_SERVED_EVENT)
        .bind(json!({ "type": NOT_SERVED_EVENT, "data": { "provider": provider, "model": model } }))
        .bind(age_secs as f64)
        .execute(pool)
        .await
        .expect("insert an observation");
    }

    /// The events seed the ledger, so the window survives a restart and an
    /// observation outside it counts for nothing (I9).
    #[tokio::test]
    async fn the_events_seed_the_ledger_inside_the_window() {
        let (pool, db_name) = setup_test_db().await;
        let window = NOT_SERVED_WINDOW.as_secs() as i64;
        observe(&pool, "vertex", "claude-haiku-5-5", 3600).await;
        observe(&pool, "anthropic", "claude-haiku-4-5", window + 60).await;
        let ledger = NotServedLedger::default();
        seed_not_served(&pool, &ledger).await;
        assert_eq!(
            ledger.snapshot(),
            vec![(ProviderKind::Vertex, "claude-haiku-5-5".to_string())]
        );
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The pair is the provider AND the model. Vertex refusing Haiku says
    /// nothing about Anthropic serving it, or Vertex serving Gemini.
    #[test]
    fn only_the_refusing_route_is_not_served() {
        let registry = crate::llm::model_registry::empty();
        let refused = [(ProviderKind::Vertex, "claude-haiku-4-5")];
        let vertex = Reach::refusing(&[ProviderKind::Vertex], &refused);
        let anthropic = Reach::refusing(&[ProviderKind::Anthropic], &refused);
        let any = GenerationRule::AnyGeneration;
        // An unlisted Claude id goes to Vertex by the prefix heuristic.
        assert!(!vertex.serves(&registry, "claude-haiku-4-5", any));
        assert!(vertex.answered_not_found(&registry, "claude-haiku-4-5"));
        assert!(vertex.serves(&registry, "gemini-3-flash-preview", any));
        assert!(!anthropic.answered_not_found(&registry, "claude-haiku-4-5"));
    }

    /// Reach resolves within the lineage the way the router does. A stored
    /// pick steps back past a refused successor; a default does not (I4).
    #[test]
    fn reach_follows_the_lineage_by_its_rule() {
        use crate::llm::model_registry::Generation;
        let registry = crate::llm::routing::lineage_stub::lineage_registry();
        let reach = Reach::refusing(&[ProviderKind::Local], &[(ProviderKind::Local, "new")]);
        let (any, named) = (GenerationRule::AnyGeneration, GenerationRule::NamedOrNewer);
        assert_eq!(
            reach.resolve(&registry, "old", named).generation,
            Generation::Held {
                successor: "new".to_string()
            }
        );
        let stepped = reach.resolve(&registry, "new", any);
        assert_eq!(
            (stepped.model.as_str(), stepped.generation),
            ("old", Generation::Predecessor)
        );
        assert!(reach.serves(&registry, "new", any));
        assert!(!reach.serves(&registry, "new", named));
    }
}
