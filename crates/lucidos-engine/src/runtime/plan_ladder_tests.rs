#[tokio::test]
async fn ladder_availability_reuses_the_plan_listing_without_spending_it() {
    const SOL: &str = "gpt-6.1-sol";
    let (provider, requests) =
        crate::llm::openai::plan_test_support::plan_availability_fixture().await;
    use crate::llm::LlmProvider;
    for (model, expected) in [
        ("gpt-6.1-sol", true),
        ("gpt-6.2-sol", false),
        ("gpt-reserve", false),
    ] {
        assert_eq!(
            provider
                .route_available(crate::llm::ProviderKind::ChatGptPlan, model, model)
                .await
                .unwrap(),
            expected
        );
    }
    // The same account authority reaches the database-backed served ladder.
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let registry = crate::llm::model_registry::empty();
    crate::llm::model_registry::reload(&registry, &pool).await;
    let router = crate::llm::RoutingProvider::new(
        crate::llm::RoutingBackends {
            chatgpt_plan: Some(std::sync::Arc::new(provider)),
            ..Default::default()
        },
        registry.clone(),
        SOL.into(),
    );
    let routes = crate::runtime::intelligence_ladder::ModelRoutes {
        registry,
        configured: vec![crate::llm::ProviderKind::ChatGptPlan],
        provider: Some(std::sync::Arc::new(router)),
    };
    let ladder = crate::runtime::intelligence_ladder::load(
        &pool,
        &routes,
        crate::runtime::intelligence_ladder::LadderSurface::Chat,
    )
    .await;
    assert!(!ladder.pairs().is_empty());
    assert!(ladder.pairs().iter().all(|p| p.model == SOL));
    assert!(ladder
        .intelligence_rungs
        .iter()
        .any(|p| p.model == "gpt-6.2-sol"));
    crate::test_support::teardown_test_db(&db_name).await;
    assert!(requests.lock().unwrap().is_empty());
}
