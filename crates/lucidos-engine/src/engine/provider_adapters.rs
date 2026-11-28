use async_trait::async_trait;
use sqlx::PgPool;
use std::sync::Arc;

use crate::core::oauth_callback::CallbackRoute;
use crate::llm::chatgpt_plan::AuthServer;
use crate::llm::claude_gateway::{
    account::ClaudeGatewayAccountStore, gateway_managed_settings, served_models::ServedModels,
    telemetry,
};
use crate::llm::provider_factory::{
    ChatGptPlanAccess, GatewayAccess, GatewayAccessFactory, GatewayOrigin, ImageProvider,
    ProviderBuildContext, ProviderBuildOutcome, ProviderFactory, SignOutOutcome,
};
use crate::llm::{vertex_auth::TokenCache, vertex_defaults::LocationHandle};

use super::{
    chatgpt_plan_sign_in::ChatGptPlanSignIn,
    claude_gateway_sign_in::{self, ClaudeGatewaySignIn},
    event_bus::{BusEvent, EventBus, SystemEvent},
    provider_host::{
        ChatGptPlanStatus, ClaudeGatewaySignInState, ClaudeGatewayStatus, GatewayProxyAccess,
        StartedSignIn, WorkspaceProviders,
    },
    thread_events::MessageOrigin,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Default)]
pub struct ProviderAdapters {
    gateway_sign_in: Arc<ClaudeGatewaySignIn>,
    plan_sign_in: Arc<ChatGptPlanSignIn>,
}

#[async_trait]
impl ProviderFactory for ProviderAdapters {
    async fn build_active(
        &self,
        pool: Option<&PgPool>,
        context: &ProviderBuildContext,
    ) -> Result<ProviderBuildOutcome, BoxError> {
        crate::llm::provider_build::build_active_provider(pool, context).await
    }

    async fn build_image(
        &self,
        pool: &PgPool,
        openai_api_key: Option<&str>,
        vertex_project_id: &str,
        vertex_location: &LocationHandle,
        vertex_token_cache: &Option<TokenCache>,
    ) -> Option<Arc<dyn ImageProvider>> {
        crate::llm::image::build_image_provider(
            pool,
            openai_api_key,
            vertex_project_id,
            vertex_location,
            vertex_token_cache,
        )
        .await
    }
}

#[async_trait]
impl WorkspaceProviders for ProviderAdapters {
    fn bind_workspace(&self, pool: &PgPool, bus: &EventBus, context: &mut ProviderBuildContext) {
        context.claude_gateway_access =
            Some(claude_gateway_access_factory(pool.clone(), bus.clone()));
        context.chatgpt_plan_access = Some(chatgpt_plan_access(pool.clone(), bus.clone()));
    }

    fn start_gateway_watchers(
        &self,
        pool: &PgPool,
        bus: &EventBus,
        context: &ProviderBuildContext,
    ) {
        claude_gateway_sign_in::spawn_url_change_sign_out(
            bus.subscribe(),
            pool.clone(),
            bus.clone(),
        );
        gateway_managed_settings::spawn_refresh_task(
            bus.subscribe(),
            pool.clone(),
            bus.clone(),
            gateway_factory(context).clone(),
        );
    }

    async fn start_gateway_telemetry(
        &self,
        pool: &PgPool,
        bus: &EventBus,
        context: &ProviderBuildContext,
    ) {
        telemetry::exporter::spawn_exporter(
            bus.subscribe(),
            pool.clone(),
            gateway_factory(context).clone(),
        )
        .await;
    }

    async fn claude_gateway_status(
        &self,
        pool: &PgPool,
    ) -> Result<ClaudeGatewayStatus, sqlx::Error> {
        self.gateway_sign_in.status(pool).await
    }

    async fn claude_gateway_sign_in(
        &self,
        pool: &PgPool,
        bus: &EventBus,
        origin: GatewayOrigin,
        actor: Option<MessageOrigin>,
    ) -> Result<ClaudeGatewaySignInState, BoxError> {
        self.gateway_sign_in.start(pool, bus, origin, actor).await
    }

    async fn claude_gateway_sign_out(
        &self,
        pool: &PgPool,
        bus: &EventBus,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error> {
        self.gateway_sign_in.sign_out(pool, bus, actor).await
    }

    async fn chatgpt_plan_status(
        &self,
        pool: &PgPool,
        context: &ProviderBuildContext,
    ) -> Result<ChatGptPlanStatus, sqlx::Error> {
        let access = context
            .chatgpt_plan_access
            .as_ref()
            .expect("workspace providers bound at boot");
        self.plan_sign_in.status(pool, &access.pause).await
    }

    async fn chatgpt_plan_sign_in(
        &self,
        pool: &PgPool,
        bus: &EventBus,
        workspace: &str,
        route: CallbackRoute,
        actor: Option<MessageOrigin>,
    ) -> Result<StartedSignIn, BoxError> {
        self.plan_sign_in
            .start(pool, bus, AuthServer::openai(), workspace, route, actor)
            .await
    }

    async fn chatgpt_plan_sign_out(
        &self,
        pool: &PgPool,
        bus: &EventBus,
        forget_account: bool,
        actor: Option<MessageOrigin>,
    ) -> Result<SignOutOutcome, BoxError> {
        self.plan_sign_in
            .sign_out(pool, bus, &AuthServer::openai(), forget_account, actor)
            .await
    }

    async fn gateway_has_matching_account(
        &self,
        pool: &PgPool,
        origin: &GatewayOrigin,
    ) -> Result<bool, sqlx::Error> {
        Ok(ClaudeGatewayAccountStore::get(pool)
            .await?
            .is_some_and(|account| account.issuer == origin.as_str()))
    }

    fn gateway_proxy_access(
        &self,
        context: &ProviderBuildContext,
        origin: GatewayOrigin,
    ) -> Arc<dyn GatewayProxyAccess> {
        let access = gateway_factory(context)(origin.clone());
        Arc::new(ProxyAccess {
            access,
            served: ServedModels::new(origin),
        })
    }
}

fn gateway_factory(context: &ProviderBuildContext) -> &GatewayAccessFactory {
    context
        .claude_gateway_access
        .as_ref()
        .expect("workspace providers bound at boot")
}

struct ProxyAccess {
    access: Arc<dyn GatewayAccess>,
    served: ServedModels,
}

#[async_trait]
impl GatewayProxyAccess for ProxyAccess {
    async fn renew_rejected(&self, rejected: &str) -> Result<(), BoxError> {
        self.access.renew_rejected(rejected).await?;
        Ok(())
    }

