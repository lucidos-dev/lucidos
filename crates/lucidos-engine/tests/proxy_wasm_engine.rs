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
        Err(err) => assert!(
            err.contains("compile"),
            "expected compile error, got: {err}"
        ),
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

/// Declares far more initial linear memory than the sandbox allows: 1000 pages
/// is ~64 MiB against a 16 MiB ceiling. Instantiation must refuse it, so the
/// host never reserves the memory in the first place.
const OVERSIZED_MEMORY_WAT: &str = r#"
(module
  (memory (export "memory") 1000)
  (func (export "sign") (param i32 i32) (result i64)
    (i64.const 0)))
"#;

/// Grows linear memory in a loop, 1 MiB at a time. The execution budget cannot
/// stop this before the host is out of memory, because epoch interruption caps
/// CPU rather than resident memory. Only the store's memory ceiling can.
const MEMORY_BOMB_WAT: &str = r#"
(module
  (memory (export "memory") 1)
  (func (export "sign") (param i32 i32) (result i64)
    (loop $l
      (drop (memory.grow (i32.const 16)))
      (br $l))
    (i64.const 0)))
"#;

/// Declares two tables against a per-store ceiling of one. The element ceiling
/// is per table, so a module that multiplies tables multiplies the ceiling.
/// wasmtime refuses this itself, which is the object-count half of the sandbox.
const TWO_TABLES_WAT: &str = r#"
(module
  (memory (export "memory") 1)
  (table 1 funcref)
  (table 1 funcref)
  (func (export "sign") (param i32 i32) (result i64)
    (i64.const 0)))
"#;

