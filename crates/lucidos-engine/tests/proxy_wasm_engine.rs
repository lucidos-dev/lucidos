//! Integration tests for the wasmtime-`Engine`-creating cases in
//! `api::proxy_*`. These were lifted out of the lib's `#[cfg(test)] mod
//! tests` blocks because of a macOS-specific abort:
//!
//!   `mach_msg failed with 268451845` (`MACH_RCV_INTERRUPTED`, SIGABRT)
//!
//! Each `wasmtime::Engine::new()` allocates a JIT memory pool via
//! `MAP_JIT` `mmap`, which exchanges Mach messages with the kernel.
//! Many concurrent allocations from many `#[tokio::test]` runtimes
//! (the parallel `cargo test -p lucidos-engine --lib` schedule)
//! crossed a per-process Mach IPC threshold and crashed libdispatch.
//! Splitting the wasmtime-Engine cases into their own `[[test]]`
//! binary gives them a separate process — separate per-process
//! budget — and the abort is gone. The bisection details and
//! everything-we-tried list live atop `engine::change_ops::tests`.
//!
//! Run with:
//!   cargo test -p lucidos-engine --test proxy_wasm_engine
//!
//! Sync `#[test]`s in the same files (no wasmtime allocation) stay in
//! the lib's `mod tests` blocks — only the Engine-creating cases moved.

use lucidos_engine::__wasm_test_internals::*;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use wasmtime::{Engine, Module};

// ── proxy_pipeline_builder helpers ──────────────────────────────────────

fn lazy_pool() -> PgPool {
    PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgres://nobody:nobody@127.0.0.1:1/nobody")
        .expect("connect_lazy never errors on parse-only failure")
}

fn empty_modules() -> HashMap<String, Arc<CompiledModule>> {
    HashMap::new()
}

fn one_module(name: &str, engine: &Engine) -> HashMap<String, Arc<CompiledModule>> {
    let bytes = wat::parse_str("(module)").unwrap();
    let module = Module::new(engine, &bytes).unwrap();
    let mut map = HashMap::new();
    map.insert(
        name.to_string(),
        Arc::new(CompiledModule {
            name: name.into(),
            module,
            manifest: WasmManifest {
                secret_handles: vec![],
                body_mode: BodyMode::Either,
                capabilities: vec![],
            },
        }),
    );
    map
}

fn ctx<'a>(
    proxy_name: &'a str,
    modules: &'a HashMap<String, Arc<CompiledModule>>,
    engine: &Engine,
) -> PipelineBuildContext<'a> {
    PipelineBuildContext {
        pool: lazy_pool(),
        workspace_path: Arc::new(PathBuf::from("/ws")),
        token_cache: Arc::new(ProxyTokenCache::new()),
        proxy_name,
        proxy_modules: modules,
        wasm_engine: Arc::new(engine.clone()),
    }
}

// ── proxy_pipeline_builder::tests::* (Engine-creating) ──────────────────

#[tokio::test]
async fn build_rejects_wasm_signer_when_module_not_loaded() {
    let engine = build_wasmtime_engine().unwrap();
    let modules = empty_modules();
    let cfg = PipelineConfig {
        pipeline: vec![LayerConfig::WasmSigner {
            module: "missing-module".into(),
            credential_handles: vec![],
        }],
        granted_capabilities: vec![],
    };
    let err = match build_pipeline(&cfg, &ctx("p", &modules, &engine)).await {
        Ok(_) => panic!("build should have rejected this config"),
        Err(e) => e,
    };
    assert_eq!(err.0, axum::http::StatusCode::BAD_GATEWAY);
    assert!(err.1.contains("missing-module"));
    assert!(err.1.contains("not loaded"));
}

#[tokio::test]
async fn build_succeeds_when_wasm_module_present() {
    let engine = build_wasmtime_engine().unwrap();
    let modules = one_module("present", &engine);
    let cfg = PipelineConfig {
        pipeline: vec![LayerConfig::WasmSigner {
            module: "present".into(),
            credential_handles: vec![],
        }],
        granted_capabilities: vec![],
    };
    let layers = build_pipeline(&cfg, &ctx("p", &modules, &engine))
        .await
        .unwrap();
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0].output_namespace(), "wasm_signer");
}

