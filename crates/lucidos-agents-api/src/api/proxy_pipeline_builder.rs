//! INVARIANT: Materializes a `Vec<Arc<dyn AuthLayer>>` from an on-disk
//! `PipelineConfig`. Called per-request so credential rotations + module
//! reloads (`POST /api/v1/proxy-modules/reload`) are picked up without
//! restarting the engine.

use crate::api::proxy::lookup_credential_value;
use crate::api::proxy_hmac_layer::HmacSignedLayer;
use crate::api::proxy_pipeline_config::{LayerConfig, PipelineConfig, StaticKind};
use crate::api::proxy_script_layer::ScriptHandshakeLayer;
use crate::api::proxy_static_layers::{QueryParamLayer, StaticHeaderLayer};
use crate::engine::proxy_wasm_signer::{
    CompiledModule, CredentialHandle as WasmCredentialHandle, SecretResolver, WasmSignerLayer,
};
use crate::proxy_auth_layer::{AuthLayer, ScopeBinding};
use crate::proxy_token_cache::ProxyTokenCache;
use async_trait::async_trait;
use axum::http::StatusCode;
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use wasmtime::Engine;

pub struct PipelineBuildContext<'a> {
    pub pool: PgPool,
    pub workspace_path: Arc<PathBuf>,
    pub token_cache: Arc<ProxyTokenCache>,
    pub proxy_name: &'a str,
    pub base_url: &'a str,
    pub proxy_modules: &'a HashMap<String, Arc<CompiledModule>>,
    pub wasm_engine: Arc<Engine>,
}

pub async fn build_pipeline(
    config: &PipelineConfig,
    ctx: &PipelineBuildContext<'_>,
) -> Result<Vec<Arc<dyn AuthLayer>>, (StatusCode, String)> {
    let mut layers: Vec<Arc<dyn AuthLayer>> = Vec::with_capacity(config.pipeline.len());
    for (idx, layer_cfg) in config.pipeline.iter().enumerate() {
        let namespace = layer_namespace(idx, layer_cfg);
        match layer_cfg {
            LayerConfig::StaticCredential {
                kind,
                credential,
                header,
                param_name,
            } => {
                let value = lookup_credential_value(&ctx.pool, credential).await?;
                let binding = ScopeBinding::StoredCredential(credential.clone());
                let layer: Arc<dyn AuthLayer> = match kind {
                    StaticKind::Bearer => {
                        Arc::new(StaticHeaderLayer::bearer(namespace, value, binding))
                    }
                    StaticKind::ApiKey => {
                        let header_name = header.as_deref().unwrap_or("Authorization");
                        Arc::new(
                            StaticHeaderLayer::api_key(namespace, header_name, value, binding)
                                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?,
                        )
                    }
                    StaticKind::Basic => {
                        let (user, pass) = value.split_once(':').ok_or_else(|| {
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!(
                                    "credential '{}' for basic auth must be 'user:pass' shape",
                                    credential
                                ),
                            )
                        })?;
                        Arc::new(StaticHeaderLayer::basic(namespace, user, pass, binding))
                    }
                    StaticKind::QueryParam => {
                        let param = param_name.clone().ok_or_else(|| {
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "static_credential kind=query_param requires `param_name`"
                                    .to_string(),
                            )
                        })?;
                        Arc::new(QueryParamLayer::new(namespace, param, value, binding))
                    }
                };
                layers.push(layer);
            }
            LayerConfig::ScriptHandshake {
                credential,
                script,
                oauth_providers,
            } => {
                let oauth_lookup: Arc<dyn crate::api::proxy_script_layer::OAuthLookup> = Arc::new(
                    crate::api::proxy_script_layer::DbOAuthLookup::new(ctx.pool.clone()),
                );
                layers.push(Arc::new(ScriptHandshakeLayer::new(
                    namespace,
                    ctx.proxy_name.to_string(),
                    credential.clone(),
                    script.clone(),
                    ctx.base_url.to_string(),
                    oauth_providers.clone(),
                    ctx.pool.clone(),
                    ctx.workspace_path.clone(),
                    ctx.token_cache.clone(),
                    oauth_lookup,
                )));
            }
            LayerConfig::HmacSigned {
                key_credential,
                secret_credential,
                key_header,
                algorithm,
                signed_payload: _signed_payload,
                signature_param,
                timestamp_param,
            } => {
                layers.push(Arc::new(HmacSignedLayer::new(
                    namespace,
                    key_credential.clone(),
                    secret_credential.clone(),
                    key_header.clone(),
                    *algorithm,
                    signature_param.clone(),
                    timestamp_param.clone(),
                    ctx.pool.clone(),
                )));
            }
            LayerConfig::WasmSigner {
                module,
                credential_handles,
            } => {
                let compiled = ctx.proxy_modules.get(module).cloned().ok_or_else(|| {
                    (
                        StatusCode::BAD_GATEWAY,
                        format!(
                            "proxy '{}' references WASM module '{}' but it is not loaded \
                             (expected `data/auth-modules/{}.wasm`)",
                            ctx.proxy_name, module, module
                        ),
                    )
                })?;
                let resolver: Arc<dyn SecretResolver> = Arc::new(PgPoolSecretResolver {
                    pool: ctx.pool.clone(),
                });
                let handles = credential_handles
                    .iter()
                    .map(|h| WasmCredentialHandle {
                        name: h.name.clone(),
                        credential: h.credential.clone(),
                    })
                    .collect();
                layers.push(Arc::new(WasmSignerLayer::new(
                    namespace,
                    compiled,
                    ctx.wasm_engine.clone(),
                    resolver,
                    handles,
                    config.granted_capabilities.clone(),
                )));
            }
        }
    }
    Ok(layers)
}

