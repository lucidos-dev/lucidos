//! A subscriber that mirrors a setting re-reads it after a broadcast lag, so a
//! change carried by a dropped event still lands without a restart.

use std::sync::{Arc, RwLock};

use crate::core::models::{ModelFields, Route};
use crate::core::preferences::PreferenceStore;
use crate::engine::event_bus::EventBus;
use crate::engine::{follow_models_table, follow_vertex_region, provider_config_read, MirrorRead};
use crate::test_support::{flood_past_bus_capacity, setup_test_db, teardown_test_db};

#[tokio::test]
async fn a_vertex_region_change_lost_to_a_lag_still_lands() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let mut rx = bus.subscribe();
    let location = Arc::new(RwLock::new("europe-west1".to_string()));

    PreferenceStore::set(
        &pool,
        &bus,
        crate::core::prefs::VERTEX_REGION.key(),
        "us-central1",
        None,
    )
    .await
    .unwrap();
    flood_past_bus_capacity(&bus);

    assert!(follow_vertex_region(rx.recv().await, &location, &pool).await);
    assert_eq!(*location.read().unwrap(), "us-central1");

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_model_created_during_a_lag_reaches_the_registry() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let mut rx = bus.subscribe();
    let registry = crate::llm::model_registry::empty();

    let fields = ModelFields {
        label: "Lag Model".to_string(),
        routes: vec![Route::bare("local")],
        preferred_provider: None,
        vision: false,
        sort_order: 1000,
    };
    crate::core::ModelStore::create(&pool, &bus, "lag-model", &fields, None)
        .await
        .unwrap();
    flood_past_bus_capacity(&bus);

    assert!(follow_models_table(rx.recv().await, &registry, &pool).await);
    assert!(registry.read().unwrap().contains_key("lag-model"));

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The provider build reads credentials and preferences itself, so a lag only
/// has to ask for a rebuild.
#[tokio::test]
async fn a_lag_asks_the_provider_subscriber_to_rebuild() {
    let bus = crate::test_support::offline_event_bus();
    let mut rx = bus.subscribe();
    flood_past_bus_capacity(&bus);

    assert!(matches!(
        provider_config_read(rx.recv().await),
        MirrorRead::Refresh(_)
    ));
}