#[tokio::test]
async fn build_rejects_query_param_without_param_name() {
    let engine = build_wasmtime_engine().unwrap();
    let modules = empty_modules();
    let cfg = PipelineConfig {
        pipeline: vec![LayerConfig::StaticCredential {
            kind: StaticKind::QueryParam,
            credential: "k".into(),
            header: None,
            param_name: None,
        }],
        granted_capabilities: vec![],
    };
    let err = match build_pipeline(&cfg, &ctx("p", &modules, &engine)).await {
        Ok(_) => panic!("build should have rejected this config"),
        Err(e) => e,
    };
    assert!(
        err.1.contains("param_name") || err.1.contains("credential"),
        "expected shape or lookup error, got: {}",
        err.1
    );
}

// ── proxy_wasm_host::tests (Engine-creating only) ──────────────────────

#[test]
fn register_host_imports_succeeds() {
    let engine = build_wasmtime_engine().unwrap();
    let mut linker: wasmtime::Linker<HostState> = wasmtime::Linker::new(&engine);
    register_host_imports(&mut linker).unwrap();
}

// ── proxy::tests::reload_picks_up_freshly_added_wasm_files ─────────────

#[tokio::test]
async fn reload_picks_up_freshly_added_wasm_files() {
    let engine = build_wasmtime_engine().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let modules_dir = tmp.path().join("data/auth-modules");
    std::fs::create_dir_all(&modules_dir).unwrap();

    let registry = Arc::new(tokio::sync::RwLock::new(
        load_wasm_modules(&modules_dir, &engine).unwrap(),
    ));
    assert!(registry.read().await.is_empty());

    let wasm_bytes = wat::parse_str("(module)").unwrap();
    std::fs::write(modules_dir.join("freshly-added.wasm"), &wasm_bytes).unwrap();

    let new_modules = load_wasm_modules(&modules_dir, &engine).unwrap();
    *registry.write().await = new_modules;
    assert!(registry.read().await.contains_key("freshly-added"));
}

// ── proxy_wasm_signer::tests::loader_* (Engine-creating) ────────────────

#[test]
fn loader_returns_empty_for_missing_dir() {
    let engine = build_wasmtime_engine().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("does-not-exist");
    let modules = load_wasm_modules(&dir, &engine).unwrap();
    assert!(modules.is_empty());
}

#[test]
fn loader_compiles_wasm_files() {
    let engine = build_wasmtime_engine().unwrap();
    let wat = r#"(module)"#;
    let wasm_bytes = wat::parse_str(wat).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("test.wasm"), &wasm_bytes).unwrap();
    let modules = load_wasm_modules(tmp.path(), &engine).unwrap();
    assert!(modules.contains_key("test"));
    let m = modules.get("test").unwrap();
    assert_eq!(m.manifest.body_mode, BodyMode::Either);
    assert!(m.manifest.secret_handles.is_empty());
    assert!(m.manifest.capabilities.is_empty());
}

#[test]
fn loader_skips_non_wasm_files() {
    let engine = build_wasmtime_engine().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("readme.txt"), b"hello").unwrap();
    let modules = load_wasm_modules(tmp.path(), &engine).unwrap();
    assert!(modules.is_empty());
}

#[test]
fn loader_reads_manifest_sidecar() {
    let engine = build_wasmtime_engine().unwrap();
    let wat = r#"(module)"#;
    let wasm_bytes = wat::parse_str(wat).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("custom.wasm"), &wasm_bytes).unwrap();
    std::fs::write(
        tmp.path().join("custom.manifest.json"),
        r#"{"secret_handles":["api_secret"],"body_mode":"hash","capabilities":["replace_body"]}"#,
    )
    .unwrap();
    let modules = load_wasm_modules(tmp.path(), &engine).unwrap();
    let m = modules.get("custom").unwrap();
    assert_eq!(m.manifest.secret_handles, vec!["api_secret".to_string()]);
    assert_eq!(m.manifest.body_mode, BodyMode::Hash);
    assert_eq!(m.manifest.capabilities, vec!["replace_body".to_string()]);
}

