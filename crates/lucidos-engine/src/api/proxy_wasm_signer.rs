//! `WasmSigner` — per-request `AuthLayer` impl backed by a sandboxed
//! wasmtime module loaded from `data/auth-modules/<name>.wasm`.

use crate::api::proxy_auth_layer::{AuthLayer, AuthMutation, BodyView, LayerInput};
use crate::api::proxy_hex::hex_lower;
use crate::api::proxy_wasm_host::{register_host_imports, HostState};
use async_trait::async_trait;
use axum::http::{HeaderName, StatusCode};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use wasmtime::{Config, Engine, Linker, Module, Store};

pub struct CompiledModule {
    pub name: String,
    pub module: Module,
    pub manifest: WasmManifest,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct WasmManifest {
    /// Names of secrets the signer expects to be passed by handle. Order
    /// matches `secret_handles` in `SignInput`.
    #[serde(default)]
    pub secret_handles: Vec<String>,
    /// What the signer wants to see in `SignInput.body`.
    #[serde(default)]
    pub body_mode: BodyMode,
    /// Capabilities the signer wants. Empty = sign-only with opaque handles.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Manifest declaration of how a signer wants the request body presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BodyMode {
    /// Signer requires full body bytes (refuses requests >1MB).
    Raw,
    /// Signer doesn't need raw bytes — host hashes even when raw would
    /// fit (privacy / sandbox).
    Hash,
    /// Signer accepts whatever the pipeline hands it.
    #[default]
    Either,
}

/// Capability strings recognized by the WASM signer pipeline. Manifests
/// declare these in `WasmManifest.capabilities`; per-provider configs
/// grant them in `PipelineConfig.granted_capabilities`. Both must contain
/// the capability for the corresponding feature to engage.
pub const CAP_REPLACE_BODY: &str = "replace_body";

/// Default wall-clock execution budget for a single signer invocation
/// (instantiate + `alloc` + `sign`). Real signers do a handful of crypto
/// primitives and finish in microseconds; 5s is enormous headroom that only a
/// runaway / non-terminating module would ever hit. Enforced via wasmtime
/// epoch interruption (see [`build_wasmtime_engine`] + [`WasmSignerLayer`]).
pub const WASM_SIGNER_BUDGET: Duration = Duration::from_secs(5);

/// How often the engine's epoch counter is advanced by the background ticker
/// (see [`build_wasmtime_engine`]). The execution budget is rounded up to a
/// whole number of these ticks, so this is also the budget's resolution.
const EPOCH_TICK: Duration = Duration::from_millis(100);

/// Build the wasmtime engine with our standard config: async, cranelift JIT,
/// and **epoch-based interruption** so a runaway signer is killed by a real
/// execution budget rather than pinning the request-execution task forever.
///
/// A `tokio::time::timeout` alone cannot stop a CPU-bound WASM loop: with no
/// fuel/epoch the module's `call_async` future never yields, so the timeout
/// future is never polled. Epoch interruption is the mechanism that actually
/// interrupts a tight loop — wasmtime checks the engine's epoch counter at loop
/// back-edges / function entries and traps once the store's deadline is passed.
///
/// The engine is a process singleton (built once at startup, shared by the
/// startup module compile and every per-request `Store`), so we spawn ONE
/// detached OS thread here that advances the epoch every [`EPOCH_TICK`]. A
/// dedicated thread (not a tokio task) keeps interruption working even when all
/// tokio workers are busy / blocked on the runaway call. Per-call deadlines are
/// set on the `Store` in [`WasmSignerLayer::apply`].
pub fn build_wasmtime_engine() -> Result<Engine, String> {
    let mut config = Config::new();
    config.async_support(true);
    config.epoch_interruption(true);
    let engine = Engine::new(&config).map_err(|e| format!("wasmtime engine init: {e}"))?;

    // Detached epoch ticker. Holds an `Engine` clone (cheap Arc-backed handle);
    // since the engine lives for the process there is one such thread per
    // process. (In the test binaries each `build_wasmtime_engine()` spawns its
    // own short-lived ticker — harmless for a test process.)
    let ticker = engine.clone();
    std::thread::Builder::new()
        .name("wasm-epoch-ticker".to_string())
        .spawn(move || loop {
            std::thread::sleep(EPOCH_TICK);
            ticker.increment_epoch();
        })
        .map_err(|e| format!("wasm epoch ticker spawn: {e}"))?;

    Ok(engine)
}

/// Number of epoch ticks that make up `budget` (at least 1, so even a
/// sub-tick budget still traps at the next increment).
fn epoch_deadline_ticks(budget: Duration) -> u64 {
    let ticks = budget.as_millis().div_ceil(EPOCH_TICK.as_millis());
    (ticks as u64).max(1)
}

/// Map a wasmtime call error to an `(StatusCode, message)`. An epoch-deadline
/// trip surfaces as `Trap::Interrupt`; turn that into a clear budget message
/// (always `BAD_GATEWAY` — a runaway signer is an upstream/module fault).
/// Any other error keeps `stage`'s default status and embeds the wasm message.
fn budget_or(
    signer: &str,
    stage: &str,
    default_status: StatusCode,
    e: wasmtime::Error,
) -> (StatusCode, String) {
    if let Some(wasmtime::Trap::Interrupt) = e.downcast_ref::<wasmtime::Trap>() {
        return (
            StatusCode::BAD_GATEWAY,
            format!(
                "signer {signer} exceeded its execution budget during {stage} and was terminated"
            ),
        );
    }
    (
        default_status,
        format!("signer {signer} {stage} failed: {e}"),
    )
}

/// Build the set of sensitive substrings to scrub from a signer's `log()`
/// output: the resolved secret material in the encodings a module might emit
/// (utf8, hex, base64) plus every auth-header/token value passed in via
/// `prior_layer_outputs` (any `headers` object — the documented leak vector,
/// e.g. an upstream `script_handshake` token the signer can read).
fn collect_log_redactions(secrets: &[Vec<u8>], prior: &serde_json::Value) -> Vec<String> {
    use base64::Engine as _;
    let mut out: Vec<String> = Vec::new();
    for s in secrets {
        if let Ok(utf8) = std::str::from_utf8(s) {
            out.push(utf8.to_string());
        }
        out.push(hex_lower(s));
        out.push(base64::engine::general_purpose::STANDARD.encode(s));
    }
    collect_header_values(prior, &mut out);
    out
}

/// Walk a JSON value and push the string values of every `headers` object into
/// `out`. Auth layers publish their produced headers under a `headers` key
/// (`ScriptHandshakeLayer::apply` → `outputs: {"headers": {…}}`), so this
/// captures the token values a downstream signer receives without redacting
/// every unrelated string in the prior outputs.
fn collect_header_values(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, v) in map {
                if key == "headers" {
                    if let Some(headers) = v.as_object() {
                        for hv in headers.values() {
                            if let Some(s) = hv.as_str() {
                                out.push(s.to_string());
                            }
                        }
                    }
                }
                collect_header_values(v, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_header_values(item, out);
            }
        }
        _ => {}
    }
}

