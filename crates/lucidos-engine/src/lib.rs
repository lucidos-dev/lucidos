#![deny(clippy::print_stdout, clippy::print_stderr)]

#[macro_use]
extern crate lucidos_core;

pub use lucidos_core::{
    log, log_legacy, log_line, log_stamp, module_label, providers_settings, restore_floor_sql,
    typed_log_line,
};

pub mod app_reach_policy;
pub use lucidos_core::boot_failure;
pub use lucidos_core::boot_report;
pub use lucidos_core::capability_manifest;
#[doc(hidden)]
pub mod codegen_inputs;
pub use lucidos_core::core;
pub use lucidos_core::diagnostic_bundles;
pub use lucidos_core::diagnostic_log;
pub mod engine;
pub use lucidos_core::engine::{proxy_auth_layer, proxy_token_cache};
pub use lucidos_core::engine_runtime;
#[cfg(any(test, feature = "test-support"))]
mod engine_test_support;
pub use lucidos_core::gateway_auth;
pub use lucidos_core::gateway_devices;
pub use lucidos_core::gateway_users;
#[cfg(test)]
mod guard_tests;
pub use lucidos_core::internal_faults;
pub mod landing_gateway;
pub use lucidos_core::llm;
pub mod mcp;
pub mod memory;
#[cfg(test)]
mod migration_tests;
pub use lucidos_core::net_config;
pub use lucidos_core::paths;
#[cfg(test)]
mod rung_vocabulary_tests;
pub mod runtime;
pub mod scheduler;
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod test_support {
    #[cfg(test)]
    pub use crate::engine_test_support::boot_mock_engine;
    pub use crate::engine_test_support::{boot_mock_engine_with_host, Fixed384Embedder};
    pub use lucidos_core::test_support::*;

    #[cfg(test)]
    pub async fn setup_test_db() -> (sqlx::PgPool, String) {
        crate::core::secret_store::key_store::install_test_key_store();
        lucidos_core::test_support::setup_test_db().await
    }
}
pub mod triggers;
pub mod voice;

pub use engine::LucidosEngine;

pub use engine::proxy_wasm_signer::jit_selftest;

/// INVARIANT: Hidden re-exports of crate-internal items exercised by the wasmtime
/// integration tests in `tests/proxy_wasm_engine.rs`.
///
/// **Why this exists.** Those tests originally lived in `#[cfg(test)]`
/// `mod tests` blocks inside `engine/proxy_wasm_*.rs` and friends. Each
/// test created a `wasmtime::Engine` (and a `Module`/`Store`), all of
/// which `MAP_JIT`-`mmap` JIT memory and exchange Mach messages with
/// the kernel under macOS. The parallel `cargo test -p lucidos-engine
/// --lib` schedule allocated those concurrently across many
/// `#[tokio::test]` runtimes, crossing a per-process Mach IPC threshold
/// and crashing libdispatch with `mach_msg failed with 268451845`
/// (`MACH_RCV_INTERRUPTED`, SIGABRT). Moving the tests to a separate
/// `[[test]]` binary gives them their own process — own per-process
/// budget — and the abort goes away. The bisection details live atop
/// `engine::change_ops::tests`.
///
/// `#[doc(hidden)]` keeps these out of public API docs. They're
/// re-exported individually (not as whole modules) because `pub use`
/// of a `pub(crate)` module is rejected (E0365).
#[doc(hidden)]
pub mod __wasm_test_internals {
    pub use crate::engine::proxy_auth_layer::{AuthLayer, AuthMutation, BodyView, LayerInput};
    pub use crate::engine::proxy_token_cache::ProxyTokenCache;
    pub use crate::engine::proxy_wasm_host::{
        register_host_imports, HostState, LogBudget, LOG_BYTES_PER_INVOCATION,
        LOG_LINES_PER_INVOCATION,
    };
    pub use crate::engine::proxy_wasm_signer::{
        build_wasmtime_engine, load_wasm_modules, BodyMode, CompiledModule, CredentialHandle,
        SecretResolver, SignInput, SignInputBody, SignOutput, SignerLimits, WasmManifest,
        WasmSignerLayer, CAP_READ_PRIOR_HEADERS, CAP_REPLACE_BODY, WITHHELD_HEADER_VALUE,
    };
    pub use wasmtime::Engine;

    use std::collections::HashMap;

    pub struct MapResolver(pub HashMap<String, Vec<u8>>);

    #[async_trait::async_trait]
    impl SecretResolver for MapResolver {
        async fn resolve(&self, credential_id: &str) -> Result<Vec<u8>, String> {
            self.0
                .get(credential_id)
                .cloned()
                .ok_or_else(|| format!("test resolver: no credential `{credential_id}`"))
        }
    }
}

pub struct BuildIdentity {
    pub id: &'static str,
    pub release: &'static str,
    pub release_dirty: bool,
    pub engine_version: &'static str,
}

static BUILD_IDENTITY: std::sync::OnceLock<BuildIdentity> = std::sync::OnceLock::new();