/// Scans its own input for a `~` (byte 0x7E) and reports whether it found one,
/// via `x-saw: yes` or `x-saw: no`.
///
/// This is how a test asks the SANDBOX what it received, instead of asking the
/// host what it meant to send. In the `SignInput` these tests build, `~` occurs
/// in exactly one place: the upstream header value a signer may read only with
/// the `read_prior_headers` grant.
const TILDE_SCAN_WAT: &str = r#"
(module
  (memory (export "memory") 1)
  (data (i32.const 1024) "{\"add_headers\":[[\"x-saw\",\"yes\"]]}")
  (data (i32.const 1536) "{\"add_headers\":[[\"x-saw\",\"no\"]]}")
  (func (export "sign") (param $ptr i32) (param $len i32) (result i64)
    (local $i i32)
    (local.set $i (i32.const 0))
    (block $done
      (loop $scan
        (br_if $done (i32.ge_u (local.get $i) (local.get $len)))
        (if (i32.eq
              (i32.load8_u (i32.add (local.get $ptr) (local.get $i)))
              (i32.const 126))
          (then (return (i64.or
                          (i64.shl (i64.const 1024) (i64.const 32))
                          (i64.const 33)))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $scan)))
    (i64.or (i64.shl (i64.const 1536) (i64.const 32)) (i64.const 32))))
"#;

fn signer_layer(name: &str, wat_src: &str, engine: &Arc<Engine>) -> WasmSignerLayer {
    WasmSignerLayer::new(
        name.into(),
        compile_wat(name, wat_src, engine),
        engine.clone(),
        Arc::new(MapResolver(HashMap::new())),
        vec![],
        vec![],
    )
}

/// The sandbox kill left the process healthy: a good signer on the SAME engine
/// still signs. An abort or an OOM would have taken this down with it.
async fn assert_engine_still_serves(engine: &Arc<Engine>) {
    let layer = signer_layer("echo", ECHO_WAT, engine);
    let body = bytes::Bytes::new();
    let prior = HashMap::new();
    let input = make_layer_input(&body, &prior);
    let m = layer
        .apply(&input)
        .await
        .expect("the engine must still serve a healthy signer after a sandbox kill");
    assert_eq!(m.add_headers[0].1, "ok");
}

// ── proxy_wasm_signer::tests::wasm_signer_layer_* (Engine-creating) ────
// Real-artifact variants moved to crates/lucidos-e2e/tests/wasm_signers.rs
// (run via ./scripts/e2e-wasm.sh).

#[tokio::test]
async fn wasm_signer_layer_runs_echo_signer_end_to_end() {
    let engine = Arc::new(build_wasmtime_engine().unwrap());
    let layer = signer_layer("echo", ECHO_WAT, &engine);

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
    let layer =
        signer_layer("runaway", LOOP_FOREVER_WAT, &engine).with_budget(Duration::from_millis(150));

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

/// A module declaring an oversized initial memory is refused at instantiation.
/// The 502 names the signer and the limit, and the engine keeps working.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wasm_signer_layer_rejects_a_module_declaring_oversized_memory() {
    let engine = Arc::new(build_wasmtime_engine().unwrap());
    let layer = signer_layer("greedy-init", OVERSIZED_MEMORY_WAT, &engine);

    let body = bytes::Bytes::new();
    let prior = HashMap::new();
    let input = make_layer_input(&body, &prior);
    let err = layer
        .apply(&input)
        .await
        .expect_err("an oversized initial memory must be refused, not instantiated");

    assert_eq!(err.0, axum::http::StatusCode::BAD_GATEWAY);
    assert!(
        err.1.contains("greedy-init") && err.1.contains("linear-memory limit of 16 MiB"),
        "error must name the signer and the memory limit; got: {}",
        err.1
    );

    assert_engine_still_serves(&engine).await;
}

/// A `memory.grow` loop is killed by the memory ceiling, not by the CPU budget.
/// It must die well inside the default budget, and say memory rather than
/// budget, so the two sandbox failure modes stay distinguishable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wasm_signer_layer_kills_a_memory_growing_signer() {
    let engine = Arc::new(build_wasmtime_engine().unwrap());
    let layer = signer_layer("memory-bomb", MEMORY_BOMB_WAT, &engine);

    let body = bytes::Bytes::new();
    let prior = HashMap::new();
    let input = make_layer_input(&body, &prior);

    let started = std::time::Instant::now();
    let outcome = tokio::time::timeout(Duration::from_secs(20), layer.apply(&input)).await;
    let elapsed = started.elapsed();

    let result = outcome.expect("apply() must return: a memory bomb must not hang the task");
    let err = result.expect_err("a signer growing memory without bound must be rejected");
    assert_eq!(err.0, axum::http::StatusCode::BAD_GATEWAY);
    assert!(
        err.1.contains("memory-bomb") && err.1.contains("linear-memory limit"),
        "error must name the signer and the memory limit; got: {}",
        err.1
    );
    assert!(
        !err.1.contains("execution budget"),
        "the memory cap must be the reported cause, not the CPU budget; got: {}",
        err.1
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "the memory cap must trip long before the 5s CPU budget; took {elapsed:?}"
    );

    assert_engine_still_serves(&engine).await;
}

/// An object-count cap reads like every other sandbox refusal: a 502 naming the
/// signer, and an engine that keeps serving. wasmtime states this limit in its
/// own words, so the message is asserted loosely rather than on its exact text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wasm_signer_layer_rejects_a_module_declaring_two_tables() {
    let engine = Arc::new(build_wasmtime_engine().unwrap());
    let layer = signer_layer("table-splitter", TWO_TABLES_WAT, &engine);

    let body = bytes::Bytes::new();
    let prior = HashMap::new();
    let input = make_layer_input(&body, &prior);
    let err = layer
        .apply(&input)
        .await
        .expect_err("a module over the table-count cap must be refused");

    assert_eq!(err.0, axum::http::StatusCode::BAD_GATEWAY);
    assert!(
        err.1.contains("table-splitter") && err.1.contains("limit"),
        "error must name the signer and a limit; got: {}",
        err.1
    );

    assert_engine_still_serves(&engine).await;
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

// ── prior_layer_outputs least privilege ─────────────────────────────────