    async fn wire_id_and_bearer(&self, model: &str) -> Result<(String, String), BoxError> {
        let policy = self.access.managed_settings().await?;
        self.served
            .wire_id(model, self.access.as_ref(), policy.as_ref())
            .await
    }
}

/// Makes the Claude gateway access. Each one reads the stored sign-in
/// from `pool`, and announces a sign-in the gateway rejected on `event_bus`.
fn claude_gateway_access_factory(pool: sqlx::PgPool, event_bus: EventBus) -> GatewayAccessFactory {
    let refresh_lock = Arc::new(crate::llm::claude_gateway::tokens::RefreshLock::default());
    Arc::new(move |origin| {
        Arc::new(
            crate::llm::claude_gateway::tokens::StoredGatewayAccess::new(
                pool.clone(),
                event_bus.clone(),
                origin,
                refresh_lock.clone(),
            ),
        )
    })
}

/// The ChatGPT plan's token source and limit pause, made once so every
/// provider build shares one refresh lock and one pause.
fn chatgpt_plan_access(pool: sqlx::PgPool, event_bus: EventBus) -> ChatGptPlanAccess {
    use crate::llm::chatgpt_plan::{AuthServer, LimitPause, StoredChatGptPlanTokens};
    let pause_bus = event_bus.clone();
    ChatGptPlanAccess {
        pause: Arc::new(LimitPause::new(move |paused_until| {
            let bus = pause_bus.clone();
            Box::pin(async move {
                bus.emit_or_log(
                    BusEvent::System(SystemEvent::ChatGptPlanLimitReached { paused_until }),
                    "[ChatGptPlan] ChatGptPlanLimitReached",
                )
                .await;
            })
        })),
        tokens: Arc::new(StoredChatGptPlanTokens::new(
            pool,
            event_bus,
            AuthServer::openai(),
            Arc::default(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider_factory::GatewayManagedSettings;
    use crate::llm::ModelNotServed;

    #[tokio::test]
    async fn chatgpt_plan_limit_emits_one_pause_event() {
        use crate::llm::chatgpt_plan::ChatGptPlanError;

        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let mut events = bus.subscribe();
        let access = chatgpt_plan_access(pool, bus);
        let error = ChatGptPlanError::from_error_object(&serde_json::json!({
            "code": "subscription_sharing_usage_limit_exceeded",
            "message": "Usage limit reached",
        }))
        .unwrap();

        access.pause.observe(&error).await;
        access.pause.observe(&error).await;

        let BusEvent::System(SystemEvent::ChatGptPlanLimitReached { paused_until }) =
            events.try_recv().expect("pause event").typed
        else {
            panic!("expected a ChatGPT plan limit event");
        };
        assert_eq!(Some(paused_until), access.pause.paused_until());
        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        drop(access);
        crate::test_support::teardown_test_db(&db_name).await;
    }

    struct ManagedGateway;

    #[async_trait]
    impl GatewayAccess for ManagedGateway {
        async fn bearer(&self) -> Result<String, BoxError> {
            Ok("test-token".into())
        }

        async fn renew_rejected(&self, _rejected: &str) -> Result<String, BoxError> {
            panic!("managed settings serve the listing without a renewal");
        }

        async fn managed_settings(&self) -> Result<Option<GatewayManagedSettings>, BoxError> {
            Ok(Some(GatewayManagedSettings::from_document(
                &serde_json::json!({ "availableModels": ["claude-haiku-5-5-20261001"] }),
            )))
        }
    }

    #[tokio::test]
    async fn gateway_proxy_access_uses_managed_settings_for_wire_models() {
        let context = ProviderBuildContext {
            default_model: "mock".into(),
            model_is_mock: true,
            vertex_project_id: String::new(),
            vertex_location: crate::llm::vertex_defaults::location_handle(String::new()),
            vertex_token_cache: None,
            model_registry: Default::default(),
            not_served: Default::default(),
            claude_gateway_access: Some(Arc::new(|_| Arc::new(ManagedGateway))),
            chatgpt_plan_access: None,
        };
        let origin = GatewayOrigin::parse("http://127.0.0.1:0").unwrap();
        let access = ProviderAdapters::default().gateway_proxy_access(&context, origin);

        assert_eq!(
            access
                .wire_id_and_bearer("claude-haiku-5-5[1m]")
                .await
                .unwrap(),
            ("claude-haiku-5-5-20261001[1m]".into(), "test-token".into())
        );
        let error = access
            .wire_id_and_bearer("claude-sonnet-5-5")
            .await
            .unwrap_err();
        assert!(error.is::<ModelNotServed>(), "{error}");
    }
}