#[test]
fn loader_rejects_invalid_wasm() {
    let engine = build_wasmtime_engine().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("garbage.wasm"), b"not wasm at all").unwrap();
    match load_wasm_modules(tmp.path(), &engine) {
        Err(err) => assert!(err.contains("compile"), "expected compile error, got: {err}"),
        Ok(_) => panic!("loader must reject invalid WASM bytes"),
    }
}

// ── proxy_wasm_signer::tests::wasm_signer_layer_* helpers ──────────────

fn compile_wat(name: &str, wat_src: &str, engine: &Engine) -> Arc<CompiledModule> {
    let bytes = wat::parse_str(wat_src).unwrap();
    let module = Module::new(engine, &bytes).unwrap();
    Arc::new(CompiledModule {
        name: name.into(),
        module,
        manifest: WasmManifest {
            secret_handles: vec![],
            body_mode: BodyMode::Either,
            capabilities: vec![],
        },
    })
}

fn make_layer_input<'a>(
    body: &'a bytes::Bytes,
    prior: &'a HashMap<String, serde_json::Value>,
) -> LayerInput<'a> {
    LayerInput {
        method: &axum::http::Method::GET,
        url: "https://example.com/x?q=1",
        headers: &[],
        body: BodyView::Raw(body),
        prior_layer_outputs: prior,
    }
}

const ECHO_WAT: &str = r#"
(module
  (memory (export "memory") 1)
  (data (i32.const 1024) "{\"add_headers\":[[\"x-echo\",\"ok\"]]}")
  (func (export "sign") (param i32 i32) (result i64)
    (i64.or
      (i64.shl (i64.const 1024) (i64.const 32))
      (i64.const 33))))
"#;

const REPLACE_BODY_WAT: &str = r#"
(module
  (memory (export "memory") 1)
  (data (i32.const 1024) "{\"replace_body\":[1,2,3]}")
  (func (export "sign") (param i32 i32) (result i64)
    (i64.or
      (i64.shl (i64.const 1024) (i64.const 32))
      (i64.const 24))))
"#;

/// A deliberately non-terminating signer: `sign` enters an unconditional loop
/// and never returns. The execution budget (epoch interruption) must kill it.
const LOOP_FOREVER_WAT: &str = r#"
(module
  (memory (export "memory") 1)
  (func (export "sign") (param i32 i32) (result i64)
    (loop $l (br $l))
    (i64.const 0)))
"#;

// ── proxy_wasm_signer::tests::wasm_signer_layer_* (Engine-creating) ────
// Real-artifact variants moved to crates/lucidos-e2e/tests/wasm_signers.rs
// (run via ./scripts/e2e-wasm.sh).

#[tokio::test]
async fn wasm_signer_layer_runs_echo_signer_end_to_end() {
    let engine = Arc::new(build_wasmtime_engine().unwrap());
    let module = compile_wat("echo", ECHO_WAT, &engine);
    let resolver = Arc::new(MapResolver(HashMap::new()));

    let layer = WasmSignerLayer::new("echo".into(), module, engine, resolver, vec![], vec![]);

    let body = bytes::Bytes::new();
    let prior = HashMap::new();
    let input = make_layer_input(&body, &prior);
    let m = layer.apply(&input).await.expect("apply should succeed");
    assert_eq!(m.add_headers.len(), 1);
    assert_eq!(m.add_headers[0].0.as_str(), "x-echo");
    assert_eq!(m.add_headers[0].1, "ok");
    assert!(m.add_query.is_empty());
    assert!(m.replace_body.is_none());
}

