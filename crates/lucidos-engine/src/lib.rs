//! Lucidos Engine - the core runtime behind a workspace
//!
//! This crate provides the core functionality for Lucidos including:
//! - Event sourcing with PostgreSQL
//! - Artifact management with Git
//! - Memory indexing with vector search
//! - LLM integration with tool calling
//! - Python runtime execution

/// Timestamped log with auto-derived module label.
/// - `log!("message")` → auto module from `module_path!()`
/// - `log!(@Push, "message")` → explicit label override
#[macro_export]
macro_rules! log {
    (@$label:ident, $($arg:tt)*) => {
        println!("{} [pid:{}] [{}] {}",
            chrono::Local::now().format("%H:%M:%S"),
            std::process::id(),
            stringify!($label),
            format_args!($($arg)*))
    };
    ($($arg:tt)*) => {
        println!("{} [pid:{}] [{}] {}",
            chrono::Local::now().format("%H:%M:%S"),
            std::process::id(),
            $crate::module_label(module_path!()),
            format_args!($($arg)*))
    };
}

pub fn module_label(path: &str) -> &str {
    let segment = match path.rfind("::") {
        Some(i) => &path[i + 2..],
        None => path,
    };
    match segment {
        "api" => "API",
        "engine" => "Engine",
        "pinned_apps" => "Apps",
        "store" => "Store",
        "events" => "Events",
        "artifacts" => "Artifacts",
        "credentials" => "Credentials",
        "devices" => "Devices",
        "preferences" => "Preferences",
        "changes" => "Changes",
        "scheduler" | "persistence" | "tasks" | "user_tasks" | "config" | "triggers"
        | "condition" => "Scheduler",
        "push" | "notifications" => "Push",
        "memory" | "extractor" | "pgvector" | "fastembed" | "provider" => "Memory",
        "vertex" | "openai" => "LLM",
        "mcp" | "mcp_servers" | "client" => "MCP",
        "browser" | "browser_consent" => "Browser",
        "chat" | "agentic_loop" => "Engine",
        "context" | "document" | "types" => "Engine",
        "claude_code" => "Engine",
        "files" | "http" | "import" | "python" | "web" => "Engine",
        "email" => "Engine",
        "populate_memory" => "Populate",
        "lucidos_engine" => "Engine",
        _ => segment,
    }
}

pub mod api;
pub mod boot_failure;
pub mod boot_report;
pub mod capability_manifest;
pub mod core;
pub mod engine;
pub mod gateway_auth;
pub mod llm;
pub mod mcp;
pub mod memory;
#[cfg(test)]
mod migration_tests;
pub mod net_config;
pub mod paths;
pub mod runtime;
pub mod scheduler;
#[cfg(test)]
mod test_support;
pub mod triggers;

pub use engine::LucidosEngine;

/// Hidden re-exports of crate-internal items exercised by the wasmtime
/// integration tests in `tests/proxy_wasm_engine.rs`.
///
/// **Why this exists.** Those tests originally lived in `#[cfg(test)]`
/// `mod tests` blocks inside `api/proxy_wasm_*.rs` and friends. Each
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
    pub use crate::api::proxy_auth_layer::{AuthLayer, AuthMutation, BodyView, LayerInput};
    pub use crate::api::proxy_pipeline_builder::{build_pipeline, PipelineBuildContext};
    pub use crate::api::proxy_pipeline_config::{
        CredentialHandleConfig, LayerConfig, PipelineConfig, StaticKind,
    };
    pub use crate::api::proxy_token_cache::ProxyTokenCache;
    pub use crate::api::proxy_wasm_host::{register_host_imports, HostState};
    pub use crate::api::proxy_wasm_signer::{
        build_wasmtime_engine, load_wasm_modules, BodyMode, CompiledModule, CredentialHandle,
        SecretResolver, SignInput, SignInputBody, SignOutput, WasmManifest, WasmSignerLayer,
        CAP_READ_PRIOR_HEADERS, CAP_REPLACE_BODY, WITHHELD_HEADER_VALUE,
    };
    pub use wasmtime::Engine;

    use std::collections::HashMap;

    /// In-memory `SecretResolver` for tests. Returns the mapped bytes for a
    /// known credential id, or a descriptive error otherwise. Shared by
    /// `crates/lucidos-engine/tests/proxy_wasm_engine.rs` and
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

/// Lucidos umbrella release version (e.g. "0.7"), distinct from per-crate
/// Cargo.toml semvers. Sourced from the repo-root `RELEASE` file at compile
/// time. `trim_ascii_end()` strips the trailing newline at const time.
pub const LUCIDOS_RELEASE: &str = {
    let raw = include_str!("../../../RELEASE").as_bytes();
    let trimmed = raw.trim_ascii_end();
    // SAFETY: input is ASCII (digits + '.' + whitespace); trim_ascii_end keeps
    // it valid UTF-8.
    match std::str::from_utf8(trimmed) {
        Ok(s) => s,
        Err(_) => panic!("RELEASE file must be valid UTF-8"),
    }
};

/// True when this build was made from a commit *after* the one that last
/// bumped RELEASE — i.e. the running code is past the published v<RELEASE>
/// snapshot. Set by build.rs from `git log -- RELEASE`. Defaults to false
/// when git is unavailable (shipped install, missing history).
pub const LUCIDOS_RELEASE_DIRTY: bool = match option_env!("LUCIDOS_RELEASE_DIRTY") {
    // `==` on `&str` isn't const-stable, so byte-compare the literal "true".
    Some(s) => {
        let b = s.as_bytes();
        b.len() == 4 && b[0] == b't' && b[1] == b'r' && b[2] == b'u' && b[3] == b'e'
    }
    None => false,
};

/// Deterministic-per-source id of this engine binary, baked by `build.rs` (git
/// short SHA + a hash of any uncommitted engine-source diff; a source-tree hash
/// when git is unavailable). Mirrors the gateway's `GATEWAY_BUILD_ID`. A running
/// engine compares this against the on-disk binary's `--build-id` to tell whether
/// a newer version has been built — the dev "new version available" signal (see
/// `docs/plans/2026-07-01-new-engine-version-switch-flow.md`). `option_env!` so a
/// build that somehow skipped the stamp still compiles (empty → never "newer").
pub const ENGINE_BUILD_ID: &str = match option_env!("ENGINE_BUILD_ID") {
    Some(s) => s,
    None => "",
};

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
                eprintln!(
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
