//! `WasmSigner` — per-request `AuthLayer` impl backed by a sandboxed
//! wasmtime module loaded from `data/auth-modules/<name>.wasm`.

use crate::api::hex::hex_lower;
use crate::api::proxy_auth_layer::{AuthLayer, AuthMutation, BodyView, LayerInput};
use crate::api::proxy_wasm_host::{register_host_imports, HostState};
use async_trait::async_trait;
use axum::http::{HeaderName, StatusCode};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use wasmtime::{
    Config, Engine, Linker, Module, ResourceLimiter, Store, StoreLimits, StoreLimitsBuilder,
};

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

/// Capability to read the auth-header VALUES an earlier layer published into
/// `prior_layer_outputs`.
///
/// Signer modules arrive from plugins and from workspace data, so a signer is
/// third-party code by default. Without this grant it receives every header
/// NAME an upstream layer produced, and [`WITHHELD_HEADER_VALUE`] in place of
/// each value. A `script_handshake` access token then never enters the
/// sandbox. Same two-sided gate as [`CAP_REPLACE_BODY`].
pub const CAP_READ_PRIOR_HEADERS: &str = "read_prior_headers";

/// What an ungranted signer reads where an upstream header value would be.
///
/// A placeholder rather than a removed key, so a signer author can still see
/// that the header was present. It names the capability, so the fix is in the
/// value itself rather than in a doc the author has to find.
pub const WITHHELD_HEADER_VALUE: &str = "[withheld: grant read_prior_headers]";

/// Default wall-clock execution budget for a single signer invocation
/// (instantiate + `alloc` + `sign`). Real signers do a handful of crypto
/// primitives and finish in microseconds; 5s is enormous headroom that only a
/// runaway / non-terminating module would ever hit. Enforced via wasmtime
/// epoch interruption (see [`build_wasmtime_engine`] + [`WasmSignerLayer`]).
pub const WASM_SIGNER_BUDGET: Duration = Duration::from_secs(5);

/// Maximum linear memory one signer invocation may occupy.
///
/// The layer hands a module at most a 1MB raw body. So 16 MiB is over ten
/// times the largest legitimate input. It is far more than an HMAC signer
/// touches, and still well below the host's own headroom.
///
/// Enforced per `Store` by [`SignerLimits`]. The execution budget cannot do
/// this: epoch interruption bounds CPU, not resident memory.
pub const WASM_SIGNER_MAX_MEMORY: usize = 16 * 1024 * 1024;

/// Maximum funcref table elements. A signer compiled from Rust carries tens of
/// indirect-call entries, so this is ample headroom. It caps a `table.grow`
/// loop the way the memory ceiling caps `memory.grow`.
const WASM_SIGNER_MAX_TABLE_ELEMENTS: usize = 10_000;

/// Maximum instances per `Store`. The linker exposes host functions only, so
/// one signer call instantiates exactly one module.
const WASM_SIGNER_MAX_INSTANCES: usize = 1;

/// Maximum linear memories per `Store`. [`WASM_SIGNER_MAX_MEMORY`] is per
/// memory, so without this a multi-memory module would multiply the ceiling.
const WASM_SIGNER_MAX_MEMORIES: usize = 1;

/// Maximum tables per `Store`. Same reason as the memory count: the element
/// ceiling is per table.
const WASM_SIGNER_MAX_TABLES: usize = 1;

/// Which sandbox cap a signer hit. Recorded by [`SignerLimits`] so the failure
/// can name the limit without matching on wasmtime's error text.
///
/// Only the two growth caps appear here. wasmtime reads the object counts and
/// refuses instantiation itself, with no callback to record. Those trip the
/// instantiate error path and quote wasmtime's own wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignerCap {
    Memory,
    TableElements,
}

impl SignerCap {
    /// The limit as an error message states it, sized from the constant above.
    fn describe(self) -> String {
        match self {
            SignerCap::Memory => format!(
                "linear-memory limit of {} MiB",
                WASM_SIGNER_MAX_MEMORY / (1024 * 1024)
            ),
            SignerCap::TableElements => {
                format!("table-element limit of {WASM_SIGNER_MAX_TABLE_ELEMENTS}")
            }
        }
    }
}

/// Per-`Store` resource limiter for one signer invocation.
///
/// Epoch interruption caps CPU, not resident memory. A module that declares a
/// huge initial memory, or loops on `memory.grow`, exhausts the host inside
/// its budget. This wraps wasmtime's `StoreLimits` with the signer ceilings.
/// It records which one tripped, so the 502 can name it.
///
/// A grow our cap refuses becomes a trap, not the wasm `-1`. A hostile loop
/// then dies at once instead of spinning out its budget. A grow refused ONLY by
/// the module's own declared maximum keeps the standard `-1`. That is the
/// module's business, not the sandbox's.
pub struct SignerLimits {
    inner: StoreLimits,
    tripped: Option<SignerCap>,
}