/// A non-terminating signer is killed by the execution budget rather than
/// pinning the request-execution task forever. The outer `timeout` is a test
/// safety net — if the budget mechanism regressed, the test fails (does not
/// hang). Multi-thread runtime so the epoch ticker thread can advance the epoch
/// while the runaway call occupies a worker. (This is the core fix for the
/// "WASM signer has no execution budget" finding.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wasm_signer_layer_kills_a_runaway_looping_signer() {
    let engine = Arc::new(build_wasmtime_engine().unwrap());
    let module = compile_wat("runaway", LOOP_FOREVER_WAT, &engine);
    let resolver = Arc::new(MapResolver(HashMap::new()));
    let layer = WasmSignerLayer::new("runaway".into(), module, engine, resolver, vec![], vec![])
        .with_budget(Duration::from_millis(150));

    let body = bytes::Bytes::new();
    let prior = HashMap::new();
    let input = make_layer_input(&body, &prior);

    let started = std::time::Instant::now();
    let outcome = tokio::time::timeout(Duration::from_secs(20), layer.apply(&input)).await;
    let elapsed = started.elapsed();

    let result = outcome.expect("apply() must return — a runaway signer must not hang the task");
    let err = result.expect_err("a non-terminating signer must be rejected, not succeed");
    assert_eq!(err.0, axum::http::StatusCode::BAD_GATEWAY);
    assert!(
        err.1.contains("runaway") && err.1.contains("budget"),
        "error must name the signer and the budget; got: {}",
        err.1
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "budget should trip promptly; took {elapsed:?}"
    );
}

#[tokio::test]
async fn wasm_signer_layer_rejects_raw_body_request_when_body_exceeds_1mb() {
    let engine = Arc::new(build_wasmtime_engine().unwrap());
    let bytes = wat::parse_str(ECHO_WAT).unwrap();
    let module = Module::new(&engine, &bytes).unwrap();
    let compiled = Arc::new(CompiledModule {
        name: "raw-only".into(),
        module,
        manifest: WasmManifest {
            secret_handles: vec![],
            body_mode: BodyMode::Raw,
            capabilities: vec![],
        },
    });
    let layer = WasmSignerLayer::new(
        "raw-only".into(),
        compiled,
        engine,
        Arc::new(MapResolver(HashMap::new())),
        vec![],
        vec![],
    );

    let hash = [0xAAu8; 32];
    let prior = HashMap::new();
    let input = LayerInput {
        method: &axum::http::Method::POST,
        url: "https://x",
        headers: &[],
        body: BodyView::HashOnly {
            sha256: &hash,
            length: 2_097_152,
        },
        prior_layer_outputs: &prior,
    };
    let err = layer.apply(&input).await.unwrap_err();
    assert_eq!(err.0, axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    assert!(err.1.contains("requires raw body"));
}

#[tokio::test]
async fn wasm_signer_layer_rejects_replace_body_without_capability_grant() {
    let engine = Arc::new(build_wasmtime_engine().unwrap());
    let module = compile_wat("rb", REPLACE_BODY_WAT, &engine);
    let resolver = Arc::new(MapResolver(HashMap::new()));

    let layer = WasmSignerLayer::new("rb".into(), module, engine, resolver, vec![], vec![]);

    let body = bytes::Bytes::new();
    let prior = HashMap::new();
    let input = make_layer_input(&body, &prior);
    let err = layer.apply(&input).await.unwrap_err();
    assert_eq!(err.0, axum::http::StatusCode::FORBIDDEN);
    assert!(err.1.contains("body replacement"));
}

#[tokio::test]
async fn wasm_signer_layer_accepts_replace_body_with_capability_grant() {
    let engine = build_wasmtime_engine().unwrap();
    let bytes = wat::parse_str(REPLACE_BODY_WAT).unwrap();
    let module = Module::new(&engine, &bytes).unwrap();
    let compiled = Arc::new(CompiledModule {
        name: "rb-allowed".into(),
        module,
        manifest: WasmManifest {
            secret_handles: vec![],
            body_mode: BodyMode::Either,
            capabilities: vec!["replace_body".into()],
        },
    });
    let layer = WasmSignerLayer::new(
        "rb-allowed".into(),
        compiled,
        Arc::new(engine),
        Arc::new(MapResolver(HashMap::new())),
        vec![],
        vec!["replace_body".into()],
    );

    let body = bytes::Bytes::new();
    let prior = HashMap::new();
    let input = make_layer_input(&body, &prior);
    let m = layer.apply(&input).await.expect("apply should succeed");
    let replaced = m.replace_body.expect("replace_body should be set");
    assert_eq!(&replaced[..], &[1u8, 2, 3]);
}
