//! Workspace orchestration and the compatibility facade for engine libraries.

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
pub use lucidos_core::engine_runtime;
#[cfg(any(test, feature = "test-support"))]
mod engine_test_support;
pub use lucidos_core::gateway_auth;
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

/// The `--wasm-selftest` entry point, reachable from the `lucidos-engine`
/// binary. Re-exported as an item because its module is `pub(crate)`.
pub use engine::proxy_wasm_signer::jit_selftest;

/// Hidden re-exports of crate-internal items exercised by the wasmtime
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

    /// In-memory `SecretResolver` for tests. Returns the mapped bytes for a
    /// known credential id, or a descriptive error otherwise. Shared by
    /// `crates/lucidos-agents-api/tests/proxy_wasm_engine.rs` and
    /// `crates/lucidos-e2e/tests/wasm_signers.rs` so both speak the same
    /// mock semantics.
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

/// Identity of the running executable, captured once before engine startup.
pub struct BuildIdentity {
    /// Git short SHA, with a dirty diff hash when the engine source differs.
    pub id: &'static str,
    /// Umbrella Lucidos release from the repository's RELEASE file.
    pub release: &'static str,
    /// Whether the build's commit is newer than the last RELEASE bump.
    pub release_dirty: bool,
    /// CalVer generated in the engine library's VERSION file.
    pub engine_version: &'static str,
}

static BUILD_IDENTITY: std::sync::OnceLock<BuildIdentity> = std::sync::OnceLock::new();

impl BuildIdentity {
    /// Called by the engine executable before handling CLI probes or serving requests.
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

/// Read the identity installed by the executable at startup.
pub fn build_identity() -> &'static BuildIdentity {
    if let Some(identity) = BUILD_IDENTITY.get() {
        return identity;
    }
    // Release-dependent unit tests get the repository RELEASE, compiled into
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

    /// Cached outcome of initializing the shared real embedder. Stored in a
    /// `OnceLock` so a cold-cache HuggingFace fetch failure costs a single
    /// timeout, not one per gated test.
    #[cfg(feature = "real-embedder-tests")]
    enum SharedEmbedder {
        Ready(crate::memory::FastEmbedProvider),
        /// The model/tokenizer files couldn't be fetched from huggingface.co
        /// (host unreachable, request timed out). The real-embedder tests skip
        /// on this rather than fail — a third-party network blip must never red
        /// the e2e suite. Carries the error string for the skip log.
        FetchUnavailable(String),
    }

    /// Only a model-download / network failure is allowed to skip a
    /// real-embedder test — the same classifier that lets engine construction
    /// boot degraded on a cold-cache offline first run. Single source of
    /// truth: `memory::fastembed::is_model_fetch_failure`.
    #[cfg(feature = "real-embedder-tests")]
    use crate::memory::fastembed::is_model_fetch_failure;

    /// Shared FastEmbedProvider across all real-embedder test modules.
    ///
    /// Returns `Some(provider)` when the model is available — either freshly
    /// downloaded or served from the warm `.fastembed_cache` (a cache hit makes
    /// `hf-hub`'s `ApiRepo::get` short-circuit before any network call, so a
    /// seeded cache runs fully offline). Every caller's real assertions then run
    /// unchanged.
    ///
    /// Returns `None` (and logs a `SKIP`) only when the model files can't be
    /// fetched from huggingface.co. This is the resilience guarantee: a
    /// transient HF outage degrades these tests to *skipped*, never *failed*.
    /// A non-fetch init failure (corrupt model, bad config) still panics — that
    /// is a real bug, not a network blip.
    ///
    /// Loads ~465 MB of model + tokenizer files on a cold cache, so it's gated
    /// behind the `real-embedder-tests` Cargo feature; default `cargo test`
    /// never touches it. The `OnceLock` also avoids lock-file contention when
    /// multiple gated tests initialize the model concurrently.
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

    /// Deterministic test embedder. Returns an L2-normalized one-hot-style
    /// vector indicating which configured keywords appear (case-insensitive
    /// substring match) in the input. Cosine similarity reflects keyword
    /// overlap: identical keyword sets → 1.0, disjoint sets → 0.0, no
    /// keywords on one side → 0/NaN (filtered by the discovery threshold).
    ///
    /// Use for tests that exercise wiring or pure ranking logic without
    /// depending on the network or the real fastembed model. For tests that
    /// verify properties of the actual MultilingualE5Small model (cross-lingual
    /// similarity, etc.), use `shared_embedder()` and gate the test behind
    /// `#[cfg(feature = "real-embedder-tests")]`.
    pub struct KeywordEmbedder {
        keywords: Vec<String>,
    }

    impl KeywordEmbedder {
        pub fn new(keywords: &[&str]) -> Self {
            // Empty keyword would match every text via `String::contains("")`,
            // silently turning that dimension into a constant 1 and
            // poisoning cosine similarity for any test using it.
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
