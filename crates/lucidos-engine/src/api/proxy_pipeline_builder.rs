//! Materializes a `Vec<Arc<dyn AuthLayer>>` from an on-disk
//! `PipelineConfig`. Called per-request so credential rotations + module
//! reloads (`POST /api/v1/proxy-modules/reload`) are picked up without
//! restarting the engine.

use crate::api::proxy::lookup_credential_value;
use crate::api::proxy_auth_layer::AuthLayer;
use crate::api::proxy_hmac_layer::HmacSignedLayer;
use crate::api::proxy_pipeline_config::{LayerConfig, PipelineConfig, StaticKind};
use crate::api::proxy_script_layer::ScriptHandshakeLayer;
use crate::api::proxy_static_layers::{QueryParamLayer, StaticHeaderLayer};
use crate::api::proxy_token_cache::ProxyTokenCache;
use crate::api::proxy_wasm_signer::{
    CompiledModule, CredentialHandle as WasmCredentialHandle, SecretResolver, WasmSignerLayer,
};
use async_trait::async_trait;
use axum::http::StatusCode;
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use wasmtime::Engine;

/// Inputs every per-request pipeline build needs. Pulled out so the
/// dispatch site (`proxy_handle_inner`) doesn't have to thread eight
/// arguments through `build_pipeline`.
pub struct PipelineBuildContext<'a> {
    pub pool: PgPool,
    pub workspace_path: Arc<PathBuf>,
    pub token_cache: Arc<ProxyTokenCache>,
    pub proxy_name: &'a str,
    pub proxy_modules: &'a HashMap<String, Arc<CompiledModule>>,
    pub wasm_engine: Arc<Engine>,
}

/// Translate a `PipelineConfig` into a `Vec<Arc<dyn AuthLayer>>`. The
/// returned layers own everything they need (credential bytes, cache
/// handles, compiled WASM modules) — the request handler only has to call
/// `run_pipeline` against them.
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
                let layer: Arc<dyn AuthLayer> = match kind {
                    StaticKind::Bearer => Arc::new(StaticHeaderLayer::bearer(namespace, value)),
                    StaticKind::ApiKey => {
                        let header_name = header.as_deref().unwrap_or("Authorization");
                        Arc::new(
                            StaticHeaderLayer::api_key(namespace, header_name, value)
                                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?,
                        )
                    }
                    StaticKind::Basic => {
                        // Convention: the credential's auth_value is
                        // `user:password`. Split here so the layer holds
                        // the encoded form.
                        let (user, pass) = value.split_once(':').ok_or_else(|| {
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!(
                                    "credential '{}' for basic auth must be 'user:pass' shape",
                                    credential
                                ),
                            )
                        })?;
                        Arc::new(StaticHeaderLayer::basic(namespace, user, pass))
                    }
                    StaticKind::QueryParam => {
                        let param = param_name.clone().ok_or_else(|| {
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "static_credential kind=query_param requires `param_name`"
                                    .to_string(),
                            )
                        })?;
                        Arc::new(QueryParamLayer::new(namespace, param, value))
                    }
                };
                layers.push(layer);
            }
            LayerConfig::ScriptHandshake {
                credential,
                script,
                oauth_providers,
            } => {
                let oauth_lookup: Arc<dyn crate::api::proxy_script_layer::OAuthLookup> =
                    Arc::new(crate::api::proxy_script_layer::DbOAuthLookup::new(
                        ctx.pool.clone(),
                    ));
                layers.push(Arc::new(ScriptHandshakeLayer::new(
                    namespace,
                    ctx.proxy_name.to_string(),
                    credential.clone(),
                    script.clone(),
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
                let resolver: Arc<dyn SecretResolver> =
                    Arc::new(PgPoolSecretResolver { pool: ctx.pool.clone() });
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

/// Stable per-layer namespace. Pipelines almost always have at most one
/// layer of each kind, so the kind name alone is unique. When two layers
/// of the same kind are used (rare), a numeric suffix keeps them distinct.
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

/// SecretResolver impl that pulls secret bytes from the credential store.
/// Used by `WasmSignerLayer` at request time.
pub struct PgPoolSecretResolver {
    pub pool: PgPool,
}

#[async_trait]
impl SecretResolver for PgPoolSecretResolver {
    async fn resolve(&self, credential_id: &str) -> Result<Vec<u8>, String> {
        // Unwrap the (StatusCode, String) error into a plain string — the
        // SecretResolver trait is platform-agnostic.
        let value = lookup_credential_value(&self.pool, credential_id)
            .await
            .map_err(|(_, msg)| msg)?;
        Ok(value.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Wasmtime-`Engine`-creating tests for this module live in
    // `crates/lucidos-engine/tests/proxy_wasm_engine.rs`. See the
    // diagnostic block atop `engine::change_ops::tests` for why.

    #[test]
    fn layer_namespace_is_unique_per_kind() {
        // Two layers of different kinds → distinct namespaces.
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
                    credential: "x".into(),
                    script: "scripts/x.py".into(),
                    oauth_providers: Vec::new(),
                },
            ),
            "script_handshake_1"
        );
    }
}