/// Scan `dir` for `*.wasm` files, compile each, return them keyed by basename.
/// Each `.wasm` is paired with an optional `<name>.manifest.json` sidecar; if
/// missing, defaults are applied (no capabilities, body_mode = "either", no
/// secret handles declared).
///
/// The caller passes the shared `Engine` (built via `build_wasmtime_engine()`).
/// wasmtime interns `FuncType` identity per-engine, so modules MUST be compiled
/// against the same engine that `WasmSignerLayer::apply` later uses to build
/// the `Linker`/`Store`; otherwise instantiation panics with "id from different
/// slab" / errors with "incompatible import type".
pub fn load_wasm_modules(
    dir: &Path,
    engine: &Engine,
) -> Result<HashMap<String, Arc<CompiledModule>>, String> {
    let mut out = HashMap::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("wasm") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        if stem.is_empty() {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let module =
            Module::new(engine, &bytes).map_err(|e| format!("compile {}: {e}", path.display()))?;
        let manifest_path = path.with_file_name(format!("{stem}.manifest.json"));
        let manifest = if manifest_path.exists() {
            let m = std::fs::read_to_string(&manifest_path)
                .map_err(|e| format!("read manifest {}: {e}", manifest_path.display()))?;
            serde_json::from_str(&m)
                .map_err(|e| format!("parse manifest {}: {e}", manifest_path.display()))?
        } else {
            WasmManifest {
                secret_handles: vec![],
                body_mode: BodyMode::default(),
                capabilities: vec![],
            }
        };
        out.insert(
            stem.clone(),
            Arc::new(CompiledModule {
                name: stem,
                module,
                manifest,
            }),
        );
    }
    Ok(out)
}