impl BuildIdentity {
    pub fn install(self) {
        assert!(
            BUILD_IDENTITY.set(self).is_ok(),
            "engine build identity already installed"
        );
    }
}

const UNSTAMPED_BUILD: BuildIdentity = BuildIdentity {
    id: "",
    release: "",
    release_dirty: false,
    engine_version: "",
};

pub fn build_identity() -> &'static BuildIdentity {
    if let Some(identity) = BUILD_IDENTITY.get() {
        return identity;
    }
    // INVARIANT: Release-dependent unit tests get the repository RELEASE, compiled into
    // the test build only, so the shipped library stays unstamped.
    #[cfg(test)]
    {
        static TEST_BUILD: BuildIdentity = BuildIdentity {
            release: include_str!("../../../RELEASE").trim_ascii_end(),
            ..UNSTAMPED_BUILD
        };
        &TEST_BUILD
    }
    #[cfg(not(test))]
    &UNSTAMPED_BUILD
}

#[cfg(test)]
pub(crate) mod test_util {
    use crate::memory::EmbeddingProvider;
    use async_trait::async_trait;

    #[cfg(feature = "real-embedder-tests")]
    enum SharedEmbedder {
        Ready(crate::memory::FastEmbedProvider),
        FetchUnavailable(String),
    }

    #[cfg(feature = "real-embedder-tests")]
    use crate::memory::fastembed::is_model_fetch_failure;

    #[cfg(feature = "real-embedder-tests")]
    pub fn shared_embedder() -> Option<&'static crate::memory::FastEmbedProvider> {
        use crate::memory::FastEmbedProvider;
        use std::sync::OnceLock;
        static PROVIDER: OnceLock<SharedEmbedder> = OnceLock::new();
        let outcome = PROVIDER.get_or_init(|| match FastEmbedProvider::new() {
            Ok(provider) => SharedEmbedder::Ready(provider),
            Err(e) if is_model_fetch_failure(e.as_ref()) => {
                SharedEmbedder::FetchUnavailable(e.to_string())
            }
            Err(e) => panic!("FastEmbedProvider init failed (not a model-fetch error): {e}"),
        });
        match outcome {
            SharedEmbedder::Ready(provider) => Some(provider),
            SharedEmbedder::FetchUnavailable(msg) => {
                crate::log_legacy!(
                    "[real-embedder-tests] SKIP: MultilingualE5Small model unavailable \
                     (huggingface.co fetch failed): {msg}"
                );
                None
            }
        }
    }

    pub struct KeywordEmbedder {
        keywords: Vec<String>,
    }

    impl KeywordEmbedder {
        pub fn new(keywords: &[&str]) -> Self {
            assert!(
                keywords.iter().all(|k| !k.is_empty()),
                "KeywordEmbedder keywords must be non-empty"
            );
            Self {
                keywords: keywords.iter().map(|k| k.to_lowercase()).collect(),
            }
        }

        fn encode(&self, text: &str) -> Vec<f32> {
            let lower = text.to_lowercase();
            let mut v: Vec<f32> = self
                .keywords
                .iter()
                .map(|kw| if lower.contains(kw) { 1.0 } else { 0.0 })
                .collect();
            let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for x in &mut v {
                    *x /= norm;
                }
            }
            v
        }
    }

    #[async_trait]
    impl EmbeddingProvider for KeywordEmbedder {
        async fn embed(
            &self,
            text: &str,
        ) -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>> {
            Ok(self.encode(text))
        }

        async fn embed_batch(
            &self,
            texts: &[&str],
        ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error + Send + Sync>> {
            Ok(texts.iter().map(|t| self.encode(t)).collect())
        }

        fn dimensions(&self) -> usize {
            self.keywords.len()
        }

        fn model_id(&self) -> &str {
            "test-keyword-embedder"
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::memory::cosine_similarity;

        #[tokio::test]
        async fn keyword_embedder_identical_keywords_have_cosine_one() {
            let e = KeywordEmbedder::new(&["heatpump"]);
            let a = e.embed("Heatpump dashboard").await.unwrap();
            let b = e.embed("Controls heatpumps via API").await.unwrap();
            assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
        }

        #[tokio::test]
        async fn keyword_embedder_disjoint_keywords_have_zero_similarity() {
            let e = KeywordEmbedder::new(&["heatpump", "oura"]);
            let a = e.embed("Heatpump dashboard").await.unwrap();
            let b = e.embed("Oura ring sleep").await.unwrap();
            assert_eq!(cosine_similarity(&a, &b), 0.0);
        }

        #[tokio::test]
        async fn keyword_embedder_no_match_returns_zero_vector() {
            let e = KeywordEmbedder::new(&["heatpump"]);
            let v = e.embed("totally unrelated text").await.unwrap();
            assert_eq!(v, vec![0.0]);
        }

        #[test]
        #[should_panic(expected = "non-empty")]
        fn keyword_embedder_rejects_empty_keyword() {
            KeywordEmbedder::new(&["heatpump", ""]);
        }
    }
}
