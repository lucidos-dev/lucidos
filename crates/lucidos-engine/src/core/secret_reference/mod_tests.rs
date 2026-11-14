use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// A resolver that answers from a script of outcomes and counts its calls.
#[derive(Default)]
pub(crate) struct ScriptedResolver {
    outcomes: Mutex<VecDeque<Result<String, ResolveFailure>>>,
    pub(crate) calls: AtomicUsize,
}

impl ScriptedResolver {
    pub(crate) fn answering(outcomes: Vec<Result<String, ResolveFailure>>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into()),
            calls: AtomicUsize::new(0),
        }
    }

    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SecretResolver for ScriptedResolver {
    async fn resolve(&self, _reference: &SecretReference) -> Result<String, ResolveFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.outcomes
            .lock()
            .unwrap()
            .pop_front()
            .expect("the test scripted fewer outcomes than calls")
    }
}

fn reference() -> SecretReference {
    SecretReference::parse("op://Engineering/Stripe/credential").unwrap()
}

fn transport() -> ResolveFailure {
    ResolveFailure::Transport("op could not reach 1Password".to_string())
}

fn definitive() -> ResolveFailure {
    ResolveFailure::Definitive("op said: \"Stripe\" isn't an item".to_string())
}

#[test]
fn a_reference_names_a_vault_an_item_and_a_field_with_an_optional_section() {
    for good in [
        "op://Engineering/Stripe/credential",
        "op://Engineering/Stripe/Live keys/credential",
    ] {
        assert!(SecretReference::parse(good).is_ok(), "{good}");
    }
    for bad in [
        "op://Engineering/Stripe",
        "op://Engineering//credential",
        "op://a/b/c/d/e",
        "op://Engineering/Stripe/cred\nential",
        "https://Engineering/Stripe/credential",
    ] {
        assert!(SecretReference::parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn a_field_starting_with_the_scheme_is_a_reference_and_anything_else_is_stored() {
    assert_eq!(
        SecretField::parse("sk-live-123").unwrap(),
        SecretField::Stored("sk-live-123".to_string())
    );
    assert_eq!(
        SecretField::parse("op://Engineering/Stripe/credential").unwrap(),
        SecretField::Reference(reference())
    );
    assert!(
        SecretField::parse("op://Engineering").is_err(),
        "a malformed reference is refused, never sent as a secret"
    );
}

#[tokio::test(start_paused = true)]
async fn a_fresh_value_serves_without_asking_again_and_a_stale_one_asks() {
    let cache = ReferenceCache::default();
    let inner = ScriptedResolver::answering(vec![Ok("v1".into()), Ok("v2".into())]);

    assert_eq!(cache.resolve(&inner, &reference()).await.unwrap(), "v1");
    tokio::time::advance(FRESH_FOR - Duration::from_secs(1)).await;
    assert_eq!(cache.resolve(&inner, &reference()).await.unwrap(), "v1");
    assert_eq!(inner.calls(), 1, "a fresh value runs no op");

    tokio::time::advance(Duration::from_secs(2)).await;
    assert_eq!(cache.resolve(&inner, &reference()).await.unwrap(), "v2");
    assert_eq!(inner.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_transport_failure_serves_the_last_good_value_for_at_most_a_day() {
    let cache = ReferenceCache::default();
    let inner =
        ScriptedResolver::answering(vec![Ok("good".into()), Err(transport()), Err(transport())]);
    cache.resolve(&inner, &reference()).await.unwrap();

    tokio::time::advance(Duration::from_secs(23 * 60 * 60)).await;
    assert_eq!(
        cache.resolve(&inner, &reference()).await.unwrap(),
        "good",
        "an outage inside a day keeps the last good value"
    );

    tokio::time::advance(Duration::from_secs(2 * 60 * 60)).await;
    assert_eq!(
        cache.resolve(&inner, &reference()).await,
        Err(transport()),
        "a 25-hour-old value is never served"
    );
}

#[tokio::test(start_paused = true)]
async fn a_definitive_failure_drops_the_value_at_once() {
    let cache = ReferenceCache::default();
    let inner =
        ScriptedResolver::answering(vec![Ok("good".into()), Err(definitive()), Err(transport())]);
    cache.resolve(&inner, &reference()).await.unwrap();
    tokio::time::advance(FRESH_FOR).await;

    assert_eq!(cache.resolve(&inner, &reference()).await, Err(definitive()));
    assert_eq!(
        cache.resolve(&inner, &reference()).await,
        Err(transport()),
        "after a not-found, a later outage has no value left to serve"
    );
}

#[tokio::test(start_paused = true)]
async fn clearing_the_cache_asks_again() {
    let cache = ReferenceCache::default();
    let inner = ScriptedResolver::answering(vec![Ok("v1".into()), Ok("v2".into())]);
    cache.resolve(&inner, &reference()).await.unwrap();
    cache.clear();
    assert_eq!(cache.resolve(&inner, &reference()).await.unwrap(), "v2");
}

/// A resolver that clears the cache while it reads, as a service account
/// removed mid-resolve does.
struct ClearingResolver<'a> {
    cache: &'a ReferenceCache,
}

#[async_trait]
impl SecretResolver for ClearingResolver<'_> {
    async fn resolve(&self, _reference: &SecretReference) -> Result<String, ResolveFailure> {
        self.cache.clear();
        Ok("read under the old account".into())
    }
}

#[tokio::test(start_paused = true)]
async fn a_resolve_that_straddles_a_clear_is_not_cached() {
    let cache = ReferenceCache::default();
    let clearing = ClearingResolver { cache: &cache };
    cache.resolve(&clearing, &reference()).await.unwrap();

    let next = ScriptedResolver::answering(vec![Ok("v2".into())]);
    assert_eq!(cache.resolve(&next, &reference()).await.unwrap(), "v2");
    assert_eq!(next.calls(), 1, "the straddling value must not serve");
}
