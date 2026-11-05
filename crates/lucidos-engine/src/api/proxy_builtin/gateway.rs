//! Per-dispatch gateway access. Only Messages requests may spend the sign-in.

use super::*;
use crate::api::proxy_auth_layer::BodyView;
use crate::llm::anthropic_wire::{parse_context_suffix, ANTHROPIC_BETA_1M_CONTEXT};
use crate::llm::claude_gateway::{served_models::ServedModels, GatewayAccess, GatewayOrigin};

pub(super) async fn resolve(
    engine: &Arc<crate::engine::LucidosEngine>,
) -> Result<BuiltinTarget, (StatusCode, String)> {
    resolve_for(engine.pool(), &engine.claude_gateway_access).await
}

async fn resolve_for(
    pool: &sqlx::PgPool,
    factory: &crate::llm::claude_gateway::GatewayAccessFactory,
) -> Result<BuiltinTarget, (StatusCode, String)> {
    let url = prefs::CLAUDE_GATEWAY_URL
        .try_read(pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            unconfigured_msg(
                "claude-gateway",
                "a gateway URL",
                "set the Claude gateway URL in Settings → Models → Providers",
            )
        })?;
    let origin = GatewayOrigin::parse(&url).map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    let access = factory(origin.clone());
    // Discovery reads the account without refreshing or exporting its token.
    let account = crate::llm::claude_gateway::account::ClaudeGatewayAccountStore::get(pool)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Could not read the Claude gateway sign-in: {e}"),
            )
        })?;
    if account.is_none_or(|account| account.issuer != origin.as_str()) {
        return Err(unconfigured_msg(
            "claude-gateway",
            "a matching sign-in",
            "sign in to the configured gateway in Settings → Models → Providers",
        ));
    }
    Ok((
        origin.api_base_url(),
        vec![Arc::new(GatewayLayer::new(origin, access))],
    ))
}

pub(super) struct GatewayLayer {
    base: String,
    access: Arc<dyn GatewayAccess>,
    served: ServedModels,
    // A layer belongs to one dispatch, including its redirects and one retry.
    bearer: tokio::sync::Mutex<Option<(String, bool)>>,
}

impl GatewayLayer {
    pub(super) fn new(origin: GatewayOrigin, access: Arc<dyn GatewayAccess>) -> Self {
        Self {
            base: origin.api_base_url(),
            access,
            served: ServedModels::new(origin),
            bearer: tokio::sync::Mutex::new(None),
        }
    }
}

#[async_trait]
impl AuthLayer for GatewayLayer {
    fn requires_raw_body(&self) -> bool {
        true
    }
    fn output_namespace(&self) -> &str {
        "claude-gateway"
    }
    fn scope_bindings(&self) -> Vec<ScopeBinding> {
        vec![pinned("the Claude gateway sign-in", &self.base)]
    }
    fn retry_on_401(&self) -> RetryHint {
        RetryHint::InvalidateAndRetry
    }
    async fn invalidate_cache(&self) {
        if let Some((_, rejected)) = self.bearer.lock().await.as_mut() {
            *rejected = true;
        }
    }
    async fn apply(&self, input: &LayerInput<'_>) -> Result<AuthMutation, (StatusCode, String)> {
        let bad_request = |message: &str| (StatusCode::BAD_REQUEST, message.to_string());
        if input.method != axum::http::Method::POST
            || input.url.split('?').next() != Some(format!("{}/messages", self.base).as_str())
        {
            return Err(bad_request(
                "proxy 'claude-gateway' forwards only POST /messages",
            ));
        }
        let BodyView::Raw(body) = input.body else {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Claude gateway policy requires the raw Messages body".into(),
            ));
        };
        let mut document: serde_json::Value = serde_json::from_slice(body)
            .map_err(|_| bad_request("Claude gateway Messages body must be JSON"))?;
        let model = document
            .get("model")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| bad_request("Claude gateway Messages body requires a model"))?;
        let failed = |e: Box<dyn std::error::Error + Send + Sync>| {
            let status = if e.is::<crate::llm::ModelNotServed>() {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::BAD_GATEWAY
            };
            (status, e.to_string())
        };
        let mut kept = self.bearer.lock().await;
        if let Some((token, true)) = kept.as_ref() {
            self.access.renew_rejected(token).await.map_err(failed)?;
        }
        let policy = self.access.managed_settings().await.map_err(failed)?;
        let (wire_id, token) = self
            .served
            .wire_id(model, self.access.as_ref(), policy.as_ref())
            .await
            .map_err(failed)?;
        // `[1m]` is Lucidos's own suffix: the gateway takes the beta, never the id.
        let (listed, one_million) = parse_context_suffix(&wire_id);
        document["model"] = listed.into();
        *kept = Some((token.clone(), false));
        let mut add_headers = vec![(
            HeaderName::from_static("authorization"),
            format!("Bearer {token}"),
        )];
        if one_million {
            let betas = input
                .headers
                .iter()
                .filter(|(name, _)| name == "anthropic-beta")
                .map(|(_, value)| value.as_str())
                .chain([ANTHROPIC_BETA_1M_CONTEXT]);
            add_headers.push((
                HeaderName::from_static("anthropic-beta"),
                betas.collect::<Vec<_>>().join(","),
            ));
        }
        Ok(AuthMutation {
            add_headers,
            replace_body: Some(
                serde_json::to_vec(&document)
                    .expect("JSON value serializes")
                    .into(),
            ),
            cache_was_hit: true,
            ..Default::default()
        })
    }
}

#[cfg(test)]
#[path = "gateway_tests.rs"]
mod tests;
