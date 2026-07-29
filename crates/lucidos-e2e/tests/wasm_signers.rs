//! End-to-end tests for the WASM signer layer against real, checked-in
//! signer source. These tests load actual `.wasm` artifacts produced by
//! `./signers/build-all.sh` (compiled from `signers/binance-hmac/` and
//! `signers/test-echo/` with the `wasm32-unknown-unknown` target) and run
//! them through `WasmSignerLayer::apply`.
//!
//! The artifacts are gitignored — they only exist after the build script
//! has run. `./scripts/e2e-wasm.sh` does the build then invokes this test
//! binary. Skipping the build before running means `load_signer` panics
//! with a clear "did you run `./signers/build-all.sh`?" message.

use lucidos_engine::__wasm_test_internals::*;

use bytes::Bytes;
use std::collections::HashMap;
use std::sync::Arc;

/// Walk up from CWD to find `signers/<name>/<name>.wasm`, copy the wasm and
/// manifest into a tempdir, and load the resulting module via
/// `load_wasm_modules`. Both files must exist on disk — i.e.
/// `./signers/build-all.sh <name>` must have been run first.
fn load_signer(name: &str) -> (Arc<Engine>, Arc<CompiledModule>) {
    let wasm_rel = format!("signers/{name}/{name}.wasm");
    let workspace_root = std::env::current_dir()
        .unwrap()
        .ancestors()
        .find(|p| p.join(&wasm_rel).exists())
        .unwrap_or_else(|| {
            panic!(
                "couldn't find {wasm_rel} — did you run `./signers/build-all.sh {name}`?"
            )
        })
        .to_path_buf();

    let tmp = tempfile::tempdir().unwrap();
    std::fs::copy(
        workspace_root.join(&wasm_rel),
        tmp.path().join(format!("{name}.wasm")),
    )
    .unwrap();
    std::fs::copy(
        workspace_root.join(format!("signers/{name}/manifest.json")),
        tmp.path().join(format!("{name}.manifest.json")),
    )
    .unwrap();

    let engine = Arc::new(build_wasmtime_engine().unwrap());
    let modules = load_wasm_modules(tmp.path(), &engine).unwrap();
    let module = modules
        .get(name)
        .unwrap_or_else(|| panic!("loader should pick up {name}.wasm"))
        .clone();
    (engine, module)
}

#[tokio::test]
async fn wasm_signer_layer_runs_binance_hmac_signer() {
    let (engine, module) = load_signer("binance-hmac");

    let mut creds = HashMap::new();
    creds.insert("binance-secret".to_string(), b"test-secret".to_vec());

    let layer = WasmSignerLayer::new(
        "binance-hmac".into(),
        module,
        engine,
        Arc::new(MapResolver(creds)),
        vec![CredentialHandle {
            name: "api_secret".into(),
            credential: "binance-secret".into(),
        }],
        vec![],
    );

    let body = Bytes::new();
    let prior = HashMap::new();
    let input = LayerInput {
        method: &axum::http::Method::GET,
        url: "https://api.binance.com/api/v3/account?recvWindow=5000",
        headers: &[],
        body: BodyView::Raw(&body),
        prior_layer_outputs: &prior,
    };
    let m = layer.apply(&input).await.expect("apply should succeed");
    assert!(m.add_headers.is_empty(), "Binance signer adds no headers");
    assert_eq!(m.add_query.len(), 2);
    assert_eq!(m.add_query[0].0, "timestamp");
    assert!(
        m.add_query[0].1.parse::<u64>().is_ok(),
        "timestamp must be a parseable u64, got {}",
        m.add_query[0].1
    );
    assert_eq!(m.add_query[1].0, "signature");
    assert_eq!(m.add_query[1].1.len(), 64, "sha256 hex digest is 64 chars");
    assert!(
        m.add_query[1].1.chars().all(|c| c.is_ascii_hexdigit()),
        "signature must be lowercase hex, got {}",
        m.add_query[1].1
    );
}

#[tokio::test]
async fn wasm_signer_layer_runs_test_echo_wasm() {
    let (engine, module) = load_signer("test-echo");

    let layer = WasmSignerLayer::new(
        "test-echo".into(),
        module,
        engine,
        Arc::new(MapResolver(HashMap::new())),
        vec![],
        vec![],
    );

    let body = Bytes::new();
    let prior = HashMap::new();
    let input = LayerInput {
        method: &axum::http::Method::GET,
        url: "https://example.com/x?q=1",
        headers: &[],
        body: BodyView::Raw(&body),
        prior_layer_outputs: &prior,
    };
    let m = layer.apply(&input).await.expect("apply should succeed");
    assert_eq!(m.add_headers.len(), 1);
    assert_eq!(m.add_headers[0].0.as_str(), "x-echo");
    assert_eq!(m.add_headers[0].1, "ok");
}