impl SignerLimits {
    pub fn new() -> Self {
        Self {
            inner: StoreLimitsBuilder::new()
                .memory_size(WASM_SIGNER_MAX_MEMORY)
                .table_elements(WASM_SIGNER_MAX_TABLE_ELEMENTS)
                .instances(WASM_SIGNER_MAX_INSTANCES)
                .memories(WASM_SIGNER_MAX_MEMORIES)
                .tables(WASM_SIGNER_MAX_TABLES)
                .build(),
            tripped: None,
        }
    }

    /// The cap this invocation hit, if any.
    pub fn tripped(&self) -> Option<SignerCap> {
        self.tripped
    }
}

impl Default for SignerLimits {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceLimiter for SignerLimits {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> Result<bool, wasmtime::Error> {
        let allowed = self.inner.memory_growing(current, desired, maximum)?;
        if !allowed && desired > WASM_SIGNER_MAX_MEMORY {
            self.tripped = Some(SignerCap::Memory);
            return Err(wasmtime::Error::msg(format!(
                "linear memory growth to {desired} bytes exceeds the signer sandbox limit"
            )));
        }
        Ok(allowed)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> Result<bool, wasmtime::Error> {
        let allowed = self.inner.table_growing(current, desired, maximum)?;
        if !allowed && desired > WASM_SIGNER_MAX_TABLE_ELEMENTS {
            self.tripped = Some(SignerCap::TableElements);
            return Err(wasmtime::Error::msg(format!(
                "table growth to {desired} elements exceeds the signer sandbox limit"
            )));
        }
        Ok(allowed)
    }

    fn instances(&self) -> usize {
        self.inner.instances()
    }

    fn tables(&self) -> usize {
        self.inner.tables()
    }

    fn memories(&self) -> usize {
        self.inner.memories()
    }
}

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

/// Map a wasmtime call error to a `(StatusCode, message)`.
///
/// Two sandbox faults read alike, and both give `BAD_GATEWAY`, because a
/// runaway signer is a module fault. One is a resource cap recorded by
/// [`SignerLimits`]. The other is an epoch-deadline trip, arriving as
/// `Trap::Interrupt`. The cap is checked first, since it is the more specific
/// fact. Any other error keeps `stage`'s default status and embeds the wasm
/// message.
fn sandbox_or(
    signer: &str,
    stage: &str,
    default_status: StatusCode,
    tripped: Option<SignerCap>,
    e: wasmtime::Error,
) -> (StatusCode, String) {
    if let Some(cap) = tripped {
        return (
            StatusCode::BAD_GATEWAY,
            format!(
                "signer {signer} exceeded its {} during {stage} and was terminated",
                cap.describe()
            ),
        );
    }
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

/// True iff both halves of a capability grant are present: the module's own
/// manifest declares it, AND the provider config grants it to this pipeline.
/// Either half alone is not a grant.
fn capability_granted(cap: &str, declared: &[String], granted: &[String]) -> bool {
    declared.iter().any(|c| c == cap) && granted.iter().any(|c| c == cap)
}

/// Prepare the two things one invocation derives from the prior layers' output:
/// what the module is HANDED, and what its `log()` may never print.
///
/// Returns the outputs to serialize into `SignInput`, plus the substrings to
/// scrub. When `grant_headers` is false every `headers` value is replaced by
/// [`WITHHELD_HEADER_VALUE`]. The scrub list is built from the ORIGINAL values
/// either way, so a granted signer still cannot echo a token into the log.
///
/// It also carries the resolved secret material in the encodings a module might
/// emit (utf8, hex, base64). Secrets reach a module only by opaque handle, so
/// that half is defense in depth.
fn prepare_prior_outputs(
    mut prior: serde_json::Value,
    secrets: &[Vec<u8>],
    grant_headers: bool,
) -> (serde_json::Value, Vec<String>) {
    use base64::Engine as _;
    let mut redactions: Vec<String> = Vec::new();
    for s in secrets {
        if let Ok(utf8) = std::str::from_utf8(s) {
            redactions.push(utf8.to_string());
        }
        redactions.push(hex_lower(s));
        redactions.push(base64::engine::general_purpose::STANDARD.encode(s));
    }
    take_header_values(&mut prior, &mut redactions, !grant_headers);
    (prior, redactions)
}

/// Walk a JSON value, push the string values of every `headers` object into
/// `out`, and overwrite each with [`WITHHELD_HEADER_VALUE`] when `withhold`.
///
/// Auth layers publish their produced headers under a `headers` key
/// (`ScriptHandshakeLayer::apply` → `outputs: {"headers": {…}}`), so this
/// reaches exactly the token values a downstream signer would receive. Scoping
/// it to that key is deliberate: a heuristic over every string in the prior
/// outputs would withhold legitimate signer input.
fn take_header_values(value: &mut serde_json::Value, out: &mut Vec<String>, withhold: bool) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if key == "headers" {
                    if let Some(headers) = v.as_object_mut() {
                        for hv in headers.values_mut() {
                            let Some(s) = hv.as_str() else { continue };
                            out.push(s.to_string());
                            if withhold {
                                *hv = serde_json::Value::String(WITHHELD_HEADER_VALUE.to_string());
                            }
                        }
                    }
                }
                take_header_values(v, out, withhold);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                take_header_values(item, out, withhold);
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

    /// Whether this signer holds `cap`. See [`capability_granted`]: the
    /// manifest must declare it and the provider config must grant it.
    fn has_capability(&self, cap: &str) -> bool {
        capability_granted(
            cap,
            &self.module.manifest.capabilities,
            &self.granted_capabilities,
        )
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
        // Least privilege on what the signer is HANDED, not only on what it may
        // print. An earlier layer's produced auth headers travel in
        // prior_layer_outputs, so a signer with no `read_prior_headers` grant
        // gets the header names and a placeholder value. The scrub list is
        // built from the real values regardless (see `prepare_prior_outputs`).
        let prior = serde_json::to_value(input.prior_layer_outputs).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("serialize prior_layer_outputs: {e}"),
            )
        })?;
        let (prior_layer_outputs, log_redactions) =
            prepare_prior_outputs(prior, &secrets, self.has_capability(CAP_READ_PRIOR_HEADERS));

