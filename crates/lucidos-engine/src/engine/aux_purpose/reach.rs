//! Whether a model is *reachable*: a configured provider serves it, and that
//! provider has not answered that it lacks the model within the window. The
//! second half is how an unset default leaves a *not-served model* (ADR 0403).

use std::time::Duration;

use sqlx::PgPool;

use crate::llm::model_registry::{resolve_route, ModelRegistry, ProviderKind, RouteEntry};

/// How long a not-found answer keeps a model out of default selection. After
/// it, the next call tries the model again, so enabling it in the provider's
/// console takes effect with no restart.
pub(crate) const NOT_SERVED_WINDOW: Duration = Duration::from_secs(6 * 60 * 60);

/// The event that records one not-found answer.
pub(crate) const NOT_SERVED_EVENT: &str = "ModelNotServedObserved";

/// The `(provider, model)` pairs a provider answered not-found for within
/// [`NOT_SERVED_WINDOW`], read from the events. A restart keeps them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct NotServed(Vec<(ProviderKind, String)>);

impl NotServed {
    /// The pairs observed within the window.
    ///
    /// A failed read is unknown, not "nothing is retired": it returns no pairs,
    /// so the call goes out and a retired model answers for itself.
    pub(crate) async fn recent(pool: &PgPool) -> Self {
        let rows: Result<Vec<(String, String)>, _> = sqlx::query_as(
            "SELECT DISTINCT payload->'data'->>'provider', payload->'data'->>'model' FROM events \
             WHERE event_type = $1 AND created >= now() - make_interval(secs => $2)",
        )
        .bind(NOT_SERVED_EVENT)
        .bind(NOT_SERVED_WINDOW.as_secs() as f64)
        .fetch_all(pool)
        .await;
        match rows {
            Ok(rows) => Self(
                rows.into_iter()
                    .map(|(provider, model)| (ProviderKind::parse(&provider), model))
                    .collect(),
            ),
            Err(e) => {
                crate::log!("[AuxPurpose] could not read not-served models: {}", e);
                Self::default()
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn of(pairs: &[(ProviderKind, &str)]) -> Self {
        Self(
            pairs
                .iter()
                .map(|(kind, model)| (*kind, model.to_string()))
                .collect(),
        )
    }

    fn contains(&self, provider: ProviderKind, model: &str) -> bool {
        self.0.iter().any(|(p, m)| *p == provider && m == model)
    }
}

/// What decides reachability for one selection: the configured providers,
/// and the models they recently answered not-found for.
#[derive(Clone, Debug)]
pub(crate) struct Reach {
    configured: Vec<ProviderKind>,
    not_served: NotServed,
}

impl Reach {
    pub(crate) fn new(configured: Vec<ProviderKind>, not_served: NotServed) -> Self {
        Self {
            configured,
            not_served,
        }
    }

    /// `kinds` configured, and nothing answered not-found.
    #[cfg(test)]
    pub(crate) fn configured(kinds: &[ProviderKind]) -> Self {
        Self::new(kinds.to_vec(), NotServed::default())
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
            .is_some_and(|route| self.not_served.contains(route.provider, model))
    }

    /// Whether a call on `model` can be expected to answer.
    pub(crate) fn serves(&self, registry: &ModelRegistry, model: &str) -> bool {
        self.route(registry, model).is_some() && !self.answered_not_found(registry, model)
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

    /// The record is the events table, so it survives a restart and lapses on
    /// its own once the window passes (I5).
    #[tokio::test]
    async fn an_observation_counts_inside_the_window_and_lapses_after_it() {
        let (pool, db_name) = setup_test_db().await;
        let window = NOT_SERVED_WINDOW.as_secs() as i64;
        observe(&pool, "vertex", "claude-haiku-4-5", 60).await;
        observe(&pool, "anthropic", "claude-haiku-4-5", window + 60).await;
        let recent = NotServed::recent(&pool).await;
        assert_eq!(
            recent,
            NotServed::of(&[(ProviderKind::Vertex, "claude-haiku-4-5")])
        );
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The pair is the provider AND the model. Vertex refusing Haiku says
    /// nothing about Anthropic serving it, or Vertex serving Gemini.
    #[test]
    fn only_the_refusing_route_is_not_served() {
        let registry = crate::llm::model_registry::empty();
        let refused = NotServed::of(&[(ProviderKind::Vertex, "claude-haiku-4-5")]);
        let vertex = Reach::new(vec![ProviderKind::Vertex], refused.clone());
        let anthropic = Reach::new(vec![ProviderKind::Anthropic], refused);
        // An unlisted Claude id goes to Vertex by the prefix heuristic.
        assert!(!vertex.serves(&registry, "claude-haiku-4-5"));
        assert!(vertex.answered_not_found(&registry, "claude-haiku-4-5"));
        assert!(vertex.serves(&registry, "gemini-3-flash-preview"));
        assert!(!anthropic.answered_not_found(&registry, "claude-haiku-4-5"));
    }
}
