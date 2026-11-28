use crate::core::hex::hex_lower;
use crate::engine::proxy_wasm_host::{register_host_imports, HostState, LogBudget};
use async_trait::async_trait;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use lucidos_core::engine::proxy_auth_layer::{AuthLayer, AuthMutation, BodyView, LayerInput};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use wasmtime::{
    Config, Engine, Linker, Module, ResourceLimiter, Store, StoreLimits, StoreLimitsBuilder,
    UpdateDeadline,
};

pub struct CompiledModule {
    pub name: String,
    pub module: Module,
    pub manifest: WasmManifest,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct WasmManifest {
    /// INVARIANT: Names of secrets the signer expects to be passed by handle. Order
    /// matches `secret_handles` in `SignInput`.
    #[serde(default)]
    pub secret_handles: Vec<String>,
    #[serde(default)]
    pub body_mode: BodyMode,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BodyMode {
    Raw,
    Hash,
    #[default]
    Either,
}

pub const CAP_REPLACE_BODY: &str = "replace_body";

/// INVARIANT: Capability to read the auth-header VALUES an earlier layer published into
/// `prior_layer_outputs`.
///
/// Signer modules arrive from plugins and from workspace data, so a signer is
/// third-party code by default. Without this grant it receives every header
/// NAME an upstream layer produced, and [`WITHHELD_HEADER_VALUE`] in place of
/// each value. A `script_handshake` access token then never enters the
/// sandbox. Same two-sided gate as [`CAP_REPLACE_BODY`].
pub const CAP_READ_PRIOR_HEADERS: &str = "read_prior_headers";

pub const WITHHELD_HEADER_VALUE: &str = "[withheld: grant read_prior_headers]";

pub const WASM_SIGNER_BUDGET: Duration = Duration::from_secs(5);

const SIGNER_SLOT_WAIT: Duration = Duration::from_secs(1);

/// INVARIANT: Engine-wide cap on signer calls running at once.
///
/// A running signer holds a tokio worker between epoch yields. Without a cap,
/// enough runaway signers could occupy every worker at the same time. Then
/// the UI, SSE, the agent and triggers would stall until the budgets ran out.
/// The cap keeps most workers free for everything else.
///
/// Sized from the runtime the first signer layer is built on, which is the
/// engine's own. `TOKIO_WORKER_THREADS` can make that differ from the core count.
static SIGNER_SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| {
    let workers = tokio::runtime::Handle::try_current().map_or_else(
        |_| std::thread::available_parallelism().map_or(1, NonZeroUsize::get),
        |runtime| runtime.metrics().num_workers(),
    );
    Arc::new(Semaphore::new(signer_slots_for(workers)))
});

fn signer_slots_for(workers: usize) -> usize {
    (workers / 4).max(1)
}

/// INVARIANT: Maximum linear memory one signer invocation may occupy.
///
/// The layer hands a module at most a 1MB raw body. So 16 MiB is over ten
/// times the largest legitimate input. It is far more than an HMAC signer
/// touches, and still well below the host's own headroom.
///
/// Enforced per `Store` by [`SignerLimits`]. The execution budget cannot do
/// this: epoch interruption bounds CPU, not resident memory.
pub const WASM_SIGNER_MAX_MEMORY: usize = 16 * 1024 * 1024;

const WASM_SIGNER_MAX_TABLE_ELEMENTS: usize = 10_000;

const WASM_SIGNER_MAX_INSTANCES: usize = 1;

/// INVARIANT: Maximum linear memories per `Store`. [`WASM_SIGNER_MAX_MEMORY`] is per
/// memory, so without this a multi-memory module would multiply the ceiling.
const WASM_SIGNER_MAX_MEMORIES: usize = 1;

/// INVARIANT: Maximum tables per `Store`. Same reason as the memory count: the element
/// ceiling is per table.
const WASM_SIGNER_MAX_TABLES: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignerCap {
    Memory,
    TableElements,
}

impl SignerCap {
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

/// INVARIANT: Per-`Store` resource limiter for one signer invocation.
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

const EPOCH_TICK: Duration = Duration::from_millis(100);

/// INVARIANT: Build the wasmtime engine with our standard config: async, cranelift JIT,
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
    // INVARIANT: On macOS, wasmtime's mach-port handler thread aborts the whole process
    // when a signal interrupts its wait. Unix signal handlers have no such path.
    config.macos_use_mach_ports(false);
    let engine = Engine::new(&config).map_err(|e| format!("wasmtime engine init: {e}"))?;

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

const SELFTEST_ANSWER: i32 = 42;

const SELFTEST_MODULE: &[u8] = &[
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f, 0x03,
    0x02, 0x01, 0x00, 0x07, 0x0c, 0x01, 0x08, 0x73, 0x65, 0x6c, 0x66, 0x74, 0x65, 0x73, 0x74, 0x00,
    0x00, 0x0a, 0x06, 0x01, 0x04, 0x00, 0x41, 0x2a, 0x0b,
];

const SELFTEST_BUDGET: Duration = Duration::from_secs(30);

/// INVARIANT: Compile and run [`SELFTEST_MODULE`], proving this binary may execute code it
/// compiled at runtime.
///
/// **This is a packaging gate, not a health check.** On macOS the hardened
/// runtime kills a process that executes a page no signature covers, and
/// wasmtime does exactly that. So a signed engine missing
/// `com.apple.security.cs.allow-unsigned-executable-memory` dies with SIGKILL on
/// its first proxy signer call, and it dies the same way on every retry. That
/// shipped in 2026.08.28.0.
///
/// The build runs this against the SIGNED binary, which is the whole point: an
/// entitlement readback proves what the plist asked for, and this proves the
/// kernel agreed. Reuses [`build_wasmtime_engine`] so the config it clears is
/// the config the signer uses, never a simpler one that could pass alone.
///
/// Needs no database, no workspace and no network, so it can run inside a
/// signing step. See
/// `docs/plans/2026-08-29-the-packaged-engine-is-allowed-to-jit.md`.
pub async fn jit_selftest() -> Result<(), String> {
    let engine = build_wasmtime_engine()?;
    let module =
        Module::new(&engine, SELFTEST_MODULE).map_err(|e| format!("compile selftest: {e}"))?;
    let mut store: Store<()> = Store::new(&engine, ());
    store.set_epoch_deadline(epoch_deadline_ticks(SELFTEST_BUDGET));
    store.epoch_deadline_trap();
    let linker: Linker<()> = Linker::new(&engine);
    let instance = linker
        .instantiate_async(&mut store, &module)
        .await
        .map_err(|e| format!("instantiate selftest: {e}"))?;
    let func = instance
        .get_typed_func::<(), i32>(&mut store, "selftest")
        .map_err(|e| format!("selftest export missing: {e}"))?;
    // INVARIANT: The call is what the hardened runtime kills. Nothing before it touches a
    // page the kernel objects to, so a binary that reaches here still fails.
    let got = func
        .call_async(&mut store, ())
        .await
        .map_err(|e| format!("call selftest: {e}"))?;
    if got != SELFTEST_ANSWER {
        return Err(format!(
            "selftest returned {got}, expected {SELFTEST_ANSWER}"
        ));
    }
    Ok(())
}

fn epoch_deadline_ticks(budget: Duration) -> u64 {
    let ticks = budget.as_millis().div_ceil(EPOCH_TICK.as_millis());
    (ticks as u64).max(1)
}

/// INVARIANT: Epoch-deadline callback for one signer call: yield the thread at every
/// tick, and trap once `deadline` has passed.
///
/// Every host import is synchronous, so a looping module never yields by
/// itself. In plain trap mode it holds its tokio worker for the whole budget,
/// and no timeout around the call can fire. Yielding gives the worker back each
/// tick. The wall-clock deadline keeps the total budget, however long the
/// yields wait to resume.
///
/// The trap is the same `Trap::Interrupt` as trap mode, so [`sandbox_or`] still
/// reports it with the fixed "exceeded its execution budget" body. Tokio's own
/// `yield_now` lets the worker drive timers and IO before it resumes the module.
fn yield_until<T>(
    deadline: Instant,
) -> impl FnMut(wasmtime::StoreContextMut<T>) -> wasmtime::Result<UpdateDeadline> + Send + Sync + 'static
{
    move |_| {
        if Instant::now() >= deadline {
            return Err(wasmtime::Trap::Interrupt.into());
        }
        Ok(UpdateDeadline::YieldCustom(
            1,
            Box::pin(tokio::task::yield_now()),
        ))
    }
}

/// INVARIANT: Map a wasmtime call error to a `(StatusCode, message)`.
///
/// Two sandbox faults read alike, and both give `BAD_GATEWAY`, because a
/// runaway signer is a module fault. One is a resource cap recorded by
/// [`SignerLimits`]. The other is an epoch-deadline trip, arriving as
/// `Trap::Interrupt`. The cap is checked first, since it is the more specific
/// fact. Any other error keeps `stage`'s default status, and its wasm message
/// goes to the log only: a trap backtrace carries a code offset the module chose.
fn sandbox_or(
    signer: &str,
    stage: &str,
    default_status: StatusCode,
    tripped: Option<SignerCap>,
    redactions: &[String],
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
    signer_fault(
        default_status,
        signer,
        &format!("{stage} failed"),
        &format!("{e:?}"),
        redactions,
    )
}

/// INVARIANT: Refuse a signer call with a body that names only the signer and `what`.
///
/// `detail` carries what the module chose: its output, a trap site, a slice it
/// packed. That can encode a token it holds or a signature over data the app
/// chose, and redaction cannot catch the second. So the detail goes to the
/// engine log, scrubbed, and never into the body the app reads.
fn signer_fault(
    status: StatusCode,
    signer: &str,
    what: &str,
    detail: &str,
    redactions: &[String],
) -> (StatusCode, String) {
    let detail = crate::core::redact_secret_values(detail, redactions);
    crate::log_legacy!("[wasm-signer:{signer}] {what}: {detail}");
    (status, format!("signer {signer} {what}"))
}

fn capability_granted(cap: &str, declared: &[String], granted: &[String]) -> bool {
    declared.iter().any(|c| c == cap) && granted.iter().any(|c| c == cap)
}

/// INVARIANT: Prepare the two things one invocation derives from the prior layers' output:
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

/// INVARIANT: Walk a JSON value, push the string values of every `headers` object into
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct SignInput {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: SignInputBody,
    pub prior_layer_outputs: serde_json::Value,
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
    /// INVARIANT: Only honored if module manifest declared `replace_body` capability AND
    /// user granted explicit consent for this provider.
    #[serde(default)]
    pub replace_body: Option<serde_bytes::ByteBuf>,
}

#[async_trait]
pub trait SecretResolver: Send + Sync {
    async fn resolve(&self, credential_id: &str) -> Result<Vec<u8>, String>;
}

#[derive(Debug, Clone)]
pub struct CredentialHandle {
    pub name: String,
    pub credential: String,
}

/// INVARIANT: Per-request WASM signer. Each `apply()` call instantiates a fresh
/// `Store` so secret state cannot leak across callers.
pub struct WasmSignerLayer {
    namespace: String,
    module: Arc<CompiledModule>,
    engine: Arc<Engine>,
    resolver: Arc<dyn SecretResolver>,
    credential_handles: Vec<CredentialHandle>,
    granted_capabilities: Vec<String>,
    budget: Duration,
    slots: Arc<Semaphore>,
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
            slots: SIGNER_SLOTS.clone(),
        }
    }