// ---- WASM ABI types ----------------------------------------------------
//
// Wire format between the host and a signer module. JSON over linear
// memory. The module exports `sign(in_ptr, in_len) -> i64` where the high
// 32 bits of the return are `out_ptr` and the low 32 are `out_len`. The
// host serializes `SignInput` to JSON, copies it into module memory at an
// agreed offset (or via an exported `alloc(len)` if present), calls
// `sign`, and parses `SignOutput` from the returned slice.

#[derive(Debug, Clone, serde::Serialize)]
pub struct SignInput {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// Either raw bytes (≤1MB) or sha256 of the body.
    pub body: SignInputBody,
    pub prior_layer_outputs: serde_json::Value,
    /// Maps logical name → opaque handle. Modules look up by name (declared
    /// in their manifest); host vouches for the handles being valid for this
    /// invocation only.
    pub secret_handles: std::collections::HashMap<String, u32>,
    pub current_time_ns: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignInputBody {
    Raw { bytes: serde_bytes::ByteBuf },
    HashOnly { sha256_hex: String, length: u64 },
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SignOutput {
    #[serde(default)]
    pub add_headers: Vec<(String, String)>,
    #[serde(default)]
    pub add_query: Vec<(String, String)>,
    /// Only honored if module manifest declared `replace_body` capability AND
    /// user granted explicit consent for this provider.
    #[serde(default)]
    pub replace_body: Option<serde_bytes::ByteBuf>,
}

// ---- Secret resolution + WasmSignerLayer -------------------------------

/// Looks up secret bytes by credential id at signer-call time. Production
/// uses the credential store; tests use a `HashMap`-backed impl. The trait
/// keeps `WasmSignerLayer` testable without a live PgPool.
#[async_trait]
pub trait SecretResolver: Send + Sync {
    async fn resolve(&self, credential_id: &str) -> Result<Vec<u8>, String>;
}

/// (logical_name, credential_id) — the layer fetches `credential_id` from
/// the resolver and exposes it to the WASM module under `logical_name`
/// (which the module's manifest declares in `secret_handles`).
#[derive(Debug, Clone)]
pub struct CredentialHandle {
    pub name: String,
    pub credential: String,
}

/// Per-request WASM signer. Each `apply()` call instantiates a fresh
/// `Store` so secret state cannot leak across callers.
pub struct WasmSignerLayer {
    namespace: String,
    module: Arc<CompiledModule>,
    engine: Arc<Engine>,
    resolver: Arc<dyn SecretResolver>,
    credential_handles: Vec<CredentialHandle>,
    /// Per-provider capability grants (e.g. `["replace_body"]`). Combined
    /// with the manifest's declared capabilities to gate escape hatches.
    granted_capabilities: Vec<String>,
    /// Wall-clock execution budget for one `apply()` (instantiate + alloc +
    /// sign). Enforced via the engine's epoch interruption. Defaults to
    /// [`WASM_SIGNER_BUDGET`]; tests inject a short budget to exercise the trip.
    budget: Duration,
}

impl WasmSignerLayer {
    pub fn new(
        namespace: String,
        module: Arc<CompiledModule>,
        engine: Arc<Engine>,
        resolver: Arc<dyn SecretResolver>,
        credential_handles: Vec<CredentialHandle>,
        granted_capabilities: Vec<String>,
    ) -> Self {
        Self {
            namespace,
            module,
            engine,
            resolver,
            credential_handles,
            granted_capabilities,
            budget: WASM_SIGNER_BUDGET,
        }
    }

    /// Override the execution budget (test seam — production uses the default).
    pub fn with_budget(mut self, budget: Duration) -> Self {
        self.budget = budget;
        self
    }
}

/// Fixed write offset for `SignInput` JSON when the module doesn't export
/// an `alloc` function. Modules that need more flexibility should export
/// `alloc(i32) -> i32`.
const FIXED_INPUT_OFFSET: i32 = 8192;

/// Owned variant of `BodyView` used inside `WasmSignerLayer::apply` so the
/// downgrade path (`body_mode = "hash"` for a small raw body) can produce
/// a fresh hash that lives for the duration of the call. The pipeline's
/// `BodyView` borrows from upstream-owned bytes/hash buffers; this owned
/// variant lets the layer do its own per-call ownership.
enum BodyViewOwned {
    Raw(bytes::Bytes),
    HashOnly { sha256: [u8; 32], length: u64 },
}

#[async_trait]
impl AuthLayer for WasmSignerLayer {
    fn output_namespace(&self) -> &str {
        &self.namespace
    }

    async fn apply(&self, input: &LayerInput<'_>) -> Result<AuthMutation, (StatusCode, String)> {
        // 0. Manifest-declared body_mode validation.
        //   - Raw    → fail with 413 if the pipeline handed us a hash
        //              (body was over the 1MB threshold).
        //   - Hash   → downgrade to hash even when raw would fit (signer
        //              opted out of seeing body bytes for privacy).
        //   - Either → no constraint.
        let body_view = match (self.module.manifest.body_mode, input.body) {
            (BodyMode::Raw, BodyView::HashOnly { length, .. }) => {
                return Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!(
                        "signer {} requires raw body but request size {} exceeds the 1MB threshold",
                        self.module.name, length
                    ),
                ));
            }
            (BodyMode::Hash, BodyView::Raw(bytes)) => {
                let h = crate::api::proxy_pipeline::body_hash(bytes);
                BodyViewOwned::HashOnly {
                    sha256: h,
                    length: bytes.len() as u64,
                }
            }
            (_, BodyView::Raw(bytes)) => BodyViewOwned::Raw(bytes.clone()),
            (_, BodyView::HashOnly { sha256, length }) => BodyViewOwned::HashOnly {
                sha256: *sha256,
                length,
            },
        };

        // 1. Resolve secrets per the layer's `credential_handles`. The order
        //    of insertion into the table determines each handle's index.
        let mut secrets: Vec<Vec<u8>> = Vec::new();
        let mut handles: HashMap<String, u32> = HashMap::new();
        for h in &self.credential_handles {
            let bytes = self
                .resolver
                .resolve(&h.credential)
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
            handles.insert(h.name.clone(), secrets.len() as u32);
            secrets.push(bytes);
        }

        // 2. Build the wire-format SignInput from the (possibly downgraded)
        //    body view.
        let body_field = match &body_view {
            BodyViewOwned::Raw(bytes) => SignInputBody::Raw {
                bytes: serde_bytes::ByteBuf::from(bytes.to_vec()),
            },
            BodyViewOwned::HashOnly { sha256, length } => SignInputBody::HashOnly {
                sha256_hex: hex_lower(sha256),
                length: *length,
            },
        };
        let sign_input = SignInput {
            method: input.method.as_str().to_string(),
            url: input.url.to_string(),
            headers: input
                .headers
                .iter()
                .map(|(n, v)| (n.as_str().to_string(), v.clone()))
                .collect(),
            body: body_field,
            prior_layer_outputs: serde_json::to_value(input.prior_layer_outputs).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("serialize prior_layer_outputs: {e}"),
                )
            })?,
            secret_handles: handles,
            current_time_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        };
        let in_bytes = serde_json::to_vec(&sign_input).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("serialize SignInput: {e}"),
            )
        })?;

        // 3. Spin up a fresh wasmtime Store + Linker for this call. Collect the
        //    values that must never reach the log: the resolved secret material
        //    (defense in depth — opaque handles mean the module can't see raw
        //    secrets today) plus any auth-header/token values handed to this
        //    signer via prior_layer_outputs (which it CAN see and could echo).
        let log_redactions = collect_log_redactions(&secrets, &sign_input.prior_layer_outputs);
        let host_state = HostState {
            secrets,
            module_name: self.module.name.clone(),
            log_redactions,
        };
        let mut store: Store<HostState> = Store::new(&self.engine, host_state);
        // Enforce the execution budget: the engine's epoch ticker advances every
        // EPOCH_TICK, and the store traps once `budget` worth of ticks elapse.
        // Set BEFORE instantiate so instantiate + alloc + sign are all bounded —
        // this is what kills a runaway/non-terminating signer instead of letting
        // it pin this task forever.
        store.set_epoch_deadline(epoch_deadline_ticks(self.budget));
        store.epoch_deadline_trap();
        let mut linker: Linker<HostState> = Linker::new(&self.engine);
        register_host_imports(&mut linker).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("linker init: {e}"),
            )
        })?;
        let instance = linker
            .instantiate_async(&mut store, &self.module.module)
            .await
            .map_err(|e| {
                budget_or(
                    &self.module.name,
                    "instantiate",
                    StatusCode::INTERNAL_SERVER_ERROR,
                    e,
                )
            })?;

        // 4. Find module memory + sign export. `alloc` is optional.
        let memory = instance.get_memory(&mut store, "memory").ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("module {} did not export `memory`", self.module.name),
            )
        })?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, "alloc")
            .ok();
        let sign = instance
            .get_typed_func::<(i32, i32), i64>(&mut store, "sign")
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!(
                        "module {} did not export `sign(i32, i32) -> i64`: {e}",
                        self.module.name
                    ),
                )
            })?;

        // 5. Reserve space for input bytes and write them.
        let in_len = in_bytes.len() as i32;
        let in_ptr = if let Some(alloc) = &alloc {
            alloc.call_async(&mut store, in_len).await.map_err(|e| {
                budget_or(
                    &self.module.name,
                    "alloc",
                    StatusCode::INTERNAL_SERVER_ERROR,
                    e,
                )
            })?
        } else {
            FIXED_INPUT_OFFSET
        };
        memory
            .write(&mut store, in_ptr as usize, &in_bytes)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("write SignInput to module memory: {e}"),
                )
            })?;

        // 6. Call sign and decode the packed (out_ptr, out_len). The epoch
        //    deadline (set on the store above) is the real budget — it traps a
        //    runaway loop. The `tokio::time::timeout` is a secondary bound for a
        //    hang that DOES yield (e.g. a future async host import); it cannot
        //    by itself interrupt a tight CPU loop, since that never yields back
        //    to be polled. Grace > one epoch tick so the epoch trap wins first.
        let sign_call = sign.call_async(&mut store, (in_ptr, in_len));
        let packed =
            match tokio::time::timeout(self.budget + Duration::from_secs(2), sign_call).await {
                Ok(Ok(packed)) => packed,
                Ok(Err(e)) => {
                    return Err(budget_or(
                        &self.module.name,
                        "sign",
                        StatusCode::BAD_GATEWAY,
                        e,
                    ))
                }
                Err(_elapsed) => {
                    return Err((
                        StatusCode::BAD_GATEWAY,
                        format!(
                            "signer {} exceeded its execution budget and was terminated",
                            self.module.name
                        ),
                    ))
                }
            };
        let out_ptr = ((packed >> 32) & 0xFFFF_FFFF) as usize;
        let out_len = (packed & 0xFFFF_FFFF) as usize;

        // 7. Read output and parse SignOutput.
        let mut out_buf = vec![0u8; out_len];
        memory.read(&store, out_ptr, &mut out_buf).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("read SignOutput from module memory at {out_ptr}/{out_len}: {e}"),
            )
        })?;
        let output: SignOutput = serde_json::from_slice(&out_buf).map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("parse SignOutput from {}: {e}", self.module.name),
            )
        })?;

        // 8. Capability gate: replace_body needs both manifest + grant.
        if output.replace_body.is_some()
            && !(self
                .module
                .manifest
                .capabilities
                .iter()
                .any(|c| c == CAP_REPLACE_BODY)
                && self
                    .granted_capabilities
                    .iter()
                    .any(|c| c == CAP_REPLACE_BODY))
        {
            return Err((
                StatusCode::FORBIDDEN,
                format!(
                    "signer {} requested body replacement without capability grant",
                    self.module.name
                ),
            ));
        }

        // 9. Build AuthMutation.
        let mut add_headers = Vec::with_capacity(output.add_headers.len());
        for (name, value) in output.add_headers {
            let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    format!(
                        "signer {} returned invalid header name '{name}': {e}",
                        self.module.name
                    ),
                )
            })?;
            add_headers.push((header_name, value));
        }
        // Signers are stateless — cache_was_hit defaults to false.
        Ok(AuthMutation {
            add_headers,
            add_query: output.add_query,
            replace_body: output
                .replace_body
                .map(|b| bytes::Bytes::from(b.into_vec())),
            outputs: serde_json::json!({}),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ABI roundtrip tests -----------------------------------------

    #[test]
    fn sign_input_raw_body_serializes_with_tagged_enum() {
        let input = SignInput {
            method: "GET".into(),
            url: "https://api.example.com/path?q=1".into(),
            headers: vec![("X-Test".into(), "1".into())],
            body: SignInputBody::Raw {
                bytes: serde_bytes::ByteBuf::from(b"hello".to_vec()),
            },
            prior_layer_outputs: serde_json::json!({}),
            secret_handles: std::collections::HashMap::new(),
            current_time_ns: 1_700_000_000_000_000_000,
        };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["method"], "GET");
        assert_eq!(json["body"]["type"], "raw");
        // serde_bytes serializes Vec<u8> as a sequence of byte ints in JSON.
        assert!(json["body"]["bytes"].is_array());
    }

    #[test]
    fn sign_input_hash_only_body_serializes_with_tagged_enum() {
        let input = SignInput {
            method: "POST".into(),
            url: "https://api.example.com/upload".into(),
            headers: vec![],
            body: SignInputBody::HashOnly {
                sha256_hex: "ba78".into(),
                length: 5_242_880,
            },
            prior_layer_outputs: serde_json::json!({}),
            secret_handles: std::collections::HashMap::new(),
            current_time_ns: 0,
        };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["body"]["type"], "hash_only");
        assert_eq!(json["body"]["sha256_hex"], "ba78");
        assert_eq!(json["body"]["length"], 5_242_880);
    }

    #[test]
    fn sign_output_parses_minimal_response_with_only_headers() {
        // A signer that only emits headers omits the optional fields.
        let json = serde_json::json!({
            "add_headers": [["x-signed", "abcdef"]],
        });
        let out: SignOutput = serde_json::from_value(json).unwrap();
        assert_eq!(out.add_headers.len(), 1);
        assert_eq!(out.add_headers[0], ("x-signed".into(), "abcdef".into()));
        assert!(out.add_query.is_empty());
        assert!(out.replace_body.is_none());
    }

    #[test]
    fn sign_output_parses_full_response_with_all_fields() {
        let json = serde_json::json!({
            "add_headers": [["x-key", "k"]],
            "add_query": [["timestamp", "1700000000000"], ["signature", "deadbeef"]],
            "replace_body": [104, 105], // "hi"
        });
        let out: SignOutput = serde_json::from_value(json).unwrap();
        assert_eq!(out.add_query.len(), 2);
        let body = out.replace_body.unwrap();
        assert_eq!(&body[..], b"hi");
    }

    // Wasmtime-`Engine`-creating tests for this module live in
    // `crates/lucidos-engine/tests/proxy_wasm_engine.rs`. See the
    // diagnostic block atop `engine::change_ops::tests` for why.

    #[test]
    fn collect_log_redactions_covers_secrets_and_prior_header_tokens() {
        let secrets = vec![b"super-secret-key".to_vec()];
        let prior = serde_json::json!({
            "script_handshake": {
                "headers": {
                    "authorization": "Bearer upstream-token-xyz",
                    "x-client-id": "client-abc-123"
                }
            },
            "other": { "log_url": "https://example.com/x?k=REDACTED" }
        });
        let red = collect_log_redactions(&secrets, &prior);

        // Raw secret in the encodings a module might emit.
        assert!(red.iter().any(|s| s == "super-secret-key"));
        assert!(red.iter().any(|s| s == &hex_lower(b"super-secret-key")));
        // Auth tokens passed in via prior outputs' `headers`.
        assert!(red.iter().any(|s| s == "Bearer upstream-token-xyz"));
        assert!(red.iter().any(|s| s == "client-abc-123"));

        // End-to-end: a signer log line carrying the token is scrubbed.
        let line = "debug: forwarding Bearer upstream-token-xyz to upstream";
        let scrubbed = crate::core::redact_secret_values(line, &red);
        assert!(!scrubbed.contains("upstream-token-xyz"), "got: {scrubbed}");
        assert!(scrubbed.contains("[REDACTED]"), "got: {scrubbed}");
    }

    #[test]
    fn sign_input_secret_handles_serialize_as_object() {
        let mut handles = std::collections::HashMap::new();
        handles.insert("api_secret".to_string(), 0u32);
        handles.insert("api_key".to_string(), 1u32);
        let input = SignInput {
            method: "GET".into(),
            url: "https://x".into(),
            headers: vec![],
            body: SignInputBody::Raw {
                bytes: serde_bytes::ByteBuf::new(),
            },
            prior_layer_outputs: serde_json::json!({}),
            secret_handles: handles,
            current_time_ns: 0,
        };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["secret_handles"]["api_secret"], 0);
        assert_eq!(json["secret_handles"]["api_key"], 1);
    }
}