fn layer_namespace(idx: usize, layer: &LayerConfig) -> String {
    let base = match layer {
        LayerConfig::StaticCredential { kind, .. } => match kind {
            StaticKind::Bearer => "bearer",
            StaticKind::ApiKey => "api_key",
            StaticKind::Basic => "basic",
            StaticKind::QueryParam => "query_param",
        },
        LayerConfig::ScriptHandshake { .. } => "script_handshake",
        LayerConfig::HmacSigned { .. } => "hmac_signed",
        LayerConfig::WasmSigner { .. } => "wasm_signer",
    };
    if idx == 0 {
        base.to_string()
    } else {
        format!("{base}_{idx}")
    }
}

pub struct PgPoolSecretResolver {
    pub pool: PgPool,
}

#[async_trait]
impl SecretResolver for PgPoolSecretResolver {
    async fn resolve(&self, credential_id: &str) -> Result<Vec<u8>, String> {
        let value = lookup_credential_value(&self.pool, credential_id)
            .await
            .map_err(|(_, msg)| msg)?;
        Ok(value.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_namespace_is_kind_alone_first_then_kind_and_position() {
        let bearer = || LayerConfig::StaticCredential {
            kind: StaticKind::Bearer,
            credential: "x".into(),
            header: None,
            param_name: None,
        };
        assert_eq!(layer_namespace(0, &bearer()), "bearer");
        assert_eq!(layer_namespace(1, &bearer()), "bearer_1");
        assert_eq!(layer_namespace(2, &bearer()), "bearer_2");
    }

    #[test]
    fn layer_namespace_is_unique_per_kind() {
        assert_eq!(
            layer_namespace(
                0,
                &LayerConfig::StaticCredential {
                    kind: StaticKind::Bearer,
                    credential: "x".into(),
                    header: None,
                    param_name: None,
                },
            ),
            "bearer"
        );
        assert_eq!(
            layer_namespace(
                1,
                &LayerConfig::ScriptHandshake {
                    credential: Some("x".into()),
                    script: "scripts/x.py".into(),
                    oauth_providers: Vec::new(),
                },
            ),
            "script_handshake_1"
        );
    }
}