    pub fn with_budget(mut self, budget: Duration) -> Self {
        self.budget = budget;
        self
    }

    pub fn with_slots(mut self, slots: Arc<Semaphore>) -> Self {
        self.slots = slots;
        self
    }

    fn has_capability(&self, cap: &str) -> bool {
        capability_granted(
            cap,
            &self.module.manifest.capabilities,
            &self.granted_capabilities,
        )
    }
}

const FIXED_INPUT_OFFSET: i32 = 8192;

enum BodyViewOwned {
    Raw(bytes::Bytes),
    HashOnly { sha256: [u8; 32], length: u64 },
}

#[async_trait]
impl AuthLayer for WasmSignerLayer {
    fn output_namespace(&self) -> &str {
        &self.namespace
    }

    fn scope_bindings(&self) -> Vec<lucidos_core::engine::proxy_auth_layer::ScopeBinding> {
        self.credential_handles
            .iter()
            .map(|h| {
                lucidos_core::engine::proxy_auth_layer::ScopeBinding::StoredCredential(
                    h.credential.clone(),
                )
            })
            .collect()
    }

    async fn apply(&self, input: &LayerInput<'_>) -> Result<AuthMutation, (StatusCode, String)> {
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
                let h = lucidos_core::engine::proxy_auth_layer::body_hash(bytes);
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

        let body_field = match &body_view {
            BodyViewOwned::Raw(bytes) => SignInputBody::Raw {
                bytes: serde_bytes::ByteBuf::from(bytes.to_vec()),
            },
            BodyViewOwned::HashOnly { sha256, length } => SignInputBody::HashOnly {
                sha256_hex: hex_lower(sha256),
                length: *length,
            },
        };
        // INVARIANT: Least privilege on what the signer is HANDED, not only on what it may
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

        // INVARIANT: 3. Take a signer slot, then spin up a fresh wasmtime Store + Linker
        //    for this call. The scrub list came from the real prior outputs
        //    above, so `log()` stays closed to upstream auth material even for
        //    a granted signer.
        let Ok(Ok(_slot)) = tokio::time::timeout(SIGNER_SLOT_WAIT, self.slots.acquire()).await
        else {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "signer {} was refused: too many signer calls are already running",
                    self.module.name
                ),
            ));
        };
        let host_state = HostState {
            secrets,
            module_name: self.module.name.clone(),
            log_redactions,
            limits: SignerLimits::new(),
            log_budget: LogBudget::default(),
        };
        let mut store: Store<HostState> = Store::new(&self.engine, host_state);
        // INVARIANT: Enforce the resource ceilings: memory, table elements and object
        // counts. Attached BEFORE instantiate so a module declaring an oversized
        // initial memory is refused at instantiation, not after the host has
        // already reserved it. See `SignerLimits`.
        store.limiter(|s| &mut s.limits);
        // INVARIANT: Enforce the execution budget. Set BEFORE instantiate, so instantiate,
        // alloc and sign share one budget. See `yield_until`.
        store.set_epoch_deadline(1);
        store.epoch_deadline_callback(yield_until(Instant::now() + self.budget));
        let mut linker: Linker<HostState> = Linker::new(&self.engine);
        register_host_imports(&mut linker).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("linker init: {e}"),
            )
        })?;
        // INVARIANT: Bound the awaited result before mapping the error: `map_err` reads
        // `store.data()`, which the in-flight call still borrows mutably.
        let instantiated = linker
            .instantiate_async(&mut store, &self.module.module)
            .await;
        let instance = instantiated.map_err(|e| {
            sandbox_or(
                &self.module.name,
                "instantiate",
                StatusCode::BAD_GATEWAY,
                store.data().limits.tripped(),
                &store.data().log_redactions,
                e,
            )
        })?;

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

        let in_len = in_bytes.len() as i32;
        let in_ptr = if let Some(alloc) = &alloc {
            let allocated = alloc.call_async(&mut store, in_len).await;
            allocated.map_err(|e| {
                sandbox_or(
                    &self.module.name,
                    "alloc",
                    StatusCode::INTERNAL_SERVER_ERROR,
                    store.data().limits.tripped(),
                    &store.data().log_redactions,
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

        // INVARIANT: 6. Call sign and decode the packed (out_ptr, out_len). The epoch
        //    callback set on the store above is the budget: it traps a runaway
        //    loop. The `tokio::time::timeout` is a secondary bound, and it can
        //    fire because the running module yields every epoch tick. Grace >
        //    one epoch tick, so the epoch trap and its named stage win first.
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
                    &store.data().log_redactions,
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

        // INVARIANT: 7. Read output and parse SignOutput.
        //
        // Bounds-check the module-supplied (ptr, len) against its OWN linear
        // memory BEFORE allocating. `out_len` is 32 bits of whatever the module
        // returned, so a signer that signals an error the C way (`return -1`)
        // packs `0xFFFF_FFFF` into it and the host would try to zero a 4 GiB
        // buffer: an OOM abort of the whole engine, driven from inside the
        // sandbox. Any slice a real module returns lies inside its memory (the
        // `memory.read` below would fail otherwise), so this rejects nothing
        // that used to work; it only turns the OOM into a 502.
        let reject = |class: &str, detail: String| {
            signer_fault(
                StatusCode::BAD_GATEWAY,
                &self.module.name,
                &format!("returned {class}"),
                &detail,
                &store.data().log_redactions,
            )
        };
        let mem_size = memory.data_size(&store);
        if out_ptr
            .checked_add(out_len)
            .is_none_or(|end| end > mem_size)
        {
            return Err(reject(
                "an out-of-bounds SignOutput slice",
                format!("{out_ptr}/{out_len} (module memory is {mem_size} bytes)"),
            ));
        }
        let mut out_buf = vec![0u8; out_len];
        memory.read(&store, out_ptr, &mut out_buf).map_err(|e| {
            reject(
                "an unreadable SignOutput slice",
                format!("{out_ptr}/{out_len}: {e}"),
            )
        })?;
        let output: SignOutput = serde_json::from_slice(&out_buf)
            .map_err(|e| reject("an unparseable SignOutput", e.to_string()))?;

        if output.replace_body.is_some() && !self.has_capability(CAP_REPLACE_BODY) {
            return Err((
                StatusCode::FORBIDDEN,
                format!(
                    "signer {} requested body replacement without capability grant",
                    self.module.name
                ),
            ));
        }

        let mut add_headers = Vec::with_capacity(output.add_headers.len());
        for (name, value) in output.add_headers {
            let header_name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|e| reject("an invalid header name", format!("{name:?}: {e}")))?;
            // INVARIANT: The proxy's own value check quotes the header name in its body,
            // and the module chose that name. So refuse the value here first.
            // The value can be the credential, so the log names the header only.
            if HeaderValue::from_str(&value).is_err() {
                return Err(reject(
                    "an unsendable header value",
                    format!("for header {name:?}"),
                ));
            }
            add_headers.push((header_name, value));
        }
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
            "replace_body": [104, 105],
        });
        let out: SignOutput = serde_json::from_value(json).unwrap();
        assert_eq!(out.add_query.len(), 2);
        let body = out.replace_body.unwrap();
        assert_eq!(&body[..], b"hi");
    }

    #[test]
    fn signer_slots_leave_most_workers_free() {
        assert_eq!(signer_slots_for(1), 1);
        assert_eq!(signer_slots_for(2), 1);
        assert_eq!(signer_slots_for(8), 2);
        assert_eq!(signer_slots_for(16), 4);
        for workers in 2..=256 {
            assert!(
                signer_slots_for(workers) < workers,
                "{workers} workers must keep one free of signers"
            );
        }
    }

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

        assert!(red.iter().any(|s| s == "super-secret-key"));
        assert!(red.iter().any(|s| s == &hex_lower(b"super-secret-key")));
        assert!(red.iter().any(|s| s == "Bearer upstream-token-xyz"));
        assert!(red.iter().any(|s| s == "client-abc-123"));

        let line = "debug: forwarding Bearer upstream-token-xyz to upstream";
        let scrubbed = crate::core::redact_secret_values(line, &red);
        assert!(!scrubbed.contains("upstream-token-xyz"), "got: {scrubbed}");
        assert!(scrubbed.contains("[REDACTED]"), "got: {scrubbed}");
    }

    #[test]
    fn ungranted_signer_gets_header_names_but_not_values() {
        let (prior, _) = prepare_prior_outputs(handshake_prior(), &[], false);
        let headers = prior["script_handshake"]["headers"].as_object().unwrap();

        assert!(headers.contains_key("authorization"));
        assert!(headers.contains_key("x-client-id"));
        assert_eq!(headers["authorization"], WITHHELD_HEADER_VALUE);
        assert_eq!(headers["x-client-id"], WITHHELD_HEADER_VALUE);
        assert_eq!(
            prior["other"]["log_url"],
            "https://example.com/x?k=REDACTED"
        );
    }

    #[test]
    fn withholding_still_scrubs_the_real_value_from_logs() {
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