fn signer_layer_with_caps(
    name: &str,
    wat_src: &str,
    engine: &Arc<Engine>,
    declared: Vec<String>,
    granted: Vec<String>,
) -> WasmSignerLayer {
    let bytes = wat::parse_str(wat_src).unwrap();
    let module = Module::new(engine, &bytes).unwrap();
    let compiled = Arc::new(CompiledModule {
        name: name.into(),
        module,
        manifest: WasmManifest {
            secret_handles: vec![],
            body_mode: BodyMode::Either,
            capabilities: declared,
        },
    });
    WasmSignerLayer::new(
        name.into(),
        compiled,
        engine.clone(),
        Arc::new(MapResolver(HashMap::new())),
        vec![],
        granted,
    )
}

/// An upstream `script_handshake` output whose token carries the `~` the scan
/// module looks for.
fn prior_with_upstream_token() -> HashMap<String, serde_json::Value> {
    let mut prior = HashMap::new();
    prior.insert(
        "script_handshake".to_string(),
        serde_json::json!({ "headers": { "authorization": "Bearer up~stream" } }),
    );
    prior
}

/// Run the scan module against that upstream token and return what it saw.
async fn what_the_sandbox_saw(layer: &WasmSignerLayer) -> String {
    let body = bytes::Bytes::new();
    let prior = prior_with_upstream_token();
    let input = make_layer_input(&body, &prior);
    let m = layer.apply(&input).await.expect("apply should succeed");
    m.add_headers
        .first()
        .map(|(_, v)| v.clone())
        .expect("the scan module always returns one header")
}

#[tokio::test]
async fn a_signer_without_the_grant_never_receives_an_upstream_header_value() {
    let engine = Arc::new(build_wasmtime_engine().unwrap());
    let layer = signer_layer_with_caps("scan", TILDE_SCAN_WAT, &engine, vec![], vec![]);
    assert_eq!(
        what_the_sandbox_saw(&layer).await,
        "no",
        "the module found the upstream token inside its own linear memory"
    );
}

#[tokio::test]
async fn a_signer_with_both_halves_of_the_grant_receives_the_value() {
    // The control for the test above: it proves the scan module can find the
    // needle at all, so a "no" there is withholding rather than a dead scanner.
    let engine = Arc::new(build_wasmtime_engine().unwrap());
    let cap = vec![CAP_READ_PRIOR_HEADERS.to_string()];
    let layer = signer_layer_with_caps("scan", TILDE_SCAN_WAT, &engine, cap.clone(), cap);
    assert_eq!(what_the_sandbox_saw(&layer).await, "yes");
}

#[tokio::test]
async fn a_manifest_declaration_alone_does_not_hand_over_the_value() {
    let engine = Arc::new(build_wasmtime_engine().unwrap());
    let layer = signer_layer_with_caps(
        "scan",
        TILDE_SCAN_WAT,
        &engine,
        vec![CAP_READ_PRIOR_HEADERS.to_string()],
        vec![],
    );
    assert_eq!(
        what_the_sandbox_saw(&layer).await,
        "no",
        "a module must not grant itself a capability by declaring it"
    );
}

#[tokio::test]
async fn a_provider_grant_alone_does_not_hand_over_the_value() {
    let engine = Arc::new(build_wasmtime_engine().unwrap());
    let layer = signer_layer_with_caps(
        "scan",
        TILDE_SCAN_WAT,
        &engine,
        vec![],
        vec![CAP_READ_PRIOR_HEADERS.to_string()],
    );
    assert_eq!(
        what_the_sandbox_saw(&layer).await,
        "no",
        "a blanket provider grant must not reach a module that never asked"
    );
}

#[tokio::test]
async fn wasm_signer_layer_rejects_replace_body_without_capability_grant() {
    let engine = Arc::new(build_wasmtime_engine().unwrap());
    let layer = signer_layer("rb", REPLACE_BODY_WAT, &engine);

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