        let sign_input = SignInput {
            method: input.method.as_str().to_string(),
            url: input.url.to_string(),
            headers: input
                .headers
                .iter()
                .map(|(n, v)| (n.as_str().to_string(), v.clone()))
                .collect(),
            body: body_field,
            prior_layer_outputs,
            secret_handles: handles,
            current_time_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        };
        let in_bytes = serde_json::to_vec(&sign_input).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("serialize SignInput: {e}"),
            )
        })?;

        // 3. Spin up a fresh wasmtime Store + Linker for this call. The scrub
        //    list came from the real prior outputs above, so `log()` stays
        //    closed to upstream auth material even for a granted signer.
        let host_state = HostState {
            secrets,
            module_name: self.module.name.clone(),
            log_redactions,
            limits: SignerLimits::new(),
        };
        let mut store: Store<HostState> = Store::new(&self.engine, host_state);
        // Enforce the resource ceilings: memory, table elements and object
        // counts. Attached BEFORE instantiate so a module declaring an oversized
        // initial memory is refused at instantiation, not after the host has
        // already reserved it. See `SignerLimits`.
        store.limiter(|s| &mut s.limits);
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
        // Bound the awaited result before mapping the error: `map_err` reads
        // `store.data()`, which the in-flight call still borrows mutably.
        let instantiated = linker
            .instantiate_async(&mut store, &self.module.module)
            .await;
        // A failure here is the module's, not the engine's: the linker is
        // already up, so what is left is the module's own shape. An object-count
        // cap is refused by wasmtime itself and records no `SignerCap`, so this
        // default is what makes every cap trip a 502.
        let instance = instantiated.map_err(|e| {
            sandbox_or(
                &self.module.name,
                "instantiate",
                StatusCode::BAD_GATEWAY,
                store.data().limits.tripped(),
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
            let allocated = alloc.call_async(&mut store, in_len).await;
            allocated.map_err(|e| {
                sandbox_or(
                    &self.module.name,
                    "alloc",
                    StatusCode::INTERNAL_SERVER_ERROR,
                    store.data().limits.tripped(),
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
        let outcome = tokio::time::timeout(self.budget + Duration::from_secs(2), sign_call).await;
        let packed = match outcome {
            Ok(Ok(packed)) => packed,
            Ok(Err(e)) => {
                return Err(sandbox_or(
                    &self.module.name,
                    "sign",
                    StatusCode::BAD_GATEWAY,
                    store.data().limits.tripped(),
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
        //
        // Bounds-check the module-supplied (ptr, len) against its OWN linear
        // memory BEFORE allocating. `out_len` is 32 bits of whatever the module
        // returned, so a signer that signals an error the C way (`return -1`)
        // packs `0xFFFF_FFFF` into it and the host would try to zero a 4 GiB
        // buffer: an OOM abort of the whole engine, driven from inside the
        // sandbox. Any slice a real module returns lies inside its memory (the
        // `memory.read` below would fail otherwise), so this rejects nothing
        // that used to work; it only turns the OOM into a 502.
        let mem_size = memory.data_size(&store);
        if out_ptr
            .checked_add(out_len)
            .is_none_or(|end| end > mem_size)
        {
            return Err((
                StatusCode::BAD_GATEWAY,
                format!(
                    "signer {} returned an out-of-bounds SignOutput slice at {out_ptr}/{out_len} \
                     (module memory is {mem_size} bytes)",
                    self.module.name
                ),
            ));
        }
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
        if output.replace_body.is_some() && !self.has_capability(CAP_REPLACE_BODY) {
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

    // ---- prior_layer_outputs least privilege ---------------------------

    fn handshake_prior() -> serde_json::Value {
        serde_json::json!({
            "script_handshake": {
                "headers": {
                    "authorization": "Bearer upstream-token-xyz",
                    "x-client-id": "client-abc-123"
                }
            },
            "other": { "log_url": "https://example.com/x?k=REDACTED" }
        })
    }

    #[test]
    fn redactions_cover_secrets_and_prior_header_tokens() {
        let secrets = vec![b"super-secret-key".to_vec()];
        let (_, red) = prepare_prior_outputs(handshake_prior(), &secrets, true);

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
    fn ungranted_signer_gets_header_names_but_not_values() {
        let (prior, _) = prepare_prior_outputs(handshake_prior(), &[], false);
        let headers = prior["script_handshake"]["headers"].as_object().unwrap();

        // The NAME survives, so an author can see the header was present.
        assert!(headers.contains_key("authorization"));
        assert!(headers.contains_key("x-client-id"));
        // The value is the placeholder, which names the missing capability.
        assert_eq!(headers["authorization"], WITHHELD_HEADER_VALUE);
        assert_eq!(headers["x-client-id"], WITHHELD_HEADER_VALUE);
        // Nothing outside a `headers` object is touched.
        assert_eq!(
            prior["other"]["log_url"],
            "https://example.com/x?k=REDACTED"
        );
    }

    #[test]
    fn withholding_still_scrubs_the_real_value_from_logs() {
        // The scrub list is built BEFORE withholding, so a value the module
        // never received still cannot be printed by some other route.
        let (_, red) = prepare_prior_outputs(handshake_prior(), &[], false);
        assert!(red.iter().any(|s| s == "Bearer upstream-token-xyz"));
    }

    #[test]
    fn granted_signer_receives_the_real_header_value() {
        let (prior, _) = prepare_prior_outputs(handshake_prior(), &[], true);
        assert_eq!(
            prior["script_handshake"]["headers"]["authorization"],
            "Bearer upstream-token-xyz"
        );
    }

    #[test]
    fn withholding_reaches_a_nested_headers_object() {
        let nested = serde_json::json!({
            "outer": [{ "inner": { "headers": { "authorization": "Bearer deep" } } }]
        });
        let (prior, red) = prepare_prior_outputs(nested, &[], false);
        assert_eq!(
            prior["outer"][0]["inner"]["headers"]["authorization"],
            WITHHELD_HEADER_VALUE
        );
        assert!(red.iter().any(|s| s == "Bearer deep"));
    }

    #[test]
    fn the_withheld_input_is_what_gets_serialized_to_the_module() {
        let (prior_layer_outputs, _) = prepare_prior_outputs(handshake_prior(), &[], false);
        let input = SignInput {
            method: "GET".into(),
            url: "https://x".into(),
            headers: vec![],
            body: SignInputBody::Raw {
                bytes: serde_bytes::ByteBuf::new(),
            },
            prior_layer_outputs,
            secret_handles: std::collections::HashMap::new(),
            current_time_ns: 0,
        };
        let wire = serde_json::to_string(&input).unwrap();
        assert!(!wire.contains("upstream-token-xyz"), "wire was: {wire}");
        assert!(wire.contains("authorization"), "wire was: {wire}");
        assert!(wire.contains("read_prior_headers"), "wire was: {wire}");
    }

    #[test]
    fn a_capability_needs_the_manifest_and_the_grant() {
        let cap = CAP_READ_PRIOR_HEADERS;
        let declared = vec![cap.to_string()];
        let granted = vec![cap.to_string()];
        let none: Vec<String> = vec![];

        assert!(capability_granted(cap, &declared, &granted));
        assert!(
            !capability_granted(cap, &declared, &none),
            "a manifest declaration alone must not grant the capability"
        );
        assert!(
            !capability_granted(cap, &none, &granted),
            "a provider grant alone must not reach a module that never declared it"
        );
        assert!(!capability_granted(cap, &none, &none));
        // A grant of one capability says nothing about another.
        assert!(!capability_granted(CAP_REPLACE_BODY, &declared, &granted));
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
