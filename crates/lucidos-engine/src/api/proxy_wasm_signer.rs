//! `WasmSigner` — per-request `AuthLayer` impl backed by a sandboxed
//! wasmtime module loaded from `data/auth-modules/<name>.wasm`.

use crate::api::proxy_auth_layer::{AuthLayer, AuthMutation, BodyView, LayerInput};
use crate::api::proxy_wasm_host::{register_host_imports, HostState};
use async_trait::async_trait;
use axum::http::{HeaderName, StatusCode};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
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

/// Build the wasmtime engine with our standard config: async, fuel disabled
/// (we cap signers via timeout instead), cranelift JIT.
pub fn build_wasmtime_engine() -> Result<Engine, String> {
    let mut config = Config::new();
    config.async_support(true);
    Engine::new(&config).map_err(|e| format!("wasmtime engine init: {e}"))
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
        let module = Module::new(engine, &bytes)
            .map_err(|e| format!("compile {}: {e}", path.display()))?;
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
    Raw {
        bytes: serde_bytes::ByteBuf,
    },
    HashOnly {
        sha256_hex: String,
        length: u64,
    },
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
        }
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

    async fn apply(
        &self,
        input: &LayerInput<'_>,
    ) -> Result<AuthMutation, (StatusCode, String)> {
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
                sha256_hex: hex_string(sha256),
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
            prior_layer_outputs: serde_json::to_value(input.prior_layer_outputs).map_err(
                |e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("serialize prior_layer_outputs: {e}"),
                    )
                },
            )?,
            secret_handles: handles,
            current_time_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        };
        let in_bytes = serde_json::to_vec(&sign_input).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("serialize SignInput: {e}"),
            )
        })?;

        // 3. Spin up a fresh wasmtime Store + Linker for this call.
        let host_state = HostState {
            secrets,
            module_name: self.module.name.clone(),
        };
        let mut store: Store<HostState> = Store::new(&self.engine, host_state);
        let mut linker: Linker<HostState> = Linker::new(&self.engine);
        register_host_imports(&mut linker)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("linker init: {e}")))?;
        let instance = linker
            .instantiate_async(&mut store, &self.module.module)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("instantiate {}: {e}", self.module.name),
                )
            })?;

        // 4. Find module memory + sign export. `alloc` is optional.
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| {
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
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("alloc({}) failed: {e}", in_len),
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

        // 6. Call sign and decode the packed (out_ptr, out_len).
        let packed = sign
            .call_async(&mut store, (in_ptr, in_len))
            .await
            .map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    format!("signer {} sign() failed: {e}", self.module.name),
                )
            })?;
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

fn hex_string(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
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
