//! Gateway state, supervision, lifecycle, and HTTP routing.
//!
//! Routing (ADR 0014 §2/§3/§10):
//!   * `/~/…`  — the gateway's own surface behind the reserved **sigil
//!     namespace**: `/~/api/v1/health`, `/~/api/v1/control/*`, and the workspace
//!     **picker** + its bundled assets (served from `LUCIDOS_STATIC_DIR`, with
//!     `<base href="/~/">` stamped into the picker's `index.html`).
//!   * `/`     — smart root: exactly one workspace → redirect straight into it
//!     (`/<slug>/`); otherwise serve the picker.
//!   * `/<slug>/…` — reverse-proxied to that workspace's engine (loopback in
//!     packaged, network-bound in dev) as a pure streaming forward (see
//!     [`crate::proxy`]), except for the PWA manifest, which the gateway
//!     re-scopes so a PWA installed from the gateway can navigate between
//!     workspaces without leaving standalone mode.
//!
//! A workspace slug can never start with the sigil (slugs are `[a-z0-9-]`), so
//! the first path segment is unambiguous with no reserved-word list.

use crate::boot_phase::{self, BootPhase};
use crate::error::ApiError;
use crate::net_config;
use crate::postgres::{self, PgBackend, PgHandle};
use crate::proxy;
use crate::registry::{self, Registry, Workspace, SIGIL};
use crate::stack::{self, Health, ProbeOutcome, StackRuntime, WorkspaceStatus};
use crate::BoxError;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;
use tower::ServiceExt;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

/// Default port a bare `lucidos-gateway` binds — the **dev** default, matching
/// the dev gateway data dir (`~/.lucidos/gateway`, see `resolve_app_data`). Both
/// real launch paths pass `LUCIDOS_API_PORT` explicitly, so this is only the
/// no-env fallback: dev (`web-dev.sh`) injects 5251, the packaged desktop app
/// injects its own historical 5252 (`crates/lucidos-app/src/desktop.rs`
/// `DEFAULT_ENGINE_PORT`) — so dev and packaged coexist on different ports out of
/// the box. Override with `LUCIDOS_API_PORT`.
const DEFAULT_GATEWAY_PORT: u16 = 5251;

/// Build id baked in by `build.rs` (git short SHA + a hash of any uncommitted
/// gateway-source diff). A running gateway compares this against the on-disk
/// binary's id — obtained by spawning `current_exe --build-id` — to drive the
/// picker's "new gateway available" badge. Deterministic for identical source so
/// a no-op rebuild does not raise the badge. See
/// `docs/plans/2026-06-18-gateway-reload-control.md`.
pub const GATEWAY_BUILD_ID: &str = env!("GATEWAY_BUILD_ID");

/// In-memory state of the picker's "restore from backup" flow. Single-slot (one
/// restore at a time, like the engine's old `RestoreState`): the POST flips it to
/// `Running`, the spawned task advances `phase` and then sets the terminal
/// `Completed`/`Failed`, and the picker polls `GET /~/api/v1/control/restore-status`
/// for it. Never persisted — a restore that dies with the gateway is simply gone
/// (the half-provisioned workspace is cleaned up; the user re-uploads).
#[derive(Clone, serde::Serialize, Default, Debug, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RestoreStatus {
    /// No restore has run since the gateway started (or the last result was read
    /// and the next restore hasn't begun).
    #[default]
    Idle,
    /// A restore is in flight. `phase` mirrors the engine CLI's
    /// `LUCIDOS_RESTORE_PHASE` ticks (starting, restoring, decrypting,
    /// decompressing, initializing, restoring_db, done).
    Running {
        id: String,
        name: String,
        phase: String,
    },
    /// The restore finished; the workspace `id` is registered and (best-effort)
    /// started — the picker refreshes its list and offers Open.
    Completed { id: String, name: String },
    /// The restore failed before committing; `error` is the user-facing message.
    /// No workspace was registered (cleanup removed the half-provisioned one).
    Failed { name: String, error: String },
}

/// Supervisor cadence + thresholds.
const SUPERVISE_INTERVAL: Duration = Duration::from_secs(2);
/// How long a freshly-(re)spawned engine whose PROCESS is alive has to answer
/// `/api/v1/health` before it's treated as wedged. Cold boot does pgvector init,
/// migrations, and embedding-model warmup, which can take tens of seconds — so a
/// still-booting engine must NOT be respawned out from under itself.
const BOOT_GRACE: Duration = Duration::from_secs(120);
/// Minimum gap between respawn attempts for one stack (crash backoff).
const RESPAWN_BACKOFF: Duration = Duration::from_secs(5);
/// Auto-respawn attempts (since last healthy) before a stack is marked unhealthy.
const RESTART_CAP: u32 = 5;
/// Consecutive missed probes before respawning an engine whose process has
/// EXITED (crash recovery). Small: a real death should recover promptly. An
/// engine that is still ALIVE is never culled — whatever the probe says (see
/// [`respawn_decision`]) — so there is no separate "slow" threshold.
const DEAD_MISS_THRESHOLD: u32 = 2;
/// How long a workspace may sit in the boot window (continuously non-healthy
/// since its boot phase was first set) before the boot splash escapes to the
/// manual "Back to workspaces" page (see [`proxy::stalled_page`]). Must exceed a
/// legitimate slow first-run boot: `BOOT_GRACE` (120s) plus migrations, the
/// memory-model download (the splash label warns "this can take a minute"), and
/// recovery. The escape is needed because an alive-but-unreachable engine (e.g. a
/// misconfigured bind, network partition) is NEVER marked `Unhealthy`
/// ([`respawn_decision`] never culls an alive process), so without a time budget
/// the splash would meta-refresh forever with no way out.
const BOOT_ESCAPE_BUDGET: Duration = Duration::from_secs(240);

/// One workspace's live boot-window state, mirrored in `boot_phases`. `phase`
/// drives the splash label; `since` is when the window OPENED (first
/// `set_boot_phase` of this episode) and is preserved across phase updates so the
/// elapsed-time budget ([`BOOT_ESCAPE_BUDGET`]) accumulates. The whole entry is
/// dropped when the workspace becomes healthy / is stopped.
#[derive(Clone, Copy)]
struct BootProgress {
    phase: BootPhase,
    since: Instant,
}

/// Whether a boot window that opened `elapsed` ago has stalled long enough to
/// show the manual escape page instead of the auto-refreshing "starting" splash.
/// `None` (no boot window — healthy/stopped/unknown) is never stalled. Pure so the
/// budget policy is unit-tested.
fn boot_window_stalled(elapsed: Option<Duration>) -> bool {
    elapsed.is_some_and(|e| e >= BOOT_ESCAPE_BUDGET)
}

/// Shared, cheaply-cloneable gateway handle.
#[derive(Clone)]
pub struct GatewayState {
    inner: Arc<GatewayInner>,
}

struct GatewayInner {
    /// Base dir for relative workspace dirs, `deleted/`, and `config/`.
    app_data: PathBuf,
    registry_path: PathBuf,
    gateway_port: u16,
    /// The engine binary to spawn per workspace. Must be explicit
    /// (`LUCIDOS_ENGINE_BIN`) — the gateway's own `current_exe` is the gateway,
    /// not the engine (ADR 0014 §1).
    engine_bin: PathBuf,
    /// The built frontend dir (`dist/`) the gateway serves the picker from, and
    /// passes to engines via the inherited env so they serve it too.
    static_dir: Option<PathBuf>,
    /// Whether spawned engines bind loopback-only (packaged: `true`, the security
    /// posture) or all interfaces (dev: `false`, so the workspace app is reachable
    /// directly on its port in addition to via the gateway). See
    /// `LUCIDOS_GATEWAY_ENGINE_LOOPBACK` and ADR 0014 "Dev runtime topology".
    engine_loopback: bool,
    /// Whether the spawned engine serves TLS on its port. Packaged engines serve
    /// plain HTTP on their loopback port (`false` — the gateway terminates TLS). A
    /// dev engine is the direct front and keeps its TLS cert when one is
    /// configured, so the gateway must proxy + health-probe it over https.
    engine_tls: bool,
    pg_backend: PgBackend,
    /// True under the packaged desktop runtime (`desktop.rs::spawn_gateway` sets
    /// `LUCIDOS_PACKAGED=1`), false in dev (`web-dev.sh` sets nothing). Reported in
    /// `GET /~/api/v1/control/gateway/status` so the picker hides the dev-only
    /// gateway self-reload control in packaged (where binary swaps happen via the
    /// app updater + a full service restart, never a re-exec under a running
    /// gateway).
    packaged: bool,
    /// Shared-cluster provisioning is serialized across workspaces. Docker
    /// container creation and embedded `pg_ctl` startup are cluster-level
    /// operations; concurrent per-workspace starts should queue at this boundary,
    /// then create/verify their own databases.
    pg_lock: AsyncMutex<()>,
    proxy_client: reqwest::Client,
    health_client: reqwest::Client,
    /// On-disk registry, source of truth. Mutated under a short lock (never held
    /// across an await / provisioning).
    registry: Mutex<Registry>,
    /// Runtime stacks, keyed by workspace id.
    stacks: AsyncMutex<HashMap<String, Arc<AsyncMutex<StackRuntime>>>>,
    /// Workspace ids currently being lazily brought up (proxy-hit on a stopped
    /// workspace). Guards against a burst of concurrent requests each spawning a
    /// duplicate engine before the stack lands in `stacks`.
    starting: AsyncMutex<HashSet<String>>,
    /// Hot-path id → engine port map for the proxy (the engine binds loopback in
    /// packaged builds, all interfaces in dev), so proxying never contends with a
    /// stack mutex held during a multi-second respawn.
    routes: RwLock<HashMap<String, u16>>,
    /// id → current cold-boot phase, rendered as the boot-splash label
    /// ([`crate::boot_phase`]). Gateway-observed phases are written here directly
    /// (provisioning, spawning); engine-reported phases arrive via the
    /// `boot-phase` control endpoint. Entries are removed once the workspace is
    /// healthy / stopped so a later cold open never shows a stale phase. Mirrors
    /// the `routes` map (cheap `RwLock`, off the stack-mutex path).
    boot_phases: RwLock<HashMap<String, BootProgress>>,
    /// Single-slot state of the picker's restore-from-backup flow (see
    /// [`RestoreStatus`]). Polled via the control API; never persisted.
    restore: RwLock<RestoreStatus>,
    /// Path of the binary this process was launched from (`current_exe`), used by
    /// the reload control to re-exec onto the rebuilt binary and to stat for the
    /// "new gateway available" check.
    exe_path: Option<PathBuf>,
    /// Cached result of the on-disk-binary update check, so the picker's 2s poll
    /// doesn't fork `current_exe --build-id` on every tick — re-checked only when
    /// the binary's mtime moves. See [`GatewayState::gateway_update_available`].
    update_check: Mutex<UpdateCheck>,
}

/// Memoized "is a newer gateway binary on disk?" verdict, keyed by the binary's
/// last-seen mtime. A `None` mtime means "not yet checked".
#[derive(Default)]
struct UpdateCheck {
    last_mtime: Option<std::time::SystemTime>,
    update_available: bool,
}

impl GatewayState {
    fn app_data(&self) -> &PathBuf {
        &self.inner.app_data
    }

    /// Loopback port for `id`, if registered. Hot path — no async locks.
    fn route(&self, id: &str) -> Option<u16> {
        self.inner.routes.read().ok()?.get(id).copied()
    }

    /// The scheme the spawned engine serves on its port (`https` for a dev engine
    /// with TLS, else `http`). Used to build proxy targets + health probes.
    fn engine_scheme(&self) -> &'static str {
        if self.inner.engine_tls {
            "https"
        } else {
            "http"
        }
    }

    fn set_route(&self, id: &str, port: u16) {
        if let Ok(mut r) = self.inner.routes.write() {
            r.insert(id.to_string(), port);
        }
    }

    fn clear_route(&self, id: &str) {
        if let Ok(mut r) = self.inner.routes.write() {
            r.remove(id);
        }
    }

    /// Record the current cold-boot phase for `id` (rendered on the boot splash).
    /// Hot path — no async locks. Called by the gateway as it provisions/spawns
    /// and by the `boot-phase` control endpoint when the engine reports.
    pub fn set_boot_phase(&self, id: &str, phase: BootPhase) {
        if let Ok(mut p) = self.inner.boot_phases.write() {
            // Preserve `since` across phase updates: it marks when THIS boot window
            // opened, so the escape budget ([`BOOT_ESCAPE_BUDGET`]) accumulates
            // over the whole episode rather than resetting on every phase advance.
            p.entry(id.to_string())
                .and_modify(|bp| bp.phase = phase)
                .or_insert(BootProgress {
                    phase,
                    since: Instant::now(),
                });
        }
    }

    /// Drop any boot phase for `id`. Called when the workspace becomes healthy or
    /// is stopped, so a later cold open starts from the default label rather than
    /// a stale phase from the previous boot.
    pub fn clear_boot_phase(&self, id: &str) {
        if let Ok(mut p) = self.inner.boot_phases.write() {
            p.remove(id);
        }
    }

    /// The boot-splash label for `id` — the current phase's label, or the default
    /// when no phase is known yet (initial paint / between boots).
    fn boot_phase_label(&self, id: &str) -> &'static str {
        self.inner
            .boot_phases
            .read()
            .ok()
            .and_then(|p| p.get(id).map(|bp| bp.phase.label()))
            .unwrap_or(boot_phase::DEFAULT_LABEL)
    }

    /// How long `id`'s current boot window has been open, or `None` when there is
    /// no boot window (healthy / stopped / never opened). Cheap `RwLock` read off
    /// the stack-mutex path — safe on the proxy hot path. Drives the boot-splash
    /// escape decision ([`boot_window_stalled`]).
    fn boot_elapsed(&self, id: &str) -> Option<Duration> {
        self.inner
            .boot_phases
            .read()
            .ok()
            .and_then(|p| p.get(id).map(|bp| bp.since.elapsed()))
    }

    /// The single workspace's slug when exactly one is registered, else `None`.
    fn sole_workspace(&self) -> Option<String> {
        let reg = self.inner.registry.lock().unwrap();
        if reg.workspaces.len() == 1 {
            Some(reg.workspaces[0].id.clone())
        } else {
            None
        }
    }

    // ── Status ───────────────────────────────────────────────────────────────

    /// Per-workspace status in registry order (stable for the picker).
    pub async fn list_status(&self) -> Vec<WorkspaceStatus> {
        let order: Vec<Workspace> = {
            let reg = self.inner.registry.lock().unwrap();
            reg.workspaces.clone()
        };
        // Snapshot the stack handles, then release the map lock before locking
        // individual stacks: the supervisor can hold a stack mutex across a
        // multi-second health probe, and pinning the whole map meanwhile would
        // stall create/delete and the picker's 2s poll.
        let stacks: HashMap<String, Arc<AsyncMutex<StackRuntime>>> =
            self.inner.stacks.lock().await.clone();
        let mut out = Vec::with_capacity(order.len());
        for ws in order {
            if let Some(stack) = stacks.get(&ws.id) {
                out.push(stack.lock().await.status());
            } else {
                out.push(WorkspaceStatus {
                    id: ws.id.clone(),
                    name: ws.name.clone(),
                    port: ws.port,
                    health: Health::Unhealthy,
                    autostart: ws.autostart,
                    last_error: Some("not started".to_string()),
                    // Stopped workspace → no engine to poll → no badge.
                    unread_count: None,
                });
            }
        }
        out
    }

    // ── Self-update (reload onto a rebuilt binary) ─────────────────────────────

    /// This process's baked build id.
    pub fn build_id(&self) -> &'static str {
        GATEWAY_BUILD_ID
    }

    /// True under the packaged desktop runtime. The picker hides the dev-only
    /// gateway self-reload control when this is set (binary swaps in packaged go
    /// through the app updater + a full service restart, not a gateway re-exec).
    pub fn packaged(&self) -> bool {
        self.inner.packaged
    }

    /// Whether the on-disk gateway binary has a different build id than this
    /// running process — i.e. a rebuild is waiting to be adopted via
    /// [`Self::reload_gateway`]. Cheap on the steady path: it only forks
    /// `current_exe --build-id` when the binary's mtime has moved since the last
    /// check (the picker polls this every 2s). A dev `--engine-only` Apply rebuilds
    /// the gateway binary while leaving the running gateway up, which is exactly the
    /// state this detects (see `docs/plans/2026-06-18-gateway-reload-control.md`).
    pub async fn gateway_update_available(&self) -> bool {
        let Some(exe) = self.inner.exe_path.clone() else {
            return false;
        };
        let mtime = std::fs::metadata(&exe).and_then(|m| m.modified()).ok();
        // Fast path: mtime unchanged since the last check → reuse the verdict.
        {
            let cache = self.inner.update_check.lock().unwrap();
            if cache.last_mtime == mtime {
                return cache.update_available;
            }
        }
        // mtime moved (or first check): ask the on-disk binary for its build id.
        let disk_id = match tokio::process::Command::new(&exe)
            .arg("--build-id")
            .output()
            .await
        {
            Ok(out) if out.status.success() => {
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            }
            // Can't read the on-disk id (binary mid-rewrite, spawn failure) — treat
            // as "no update" and let the next poll retry once the mtime settles.
            _ => return false,
        };
        let update_available = !disk_id.is_empty() && disk_id != GATEWAY_BUILD_ID;
        let mut cache = self.inner.update_check.lock().unwrap();
        cache.last_mtime = mtime;
        cache.update_available = update_available;
        update_available
    }

    /// Re-exec this process onto the on-disk binary — same PID, so the supervisor
    /// keeps `wait`ing on us and the pidfile stays valid; the fresh `main()`
    /// re-adopts the running engines on boot (see [`Self::boot_all`]). This is the
    /// ONLY in-place gateway restart: signalling the supervisor (SIGUSR1) is its
    /// *permanent* stop, not a restart (see `scripts/lib/gateway_supervisor.sh`).
    ///
    /// Returns after scheduling the exec so the HTTP caller still gets its
    /// response; the actual `execv` happens on a short delay. `execv` only returns
    /// on failure, in which case we log and keep running the current image.
    pub fn reload_gateway(&self) -> Result<(), BoxError> {
        let exe = self
            .inner
            .exe_path
            .clone()
            .ok_or("current_exe unavailable — cannot reload")?;
        let args: Vec<String> = std::env::args().skip(1).collect();
        crate::log!(
            "[Gateway] reload requested — re-exec {} (build id {})",
            exe.display(),
            GATEWAY_BUILD_ID
        );
        tokio::spawn(async move {
            // Give the 202 response time to flush before we replace the image.
            tokio::time::sleep(Duration::from_millis(300)).await;
            use std::os::unix::process::CommandExt;
            // `.exec()` replaces this process image and never returns on success.
            // Env is inherited; argv mirrors the original launch. CLOEXEC on the
            // listening socket (Rust default) frees the port for the new image.
            let err = std::process::Command::new(&exe).args(&args).exec();
            crate::log!("[Gateway] reload re-exec failed, staying on current image: {err}");
        });
        Ok(())
    }

    // ── Lifecycle ──────────────────────────────────────────────────────────────

    /// Bring up registered workspaces on gateway startup, concurrently and
    /// failure-isolated. Per workspace:
    ///   * an engine already answering health is **re-adopted** (regardless of
    ///     `autostart`) — engine-statelessness across a gateway restart;
    ///   * else an `autostart` workspace is **spawned** (the always-on posture:
    ///     a login-launched packaged gateway brings up its auto-start workspaces);
    ///   * else it is **left stopped** — listed in the picker (via
    ///     [`Self::list_status`]'s no-stack branch) and started only on an
    ///     explicit open (lazy, [`Self::lazy_start`]) or launch.
    async fn boot_all(&self) {
        let workspaces: Vec<Workspace> = {
            let reg = self.inner.registry.lock().unwrap();
            reg.workspaces.clone()
        };
        let futures = workspaces.into_iter().map(|ws| {
            let me = self.clone();
            async move {
                let running =
                    stack::probe_health(&me.inner.health_client, me.engine_scheme(), ws.port).await
                        == ProbeOutcome::Healthy;
                if running || ws.autostart {
                    // bring_up itself re-adopts a healthy engine and only spawns
                    // when none is running, so this is correct for both cases.
                    me.bring_up(ws).await;
                }
            }
        });
        futures::future::join_all(futures).await;
    }

    /// Re-read the on-disk registry into memory. The dev launcher writes the
    /// shared registry file directly (`seed_gateway_registry`), so a running
    /// gateway's in-memory copy can lag a freshly-launched workspace; this
    /// resyncs it so the picker lists it and [`Self::restart_workspace`] can find
    /// it. A bad read is logged, not propagated — a transient parse error must
    /// not break a restart of an already-known workspace.
    fn sync_registry_from_disk(&self) {
        match Registry::load(&self.inner.registry_path) {
            Ok(reg) => *self.inner.registry.lock().unwrap() = reg,
            Err(e) => crate::log!("[Gateway] registry reload failed: {}", e),
        }
    }

    /// Lazily start a registered-but-stopped workspace for a document navigation,
    /// then serve the boot window. Guarded by [`GatewayInner::starting`] so a
    /// burst of concurrent requests can't each spawn a duplicate engine before
    /// the stack lands in `stacks`. No-op if a stack already exists or a start is
    /// in flight.
    async fn lazy_start(&self, id: &str) {
        if self.inner.stacks.lock().await.contains_key(id) {
            return;
        }
        {
            let mut starting = self.inner.starting.lock().await;
            if !starting.insert(id.to_string()) {
                return; // already being started by another request
            }
        }
        let ws = self.inner.registry.lock().unwrap().get(id).cloned();
        if let Some(ws) = ws {
            crate::log!("[Gateway] lazy-starting '{}' on demand", id);
            self.bring_up(ws).await;
        }
        self.inner.starting.lock().await.remove(id);
    }

    /// Provision Postgres + adopt-or-spawn the engine for one workspace, then
    /// register its runtime stack + route. Never panics; a failure yields an
    /// `Unhealthy` stack carrying the error (surfaced in the picker).
    async fn bring_up(&self, ws: Workspace) {
        let resolved_dir = ws.resolve_dir(self.app_data());
        if let Err(e) = std::fs::create_dir_all(resolved_dir.join("data")) {
            crate::log!("[Gateway] {} create dir failed: {}", ws.id, e);
        }

        let stack = match self.provision_and_start(&ws, &resolved_dir).await {
            Ok((pg, engine, health)) => StackRuntime {
                ws: ws.clone(),
                resolved_dir,
                pg,
                engine,
                health,
                restart_attempts: 0,
                health_misses: 0,
                last_spawn: Some(Instant::now()),
                last_error: None,
                last_unread: None,
            },
            Err(e) => {
                crate::log!("[Gateway] workspace '{}' failed to start: {}", ws.id, e);
                // Start failed — drop any phase so the splash shows the neutral
                // default rather than a stale "Provisioning database…".
                self.clear_boot_phase(&ws.id);
                StackRuntime {
                    ws: ws.clone(),
                    resolved_dir,
                    pg: PgHandle::External,
                    engine: None,
                    health: Health::Unhealthy,
                    restart_attempts: RESTART_CAP,
                    health_misses: 0,
                    last_spawn: None,
                    last_error: Some(e.to_string()),
                    last_unread: None,
                }
            }
        };

        self.set_route(&ws.id, ws.port);
        self.inner
            .stacks
            .lock()
            .await
            .insert(ws.id.clone(), Arc::new(AsyncMutex::new(stack)));
        crate::log!("[Gateway] workspace '{}' engine on :{}", ws.id, ws.port);
    }

    /// Ensure Postgres, then adopt a healthy already-running engine or spawn a
    /// fresh one.
    async fn provision_and_start(
        &self,
        ws: &Workspace,
        resolved_dir: &Path,
    ) -> Result<(PgHandle, Option<std::process::Child>, Health), BoxError> {
        // First splash phase: provisioning Postgres can pull/start a container or
        // run `initdb` on a first-ever open.
        self.set_boot_phase(&ws.id, BootPhase::ProvisioningDatabase);
        let prov = self.ensure_postgres(ws).await?;

        if stack::probe_health(&self.inner.health_client, self.engine_scheme(), ws.port).await
            == ProbeOutcome::Healthy
        {
            crate::log!("[Gateway] re-adopting healthy engine for '{}'", ws.id);
            // Already up — no boot window to narrate.
            self.clear_boot_phase(&ws.id);
            return Ok((prov.handle, None, Health::Healthy));
        }

        stack::reclaim_stale_engine(resolved_dir);
        let child = stack::spawn_engine(
            &self.inner.engine_bin,
            ws,
            resolved_dir,
            &prov.database_url,
            self.inner.gateway_port,
            self.inner.engine_loopback,
        )?;
        // Engine process is up; it now reports finer phases (migrating, building
        // search index, recovering) via the boot-phase control endpoint until its
        // first healthy probe clears the phase.
        self.set_boot_phase(&ws.id, BootPhase::StartingEngine);
        Ok((prov.handle, Some(child), Health::Booting))
    }

    /// Create a new workspace: reserve a registry entry (id + free port), persist
    /// it, then provision + start the stack. The entry is committed before
    /// provisioning so a provisioning failure leaves a recoverable `Unhealthy`
    /// workspace (retry / delete from the picker) rather than vanishing.
    ///
    /// New workspaces are created with `autostart = false`: the user opens this
    /// one now, and whether it auto-starts on a future gateway boot is their
    /// per-workspace picker toggle. (This is the sole creation path — there is no
    /// auto-created bootstrap `default`; first run shows the picker.)
    pub async fn create_workspace(&self, name: &str) -> Result<WorkspaceStatus, BoxError> {
        let ws = {
            let mut reg = self.inner.registry.lock().unwrap();
            let base = registry::slugify(name);
            let id = registry::unique_slug(&base, &|s| reg.contains(s));
            let port = reg.allocate_port()?;
            let ws = Workspace {
                id: id.clone(),
                name: name.to_string(),
                dir: format!("workspaces/{id}"),
                port,
                database_url: None,
                autostart: false,
            };
            reg.add(ws.clone())?;
            reg.save(&self.inner.registry_path)?;
            ws
        };

        self.bring_up(ws.clone()).await;
        // Clone the Arc out and drop the map lock before locking the stack — the
        // supervisor holds stack→map, so map→stack here would deadlock (same
        // ordering as restart/stop/delete).
        let stack = self.inner.stacks.lock().await.get(&ws.id).cloned();
        let status = match stack {
            Some(s) => s.lock().await.status(),
            None => WorkspaceStatus {
                id: ws.id.clone(),
                name: ws.name.clone(),
                port: ws.port,
                health: Health::Unhealthy,
                autostart: ws.autostart,
                last_error: Some("stack missing after create".to_string()),
                unread_count: None,
            },
        };
        Ok(status)
    }

    /// Current restore-flow state, for the picker's poll.
    pub fn restore_status(&self) -> RestoreStatus {
        self.inner
            .restore
            .read()
            .map(|s| s.clone())
            .unwrap_or(RestoreStatus::Idle)
    }

    /// Clear a terminal (completed/failed) restore result back to Idle so the
    /// picker's banner can be dismissed. Refused while a restore is running so a
    /// stray call can't wipe live progress.
    pub fn clear_restore_status(&self) -> Result<(), ApiError> {
        let mut st = self
            .inner
            .restore
            .write()
            .map_err(|_| ApiError::internal("restore state poisoned"))?;
        if matches!(*st, RestoreStatus::Running { .. }) {
            return Err(ApiError::conflict("A restore is in progress"));
        }
        *st = RestoreStatus::Idle;
        Ok(())
    }

    /// Update the `phase` of an in-flight restore (no-op if not Running).
    fn set_restore_phase(&self, phase: &str) {
        if let Ok(mut st) = self.inner.restore.write() {
            if let RestoreStatus::Running { phase: p, .. } = &mut *st {
                *p = phase.to_string();
            }
        }
    }

    /// Begin restoring a local backup archive (already streamed to `archive_tmp`)
    /// into a NEW workspace. Validates synchronously — derives the workspace name
    /// (explicit `requested_name`, else parsed from `archive_filename`), rejects a
    /// name collision so the picker can ask for a different one (the user's
    /// "ask-on-collision" rule), and reserves a free port — then spawns the heavy
    /// restore in the background and returns `{id, name}` (202). The picker polls
    /// [`Self::restore_status`] for progress / completion / failure.
    ///
    /// The registry entry is committed only AFTER the archive is restored into the
    /// dir + database, so a failure before then leaves nothing registered and the
    /// cleanup removes only what this attempt created (the provisioned Postgres +
    /// the fresh dir) — never a pre-existing workspace.
    pub async fn restore_workspace(
        &self,
        archive_tmp: PathBuf,
        archive_filename: String,
        key_b64: String,
        requested_name: Option<String>,
    ) -> Result<(String, String), ApiError> {
        // Derive the display name: an explicit name wins; otherwise parse it from
        // the archive filename. A non-archive filename with no explicit name is a
        // 400 telling the picker to ask for one.
        let derived = || registry::parse_workspace_name_from_archive(&archive_filename);
        let name = requested_name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .or_else(derived)
            .ok_or_else(|| {
                let _ = std::fs::remove_file(&archive_tmp);
                ApiError::bad_request(
                    "Could not determine a workspace name from the file — enter one.",
                )
            })?;

        // Reserve id + port under the registry lock. On a slug collision, reject
        // so the picker asks for a different name (NOT a silent `-2` suffix).
        let (id, port) = {
            let reg = self.inner.registry.lock().unwrap();
            let slug = registry::slugify(&name);
            if reg.contains(&slug) {
                let _ = std::fs::remove_file(&archive_tmp);
                return Err(ApiError::conflict(format!(
                    "A workspace named \"{name}\" already exists — choose a different name."
                )));
            }
            let port = reg.allocate_port().map_err(|e| {
                let _ = std::fs::remove_file(&archive_tmp);
                ApiError::internal(e.to_string())
            })?;
            (slug, port)
        };

        let ws = Workspace {
            id: id.clone(),
            name: name.clone(),
            dir: format!("workspaces/{id}"),
            port,
            database_url: None,
            autostart: false,
        };

        // Atomically claim the single restore slot: check-and-set under ONE write
        // lock so two near-simultaneous restores can't both pass (the control
        // handler's pre-check is a best-effort fast-fail; this is the
        // authoritative gate, mirroring the engine's old `try_start`).
        {
            let mut st = self.inner.restore.write().map_err(|_| {
                let _ = std::fs::remove_file(&archive_tmp);
                ApiError::internal("restore state poisoned")
            })?;
            if matches!(*st, RestoreStatus::Running { .. }) {
                let _ = std::fs::remove_file(&archive_tmp);
                return Err(ApiError::conflict("A restore is already in progress"));
            }
            *st = RestoreStatus::Running {
                id: id.clone(),
                name: name.clone(),
                phase: "starting".to_string(),
            };
        }
        let me = self.clone();
        let (id_done, name_done) = (id.clone(), name.clone());
        tokio::spawn(async move {
            let outcome = me.run_restore(ws, &archive_tmp, &key_b64).await;
            let _ = std::fs::remove_file(&archive_tmp);
            let next = match outcome {
                Ok(()) => RestoreStatus::Completed {
                    id: id_done,
                    name: name_done,
                },
                Err(e) => {
                    crate::log!("[Gateway] restore '{}' failed: {}", id_done, e);
                    RestoreStatus::Failed {
                        name: name_done,
                        error: e,
                    }
                }
            };
            if let Ok(mut st) = me.inner.restore.write() {
                *st = next;
            }
        });

        Ok((id, name))
    }

    /// The heavy restore: provision Postgres ONCE, run the engine `restore-archive`
    /// CLI into that dir + DB, then commit the registry entry and spawn the engine
    /// server. Returns a user-facing error string on failure (the caller records
    /// it as `Failed`). Cleanup on a pre-commit failure removes only what this
    /// attempt created.
    async fn run_restore(
        &self,
        ws: Workspace,
        archive_tmp: &Path,
        key_b64: &str,
    ) -> Result<(), String> {
        let resolved_dir = ws.resolve_dir(self.app_data());
        std::fs::create_dir_all(resolved_dir.join("data"))
            .map_err(|e| format!("create workspace dir: {e}"))?;

        // Provision Postgres ONCE and reuse its URL for BOTH the CLI restore and
        // the engine server — re-provisioning (e.g. via bring_up) would, for the
        // embedded backend, stop+restart the cluster on a new port between the two
        // steps. So we spawn the engine directly here rather than calling bring_up.
        let prov = match self.ensure_postgres(&ws).await {
            Ok(p) => p,
            Err(e) => {
                self.cleanup_failed_restore(&resolved_dir, None);
                return Err(format!("provision postgres: {e}"));
            }
        };

        self.set_restore_phase("restoring");
        if let Err(e) = self
            .run_restore_cli(&resolved_dir, &prov.database_url, archive_tmp, key_b64)
            .await
        {
            self.cleanup_failed_restore(&resolved_dir, Some(&prov.handle));
            return Err(e);
        }

        // Commit the registry entry now that the dir + database carry the restore.
        {
            let mut reg = self.inner.registry.lock().unwrap();
            let added = reg
                .add(ws.clone())
                .and_then(|()| reg.save(&self.inner.registry_path));
            if let Err(e) = added {
                drop(reg);
                self.cleanup_failed_restore(&resolved_dir, Some(&prov.handle));
                return Err(format!("register workspace: {e}"));
            }
        }

        // Spawn the engine server on the provisioned (already-restored) DB; its
        // construction runs migrations, upgrading an older-schema restore. A spawn
        // failure here is non-fatal: the workspace is registered + restored, so the
        // picker lists it (stopped) and an Open lazy-starts it — don't tear down
        // the user's just-restored data.
        stack::reclaim_stale_engine(&resolved_dir);
        match stack::spawn_engine(
            &self.inner.engine_bin,
            &ws,
            &resolved_dir,
            &prov.database_url,
            self.inner.gateway_port,
            self.inner.engine_loopback,
        ) {
            Ok(child) => {
                let stack = StackRuntime {
                    ws: ws.clone(),
                    resolved_dir,
                    pg: prov.handle,
                    engine: Some(child),
                    health: Health::Booting,
                    restart_attempts: 0,
                    health_misses: 0,
                    last_spawn: Some(Instant::now()),
                    last_error: None,
                    last_unread: None,
                };
                self.set_route(&ws.id, ws.port);
                self.inner
                    .stacks
                    .lock()
                    .await
                    .insert(ws.id.clone(), Arc::new(AsyncMutex::new(stack)));
                crate::log!("[Gateway] restored '{}' engine on :{}", ws.id, ws.port);
            }
            Err(e) => {
                crate::log!(
                    "[Gateway] restored '{}' but engine spawn failed ({}) — left registered (stopped)",
                    ws.id,
                    e
                );
            }
        }
        Ok(())
    }

    /// Shell out to `lucidos-engine restore-archive` to decrypt + unpack +
    /// pg_restore the archive into `ws_dir` + `database_url`. The key and DB URL go
    /// via env (out of argv); stdout `LUCIDOS_RESTORE_PHASE=<phase>:<pct>` lines
    /// drive the picker's phase; stderr (tail) is the failure message.
    async fn run_restore_cli(
        &self,
        ws_dir: &Path,
        database_url: &str,
        archive_tmp: &Path,
        key_b64: &str,
    ) -> Result<(), String> {
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

        let mut child = tokio::process::Command::new(&self.inner.engine_bin)
            .arg("restore-archive")
            .arg("--file")
            .arg(archive_tmp)
            .arg("--workspace-dir")
            .arg(ws_dir)
            .env("LUCIDOS_RESTORE_KEY", key_b64)
            .env("DATABASE_URL", database_url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn restore-archive: {e}"))?;

        // Stream stdout for coarse phase updates.
        if let Some(stdout) = child.stdout.take() {
            let me = self.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(rest) = line.strip_prefix("LUCIDOS_RESTORE_PHASE=") {
                        let phase = rest.split(':').next().unwrap_or(rest);
                        me.set_restore_phase(phase);
                    }
                }
            });
        }

        // Collect stderr for the failure message.
        let mut stderr_buf = String::new();
        if let Some(mut serr) = child.stderr.take() {
            let _ = serr.read_to_string(&mut stderr_buf).await;
        }

        let status = child
            .wait()
            .await
            .map_err(|e| format!("wait restore-archive: {e}"))?;
        if !status.success() {
            let tail: Vec<&str> = stderr_buf.lines().rev().take(5).collect();
            let msg: String = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
            return Err(if msg.trim().is_empty() {
                format!("restore failed ({status})")
            } else {
                msg
            });
        }
        Ok(())
    }

    /// Tear down a half-provisioned restore: stop the Postgres this attempt
    /// provisioned (if any) and remove the freshly-created workspace dir. Only ever
    /// touches state THIS attempt created — the dir is `workspaces/<freshly-allocated-id>`
    /// and the registry entry isn't committed until after a successful restore.
    fn cleanup_failed_restore(&self, resolved_dir: &Path, pg: Option<&PgHandle>) {
        if let Some(pg) = pg {
            pg.teardown();
        }
        if resolved_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(resolved_dir) {
                crate::log!(
                    "[Gateway] restore cleanup: could not remove {}: {}",
                    resolved_dir.display(),
                    e
                );
            }
        }
    }

    async fn ensure_postgres(&self, ws: &Workspace) -> Result<postgres::Provisioned, BoxError> {
        let _guard = self.inner.pg_lock.lock().await;
        postgres::ensure(
            &self.inner.pg_backend,
            &ws.id,
            self.app_data(),
            ws.database_url.as_deref(),
        )
        .await
    }

    async fn teardown_postgres(&self, ws: &Workspace) -> Result<(), BoxError> {
        let _guard = self.inner.pg_lock.lock().await;
        postgres::teardown_workspace(&self.inner.pg_backend, &ws.id, self.app_data()).await
    }

    /// Rename = edit the display name only (registry + runtime). No dir move, DB
    /// reconnect, or port change.
    pub async fn rename_workspace(&self, id: &str, name: &str) -> Result<(), BoxError> {
        {
            let mut reg = self.inner.registry.lock().unwrap();
            let ws = reg
                .get_mut(id)
                .ok_or_else(|| format!("workspace '{id}' not found"))?;
            ws.name = name.to_string();
            reg.save(&self.inner.registry_path)?;
        }
        // Clone the Arc out and drop the map lock before locking the stack —
        // the supervisor holds stack→map, so map→stack here would deadlock (see
        // `set_autostart` / the restart/stop/delete pattern).
        let stack = self.inner.stacks.lock().await.get(id).cloned();
        if let Some(stack) = stack {
            stack.lock().await.ws.name = name.to_string();
        }
        crate::log!("[Gateway] renamed '{}' -> '{}'", id, name);
        Ok(())
    }

    /// Start or restart one workspace (the picker's "Retry"/"Open", the dev Apply
    /// engine-only restart, and the dev launcher's start-the-active-workspace
    /// step all route here). Resyncs the registry from disk first so a
    /// freshly-launched workspace is known, then:
    ///   * if a stack exists → respawn it (resets the restart cap), forcing the
    ///     engine onto a rebuilt binary (the Apply case);
    ///   * if none exists → bring it up from the registry entry (start a stopped
    ///     workspace).
    pub async fn restart_workspace(&self, id: &str) -> Result<(), BoxError> {
        // The dev launcher writes the shared registry file directly before
        // POSTing here, so pick up any newly-seeded entry / flag change.
        self.sync_registry_from_disk();
        let existing = self.inner.stacks.lock().await.get(id).cloned();
        match existing {
            Some(stack) => {
                let mut s = stack.lock().await;
                s.restart_attempts = 0;
                self.respawn_stack(&mut s).await;
                Ok(())
            }
            None => {
                // Not running — start it. Route through the guarded lazy_start so
                // a concurrent proxy-hit lazy-start can't double-spawn the engine.
                if !self.inner.registry.lock().unwrap().contains(id) {
                    return Err(format!("workspace '{id}' not found").into());
                }
                self.lazy_start(id).await;
                Ok(())
            }
        }
    }

    /// Stop one workspace's engine and drop its runtime stack, but KEEP its
    /// registry entry (it stays listed in the picker as stopped — membership is
    /// "all ever launched"). The dev `stop.sh` calls this so the shared gateway
    /// forgets the stack and its supervisor stops respawning the engine; the
    /// entry survives so the picker still lists it. Postgres is left untouched
    /// (dev PG is externally managed; a packaged cluster stays up for a quick
    /// restart). A no-op if the workspace isn't currently running.
    pub async fn stop_workspace(&self, id: &str) -> Result<(), BoxError> {
        let removed = self.inner.stacks.lock().await.remove(id);
        if let Some(stack) = removed {
            let mut s = stack.lock().await;
            stop_engine_process(&mut s);
        }
        self.clear_route(id);
        // No live route means the next open lazy-starts fresh — drop any stale
        // boot phase so that open begins from the default splash label.
        self.clear_boot_phase(id);
        crate::log!("[Gateway] stopped '{}' (kept in registry)", id);
        Ok(())
    }

    /// Flip a workspace's `autostart` flag (registry only — does NOT start or
    /// stop the engine). Persisted so it survives a gateway restart; the picker's
    /// per-workspace toggle drives this.
    pub async fn set_autostart(&self, id: &str, enabled: bool) -> Result<(), BoxError> {
        {
            let mut reg = self.inner.registry.lock().unwrap();
            let ws = reg
                .get_mut(id)
                .ok_or_else(|| format!("workspace '{id}' not found"))?;
            ws.autostart = enabled;
            reg.save(&self.inner.registry_path)?;
        }
        // Keep a running stack's copy in sync so a later respawn carries it.
        // Clone the Arc out and DROP the map lock before locking the stack: the
        // supervisor holds stack→map (briefly, in its apply phase), so holding
        // map→stack here would be an AB-BA deadlock. Same ordering as
        // restart/stop/delete.
        let stack = self.inner.stacks.lock().await.get(id).cloned();
        if let Some(stack) = stack {
            stack.lock().await.ws.autostart = enabled;
        }
        crate::log!("[Gateway] '{}' autostart = {}", id, enabled);
        Ok(())
    }

    /// Delete-to-trash: optional type-the-name confirm → stop the stack →
    /// unregister → move the dir to `<app-data>/deleted/<id>-<ts>/` (recoverable
    /// until purged). Never an immediate `rm`.
    pub async fn delete_workspace(&self, id: &str, confirm: Option<&str>) -> Result<(), BoxError> {
        let ws = {
            let reg = self.inner.registry.lock().unwrap();
            reg.get(id)
                .cloned()
                .ok_or_else(|| format!("workspace '{id}' not found"))?
        };
        if let Some(c) = confirm {
            if c.trim() != ws.name {
                return Err("confirmation does not match the workspace name".into());
            }
        }

        // Stop the stack engine and drop it from the runtime maps. The database
        // is dropped below by registry id so stopped workspaces and unhealthy
        // stacks without a PgHandle are cleaned up the same way. The shared
        // Postgres cluster stays up for peers.
        // Take the stack out from under the map lock first so the blocking
        // process signal doesn't pin the whole map and stall every other
        // gateway operation.
        let removed = self.inner.stacks.lock().await.remove(id);
        if let Some(stack) = removed {
            let mut s = stack.lock().await;
            stop_engine_process(&mut s);
        }
        if let Err(e) = self.teardown_postgres(&ws).await {
            crate::log!("[Gateway] '{}' database teardown failed: {}", id, e);
        }
        self.clear_route(id);

        // Unregister, then move the dir to trash.
        {
            let mut reg = self.inner.registry.lock().unwrap();
            reg.remove(id)?;
            reg.save(&self.inner.registry_path)?;
        }

        let resolved_dir = ws.resolve_dir(self.app_data());
        if resolved_dir.exists() {
            let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
            let trash = self.app_data().join("deleted").join(format!("{id}-{ts}"));
            if let Some(parent) = trash.parent() {
                std::fs::create_dir_all(parent)?;
            }
            match std::fs::rename(&resolved_dir, &trash) {
                Ok(()) => crate::log!("[Gateway] '{}' moved to {}", id, trash.display()),
                Err(e) => crate::log!(
                    "[Gateway] '{}' unregistered but dir move failed ({}); left at {}",
                    id,
                    e,
                    resolved_dir.display()
                ),
            }
        }
        Ok(())
    }

    /// Respawn one stack's engine in place: stop the old process, re-ensure its
    /// shared-cluster database, spawn fresh. Records the error (and increments
    /// the attempt counter) on failure rather than panicking.
    async fn respawn_stack(&self, s: &mut StackRuntime) {
        stop_engine_process(s);
        s.last_spawn = Some(Instant::now());
        s.restart_attempts += 1;
        // The respawn acts on the accumulated misses — clear them so the fresh
        // engine starts its boot window with a clean consecutive-miss count.
        s.health_misses = 0;

        self.set_boot_phase(&s.ws.id, BootPhase::ProvisioningDatabase);
        let prov = match self.ensure_postgres(&s.ws).await {
            Ok(p) => p,
            Err(e) => {
                s.last_error = Some(format!("postgres: {e}"));
                return;
            }
        };
        s.pg = prov.handle;

        match stack::spawn_engine(
            &self.inner.engine_bin,
            &s.ws,
            &s.resolved_dir,
            &prov.database_url,
            self.inner.gateway_port,
            self.inner.engine_loopback,
        ) {
            Ok(child) => {
                s.engine = Some(child);
                s.health = Health::Booting;
                s.last_error = None;
                self.set_boot_phase(&s.ws.id, BootPhase::StartingEngine);
                crate::log!(
                    "[Gateway] respawned '{}' (attempt {})",
                    s.ws.id,
                    s.restart_attempts
                );
            }
            Err(e) => {
                s.last_error = Some(format!("spawn: {e}"));
                crate::log!("[Gateway] respawn of '{}' failed: {}", s.ws.id, e);
            }
        }
    }

    /// One supervision pass over every stack. Process liveness vs. health: a
    /// freshly-spawned engine whose PROCESS is alive but not yet answering
    /// `/api/v1/health` is still BOOTING (cold boot can take tens of seconds)
    /// and is left alone within [`BOOT_GRACE`]; a dead process (crash) or one
    /// wedged past the grace is respawned (backoff + cap).
    async fn supervise_once(&self) {
        let stacks: Vec<Arc<AsyncMutex<StackRuntime>>> =
            { self.inner.stacks.lock().await.values().cloned().collect() };

        // Snapshot phase: capture each candidate stack's probe inputs under a
        // BRIEF lock, then RELEASE it. The probe must not run with the stack lock
        // held — `list_status` (the picker's 2s poll) and create/delete/restart
        // take that same per-stack lock, so holding it across the up-to-5s probe
        // stalls the picker for seconds whenever an engine is briefly slow to
        // answer. The `StackRuntime` mutex protects in-memory state, not the
        // network round-trip.
        let mut candidates: Vec<ProbeTarget> = Vec::with_capacity(stacks.len());
        for stack in stacks {
            let (id, port, last_spawn) = {
                let s = stack.lock().await;
                // A stack capped as Unhealthy is left for manual retry/delete.
                if s.health == Health::Unhealthy {
                    continue;
                }
                (s.ws.id.clone(), s.ws.port, s.last_spawn)
            };
            // A concurrent delete may have removed this stack from the map (and
            // trashed its dir) while we held only the snapshot Arc. Don't
            // resurrect a deleted workspace's engine + Postgres — skip it.
            if !self.inner.stacks.lock().await.contains_key(&id) {
                continue;
            }
            candidates.push(ProbeTarget {
                stack,
                id,
                port,
                last_spawn,
            });
        }

        // Probe phase: run every candidate's health probe CONCURRENTLY with no
        // stack lock held, so one slow engine never serializes the whole pass.
        let scheme = self.engine_scheme();
        let client = &self.inner.health_client;
        // Probe health and, for an engine that answers healthy, its unread count
        // in the same concurrent pass (the count fetch only runs on a healthy
        // probe, so unhealthy/booting engines cost no extra request). This is the
        // sole unread-count path — the gateway holds no DB handle (ADR 0014 §1) —
        // so a stopped workspace yields `None` and shows no badge.
        let outcomes: Vec<(ProbeOutcome, Option<u64>)> = futures::future::join_all(
            candidates.iter().map(|t| async move {
                let outcome = stack::probe_health(client, scheme, t.port).await;
                let unread = if outcome == ProbeOutcome::Healthy {
                    stack::fetch_unread_count(client, scheme, t.port).await
                } else {
                    None
                };
                (outcome, unread)
            }),
        )
        .await;

        // Apply phase: re-acquire each stack briefly to write the result back.
        for (t, (outcome, unread)) in candidates.into_iter().zip(outcomes) {
            let mut s = t.stack.lock().await;
            // The lock was dropped across the probe, so the stack may have changed
            // under us. Discard a stale result rather than apply it to a
            // stopped/deleted/respawned/capped stack (see `probe_result_is_stale`).
            // `contains_key` is checked WHILE holding the stack lock, matching the
            // delete path's ordering so a concurrent delete is observed as absent.
            let present = self.inner.stacks.lock().await.contains_key(&t.id);
            if probe_result_is_stale(present, t.last_spawn, s.last_spawn, s.health) {
                continue;
            }
            if outcome == ProbeOutcome::Healthy {
                s.health = Health::Healthy;
                s.restart_attempts = 0;
                s.health_misses = 0;
                s.last_error = None;
                // Refresh the per-workspace badge count (None if the fetch failed
                // even though health passed — show no badge rather than a stale one).
                s.last_unread = unread;
                // Booted — drop the boot phase so a later cold open starts clean.
                self.clear_boot_phase(&t.id);
                continue;
            }

            // Not healthy → no trustworthy count; clear the badge for this tick.
            s.last_unread = None;

            let since_spawn = s.last_spawn.map(|t| t.elapsed()).unwrap_or(Duration::MAX);
            let alive = engine_process_alive(&mut s);
            // Count this miss; a healthy probe (above) resets it. Only a DEAD
            // process is ever culled (after DEAD_MISS_THRESHOLD misses); an
            // alive engine is never respawned no matter how many it misses, so a
            // load spike can't cull a working engine (ADR 0014, 2026-06-27).
            s.health_misses = s.health_misses.saturating_add(1);

            match respawn_decision(
                outcome,
                alive,
                since_spawn,
                s.health_misses,
                s.restart_attempts,
            ) {
                // Healthy is handled above; treat defensively as a no-op.
                SuperviseAction::Healthy => {}
                SuperviseAction::Booting => s.health = Health::Booting,
                // Within backoff or below the miss threshold — keep accumulating.
                SuperviseAction::Wait => {}
                SuperviseAction::MarkUnhealthy => {
                    s.health = Health::Unhealthy;
                    // Gave up auto-respawning — drop the phase; the last label
                    // would otherwise lie ("Downloading memory model…" on a dead
                    // engine). The splash falls back to the neutral default.
                    self.clear_boot_phase(&t.id);
                    if s.last_error.is_none() {
                        s.last_error = Some(
                            "engine failed to become healthy after repeated restarts".to_string(),
                        );
                    }
                    crate::log!(
                        "[Gateway] '{}' marked unhealthy after {} restarts",
                        t.id,
                        s.restart_attempts
                    );
                }
                SuperviseAction::Respawn => {
                    crate::log!(
                        "[Gateway] respawning '{}' after {} missed probe(s) (outcome={:?}, alive={})",
                        t.id,
                        s.health_misses,
                        outcome,
                        alive
                    );
                    self.respawn_stack(&mut s).await;
                }
            }
        }
    }
}

/// One stack's probe inputs, snapshotted under a brief lock so the health probe
/// itself runs WITHOUT the stack lock held (see `supervise_once`). `last_spawn`
/// is the generation marker used to discard a result whose engine was respawned
/// during the unlocked probe window.
struct ProbeTarget {
    stack: Arc<AsyncMutex<StackRuntime>>,
    id: String,
    port: u16,
    last_spawn: Option<Instant>,
}

/// Whether a health-probe result must be DISCARDED rather than applied to its
/// stack. `supervise_once` releases the stack lock across the (up-to-5s) network
/// probe so the picker's `list_status` never blocks behind it; on re-lock the
/// stack may have changed under us. Discard if:
///   * the stack was removed from the map by a concurrent stop/delete
///     (`present == false`) — never resurrect a stopped/deleted engine;
///   * the stack was respawned/restarted during the probe (`last_spawn` moved) —
///     the probe described the OLD engine, so judging the fresh one off it would
///     bounce a just-restarted workspace;
///   * the stack is now capped `Unhealthy` — left for manual retry/delete.
///
/// `None == None` (a re-adopted engine that has never respawned) counts as
/// "unchanged", so its healthy probes still apply.
fn probe_result_is_stale(
    present: bool,
    last_spawn_before: Option<Instant>,
    last_spawn_now: Option<Instant>,
    health: Health,
) -> bool {
    !present || last_spawn_before != last_spawn_now || health == Health::Unhealthy
}

/// What the supervisor should do with one stack after a single health probe.
/// Output of the pure [`respawn_decision`] so the cull policy is unit-testable
/// in isolation from the async probe + process plumbing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SuperviseAction {
    /// Probe succeeded — mark healthy, reset counters. (Handled before the fn is
    /// called; returned only for completeness.)
    Healthy,
    /// Process alive, still inside its cold-boot window — leave it alone.
    Booting,
    /// Not enough evidence to cull yet (within backoff, or below the miss
    /// threshold for this outcome) — wait and keep accumulating.
    Wait,
    /// Cull and respawn the engine.
    Respawn,
    /// Hit the restart cap — mark Unhealthy and stop auto-respawning.
    MarkUnhealthy,
}

/// Pure cull/keep decision for one stack, given a probe outcome and the stack's
/// current state. Extracted from [`GatewayState::supervise_once`] so the policy
/// is exhaustively unit-tested.
///
/// **An ALIVE engine is never culled** — booting, busy, `Slow`, or even
/// `Unreachable`-while-alive (apparently wedged). Respawning a live engine
/// interrupts its in-flight threads and, under sustained contention, feeds a
/// cross-workspace respawn cascade: each respawn cold-boots (replaying ~14k
/// trigger events + a worktree rescan), which spikes load and starves the next
/// engine's probe. A timed-out HTTP probe cannot distinguish "hung forever" from
/// "busy right now", so the supervisor stops guessing and respawns ONLY a
/// process that has actually exited (`alive == false`) — preserving crash
/// recovery + lazy-start. A genuinely deadlocked-but-alive engine is the rare,
/// accepted cost: it waits for a manual restart. See ADR 0014 (2026-06-27
/// sub-addendum). `misses` is the consecutive-miss count INCLUDING the current
/// probe.
fn respawn_decision(
    outcome: ProbeOutcome,
    alive: bool,
    since_spawn: Duration,
    misses: u32,
    restart_attempts: u32,
) -> SuperviseAction {
    if outcome == ProbeOutcome::Healthy {
        return SuperviseAction::Healthy;
    }
    // Never cull an alive process. Inside the cold-boot window it's BOOTING
    // (pgvector init / migrations / embedding warmup take tens of seconds, shown
    // as the boot splash); past it, it's busy/slow/wedged — leave it be.
    if alive {
        return if since_spawn < BOOT_GRACE {
            SuperviseAction::Booting
        } else {
            SuperviseAction::Wait
        };
    }
    // The process has EXITED — crash recovery. Respawn with backoff + cap.
    if since_spawn < RESPAWN_BACKOFF {
        return SuperviseAction::Wait;
    }
    if misses < DEAD_MISS_THRESHOLD {
        return SuperviseAction::Wait;
    }
    if restart_attempts >= RESTART_CAP {
        return SuperviseAction::MarkUnhealthy;
    }
    SuperviseAction::Respawn
}

/// Whether a stack's engine process is currently alive. For an engine this
/// gateway spawned we `try_wait` the `Child`; for a re-adopted one (no `Child`
/// handle) we signal-probe the pidfile pid.
fn engine_process_alive(s: &mut StackRuntime) -> bool {
    if let Some(child) = s.engine.as_mut() {
        return matches!(child.try_wait(), Ok(None));
    }
    match stack::read_pidfile(&s.resolved_dir) {
        #[cfg(unix)]
        // SAFETY: signal 0 performs existence/permission checks without
        // delivering a signal; returns 0 iff the process exists.
        Some(pid) => unsafe { libc::kill(pid as libc::pid_t, 0) == 0 },
        #[cfg(not(unix))]
        Some(_) => true,
        None => false,
    }
}

/// Stop a stack's engine process (SIGUSR1 — the engine ignores SIGTERM), reaping
/// it off-thread so the supervisor isn't blocked by the engine's graceful drain.
fn stop_engine_process(s: &mut StackRuntime) {
    match s.engine.take() {
        Some(mut child) => {
            // If the child already exited, do NOT signal its PID — the OS may
            // have recycled it to an unrelated process. Just drop it.
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            #[cfg(unix)]
            // SAFETY: signalling a still-running child pid; ESRCH if it just died.
            unsafe {
                libc::kill(child.id() as libc::pid_t, libc::SIGUSR1);
            }
            // Reap without blocking the supervisor (graceful drain can take ~10s).
            tokio::task::spawn_blocking(move || {
                let _ = child.wait();
            });
        }
        None => stack::reclaim_stale_engine(&s.resolved_dir),
    }
}

// ── Entry point ──────────────────────────────────────────────────────────────

/// `lucidos-gateway` entry point.
pub async fn run() -> Result<(), BoxError> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls CryptoProvider");
    let _ = dotenvy::dotenv();
    raise_fd_limit();

    let app_data = resolve_app_data()?;
    std::fs::create_dir_all(&app_data)?;
    let registry_path = app_data.join("config/workspaces.json");
    let gateway_port = std::env::var("LUCIDOS_API_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(DEFAULT_GATEWAY_PORT);
    let engine_bin = std::env::var_os("LUCIDOS_ENGINE_BIN")
        .map(PathBuf::from)
        .ok_or("LUCIDOS_ENGINE_BIN must point at the lucidos-engine binary")?;
    let static_dir = std::env::var_os("LUCIDOS_STATIC_DIR").map(PathBuf::from);
    // Engines bind loopback-only by default (packaged security posture); dev sets
    // `LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0` so the engine is reachable directly on
    // its user-facing port too (ADR 0014 "Dev runtime topology").
    let engine_loopback = !matches!(
        std::env::var("LUCIDOS_GATEWAY_ENGINE_LOOPBACK")
            .unwrap_or_default()
            .trim(),
        "0" | "false" | "no" | "off"
    );
    // A dev engine (non-loopback) keeps the inherited TLS cert and serves https
    // on its own port for direct access (ADR 0014 §4), so the gateway must proxy
    // + probe it over https. A packaged engine serves plain http on loopback.
    let engine_tls = !engine_loopback && std::env::var_os("LUCIDOS_TLS_CERT").is_some();
    // The gateway fronts every workspace API and its own destructive control
    // plane. Resolve its bind with a loopback-first precedence (see
    // `net_config`): an explicit `LUCIDOS_GATEWAY_BIND_ADDR` → `LUCIDOS_GATEWAY_
    // BIND_ALL` → the machine-global `~/.lucidos/network.toml` `[gateway] bind` →
    // loopback. Default with nothing set is loopback-only so a packaged/default
    // launch is never exposed to the LAN; a malformed value fails safe to
    // loopback, never to all-interfaces.
    let network = net_config::read_network_toml();
    let gateway_bind_addr_env = std::env::var("LUCIDOS_GATEWAY_BIND_ADDR").ok();
    let gateway_bind_all_env = std::env::var("LUCIDOS_GATEWAY_BIND_ALL").ok();
    let gateway_bind_choice = net_config::resolve_gateway_bind(
        gateway_bind_addr_env.as_deref(),
        gateway_bind_all_env.as_deref(),
        network.gateway_bind.as_deref(),
    );
    let pg_backend = PgBackend::from_env()?;
    // Packaged desktop runtime sets `LUCIDOS_PACKAGED=1` (mirrors its
    // `LUCIDOS_BOOT_WITHOUT_PROVIDER=1`); dev leaves it unset. Drives the picker's
    // dev-only self-reload control gating.
    let packaged = matches!(
        std::env::var("LUCIDOS_PACKAGED").unwrap_or_default().trim(),
        "1" | "true" | "yes" | "on"
    );

    crate::log!("[Gateway] Lucidos workspace gateway starting...");
    crate::log!("[Gateway] app-data: {}", app_data.display());
    crate::log!("[Gateway] registry: {}", registry_path.display());
    crate::log!("[Gateway] engine binary: {}", engine_bin.display());
    crate::log!(
        "[Gateway] static dir: {}",
        static_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none — picker unavailable)".to_string())
    );
    crate::log!(
        "[Gateway] engine bind: {} ({})",
        if engine_loopback {
            "loopback-only"
        } else {
            "all interfaces (dev — direct access)"
        },
        if engine_tls { "https" } else { "http" }
    );
    crate::log!(
        "[Gateway] gateway bind: {}",
        net_config::bind_scope_label(&gateway_bind_choice)
    );
    crate::log!("[Gateway] postgres backend: {:?}", pg_backend);

    let registry = Registry::load(&registry_path)?;
    let state = GatewayState {
        inner: Arc::new(GatewayInner {
            app_data,
            registry_path,
            gateway_port,
            engine_bin,
            static_dir,
            engine_loopback,
            engine_tls,
            pg_backend,
            packaged,
            pg_lock: AsyncMutex::new(()),
            proxy_client: proxy::build_client(),
            health_client: stack::build_health_client(),
            registry: Mutex::new(registry),
            stacks: AsyncMutex::new(HashMap::new()),
            starting: AsyncMutex::new(HashSet::new()),
            routes: RwLock::new(HashMap::new()),
            boot_phases: RwLock::new(HashMap::new()),
            restore: RwLock::new(RestoreStatus::default()),
            exe_path: std::env::current_exe().ok(),
            update_check: Mutex::new(UpdateCheck::default()),
        }),
    };
    crate::log!("[Gateway] build id: {}", GATEWAY_BUILD_ID);

    // Re-adopt running engines and spawn the auto-start workspaces (ADR 0014).
    // A first-run empty registry is a no-op here: nothing is auto-created, so the
    // smart root serves the picker and the user names their first workspace
    // instead of being dropped into a pre-made `default`.
    state.boot_all().await;

    // Supervisor.
    {
        let sup = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(SUPERVISE_INTERVAL).await;
                sup.supervise_once().await;
            }
        });
    }

    serve(state, gateway_port, gateway_bind_choice).await
}

/// Build the gateway router and serve it (TLS when certs are configured, like
/// the engine). `/~/api/v1/health` + `/~/api/v1/control/*` are the gateway's
/// own; every other path falls through to [`fallback`] (smart root, picker
/// static under `/~/`, else proxy `/<slug>/*`).
async fn serve(
    state: GatewayState,
    port: u16,
    bind_choice: net_config::BindChoice,
) -> Result<(), BoxError> {
    let router = Router::new()
        .route("/~/api/v1/health", get(gateway_health))
        .nest("/~/api/v1/control", crate::control::router())
        .fallback(fallback)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ))
        .with_state(state);
    let router = if permissive_cors_enabled() {
        crate::log!("[Gateway] permissive CORS enabled by LUCIDOS_PERMISSIVE_CORS");
        router.layer(CorsLayer::permissive())
    } else {
        router
    };

    // Every address to listen on. A specific `Address` ALSO binds loopback (see
    // `net_config::bind_socket_addrs`) so the dev launch scripts' control POSTs
    // and each spawned engine's Apply-restart callback — both over `127.0.0.1` —
    // keep reaching the gateway. `addr` (the primary) is only for the log.
    let addrs = net_config::bind_socket_addrs(&bind_choice, port);
    let addr = addrs[0];
    let handle = axum_server::Handle::new();
    install_shutdown(handle.clone());

    let tls_cert = std::env::var("LUCIDOS_TLS_CERT").ok();
    let tls_key = std::env::var("LUCIDOS_TLS_KEY").ok();
    // Serve every resolved address concurrently under the one shared shutdown
    // `Handle`; a bind failure on any address fails fast.
    if let (Some(cert), Some(key)) = (tls_cert, tls_key) {
        let cfg = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key).await?;
        crate::log!("[Gateway] listening on https://{} (TLS)", addr);
        futures::future::try_join_all(addrs.into_iter().map(|a| {
            axum_server::bind_rustls(a, cfg.clone())
                .handle(handle.clone())
                .serve(router.clone().into_make_service())
        }))
        .await?;
    } else {
        crate::log!("[Gateway] listening on http://{}", addr);
        futures::future::try_join_all(addrs.into_iter().map(|a| {
            axum_server::bind(a)
                .handle(handle.clone())
                .serve(router.clone().into_make_service())
        }))
        .await?;
    }
    Ok(())
}

fn permissive_cors_enabled() -> bool {
    permissive_cors_enabled_value(std::env::var("LUCIDOS_PERMISSIVE_CORS").ok().as_deref())
}

fn permissive_cors_enabled_value(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("1" | "true" | "yes" | "on"))
}

/// Gateway-own health (`/~/api/v1/health`). The launcher polls this.
async fn gateway_health(State(state): State<GatewayState>) -> axum::Json<serde_json::Value> {
    let count = state.inner.routes.read().map(|r| r.len()).unwrap_or(0);
    axum::Json(serde_json::json!({
        "status": "ok",
        "role": "gateway",
        "release": crate::LUCIDOS_RELEASE,
        "workspaces": count,
    }))
}

/// Everything not handled by a gateway route:
///   * `/`            — smart root (redirect to a sole workspace, else picker).
///   * `/~/…`         — picker assets / picker shell (sigil namespace).
///   * `/<slug>/…`    — proxy to the matching engine.
async fn fallback(State(state): State<GatewayState>, req: axum::extract::Request) -> Response {
    let path = req.uri().path().to_string();

    if path == "/" {
        // Smart root: exactly one workspace → drop the user straight in.
        if let Some(slug) = state.sole_workspace() {
            return redirect(&format!("/{slug}/"));
        }
        return serve_picker_shell(&state);
    }

    // Gateway-owned sigil namespace: picker shell + its bundled assets.
    if path == format!("/{SIGIL}") {
        return redirect(&format!("/{SIGIL}/"));
    }
    if let Some(rest) = path.strip_prefix(&format!("/{SIGIL}/")) {
        return serve_sigil(&state, rest, req).await;
    }

    // `/<slug>/…` → proxy to that workspace's engine. The one exception is the
    // manifest: a gateway-fronted workspace install must cover the whole
    // gateway origin, not only `/<slug>/`, or switching workspaces leaves the
    // installed PWA's scope and the browser opens a separate browser context.
    let slug = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or("")
        .to_string();
    let rest = path
        .trim_start_matches('/')
        .split_once('/')
        .map(|(_, rest)| rest)
        .unwrap_or("");
    match state.route(&slug) {
        Some(port) => {
            if rest == "manifest.json" {
                return serve_workspace_manifest(&state, &slug);
            }
            // A document navigation to a workspace stuck in its boot window past
            // the budget ([`BOOT_ESCAPE_BUDGET`]) escapes to the manual "Back to
            // workspaces" page instead of the forever-refreshing starting splash —
            // an alive-but-unreachable engine (e.g. a misconfigured bind) is never
            // marked `Unhealthy`, so the time budget is the only honest signal.
            // Non-document traffic (API/SSE/assets from an already-open tab) still
            // proxies; the frontend owns its own disconnected recovery.
            if is_document_navigation(&req) && boot_window_stalled(state.boot_elapsed(&slug)) {
                return proxy::stalled_page();
            }
            let target = format!("{}://127.0.0.1:{port}", state.engine_scheme());
            // The route is set the instant `bring_up` spawns the engine — while
            // it's still Booting — so a cold-open navigation lands HERE, not on
            // the no-route branch below. Pass the current boot phase so the
            // proxy's connect-failure splash narrates the engine-reported phases
            // (migrating → downloading memory model → recovering); a transient
            // restart of a live workspace simply has no phase set (default).
            let boot_label = state.boot_phase_label(&slug);
            proxy::proxy(&state.inner.proxy_client, &target, &slug, boot_label, req).await
        }
        None => {
            // No live route. If the slug is a registered-but-stopped workspace
            // (membership is "all ever launched"; an autostart=false / stopped
            // workspace has no stack), lazy-start it for a document navigation
            // and serve the boot window — the page's auto-refresh lands once the
            // engine is healthy and the route exists. Do NOT lazy-start on API,
            // SSE, asset, or service-worker retry traffic from an already-open
            // tab; otherwise the picker's Stop button would shut the engine down
            // only for the stopped app to immediately resurrect itself.
            let registered = state.inner.registry.lock().unwrap().contains(&slug);
            if registered {
                if rest == "manifest.json" {
                    return serve_workspace_manifest(&state, &slug);
                }
                if is_document_navigation(&req) {
                    // Kick the lazy-start in the background and return the boot
                    // window immediately — don't block this response on a
                    // multi-second provision+spawn; the page's auto-refresh lands
                    // once the engine is healthy. lazy_start is self-guarded
                    // against duplicate starts.
                    let st = state.clone();
                    let id = slug.clone();
                    tokio::spawn(async move { st.lazy_start(&id).await });
                    // Label reflects the current boot phase (default until the
                    // background lazy-start records one); the 2s meta-refresh
                    // picks up later phases.
                    return proxy::starting_page(state.boot_phase_label(&slug));
                }
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!("workspace '{slug}' is stopped"),
                )
                    .into_response();
            }
            // Unknown (never-registered / deleted) slug. On a document navigation,
            // send the browser to the picker list (`/~/?pick`) rather than a raw
            // 404 dead-end: the PWA cold-start head redirect (index.html) opens
            // the remembered workspace optimistically without an existence check,
            // so a since-deleted last-workspace must land somewhere recoverable.
            // `?pick` also makes that head redirect stand down, so the picker
            // renders its list (and forgets the stale workspace) instead of
            // bouncing straight back here — no redirect loop. Non-document traffic
            // (API/SSE/assets/SW retries) still gets a 404.
            if is_document_navigation(&req) {
                return redirect(&format!("/{SIGIL}/?pick"));
            }
            (StatusCode::NOT_FOUND, format!("unknown workspace '{slug}'")).into_response()
        }
    }
}

/// True for a browser document navigation that should wake a stopped workspace.
/// Background traffic from an already-open app tab (API fetches, SSE reconnects,
/// service-worker requests, assets) must not wake it, or "Stop" cannot stick.
fn is_document_navigation(req: &axum::extract::Request) -> bool {
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return false;
    }

    let headers = req.headers();
    let header_eq = |name: &str, expected: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.eq_ignore_ascii_case(expected))
            .unwrap_or(false)
    };
    if header_eq("sec-fetch-mode", "navigate") || header_eq("sec-fetch-dest", "document") {
        return true;
    }

    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|accept| {
            accept.split(',').any(|part| {
                matches!(
                    part.trim().split(';').next(),
                    Some("text/html" | "application/xhtml+xml")
                )
            })
        })
        .unwrap_or(false)
}

/// Serve a path under the sigil namespace (`rest` is the path AFTER `/~/`). A
/// real bundled asset is streamed from `static_dir`; anything else (the picker's
/// own SPA routes, `/~/`) falls back to the picker shell.
async fn serve_sigil(state: &GatewayState, rest: &str, req: axum::extract::Request) -> Response {
    let Some(dir) = state.inner.static_dir.clone() else {
        return (StatusCode::NOT_FOUND, "no frontend configured").into_response();
    };
    // `/~/` and an explicit `/~/index.html` → the picker shell (with `<base
    // href="/~/">` stamped). Serving the raw `dist/index.html` for the latter
    // would carry no base tag, so the bundle would render the app, not the
    // picker — mirror the engine's `serve_frontend` index special-case.
    if rest.is_empty() || rest == "index.html" {
        return serve_picker_shell(state);
    }
    // The PWA manifest needs a picker-specific `scope`/`start_url` re-stamp so the
    // installed picker keeps workspace navigation in-app (see
    // `serve_picker_manifest`); it must NOT be served verbatim from `dist/`.
    if rest == "manifest.json" {
        return serve_picker_manifest(state);
    }
    // Reconstruct the request with the sigil stripped so ServeDir resolves the
    // asset against `static_dir` (e.g. `/~/assets/x.js` → `/assets/x.js`).
    let query = req.uri().query();
    let stripped = match query {
        Some(q) => format!("/{rest}?{q}"),
        None => format!("/{rest}"),
    };
    let (mut parts, body) = req.into_parts();
    parts.uri = match Uri::try_from(stripped) {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad path").into_response(),
    };
    let asset_req = axum::extract::Request::from_parts(parts, body);

    let service = ServeDir::new(&dir);
    match service.oneshot(asset_req).await {
        Ok(resp) if resp.status() != StatusCode::NOT_FOUND => resp.map(Body::new),
        // No such asset → the picker is a SPA; serve its shell.
        _ => serve_picker_shell(state),
    }
}

/// Serve the picker shell: `index.html` from `static_dir` with `<base
/// href="/~/">` stamped in, so the bundle's relative asset refs resolve under
/// the sigil namespace (and `main.tsx` recognises the picker context).
fn serve_picker_shell(state: &GatewayState) -> Response {
    let Some(dir) = state.inner.static_dir.as_ref() else {
        return (StatusCode::NOT_FOUND, "no frontend configured").into_response();
    };
    let index = dir.join("index.html");
    let html = match std::fs::read_to_string(&index) {
        Ok(h) => h,
        Err(e) => {
            crate::log!(
                "[Gateway] picker index read failed ({}): {}",
                index.display(),
                e
            );
            return (StatusCode::INTERNAL_SERVER_ERROR, "picker unavailable").into_response();
        }
    };
    let stamped = inject_base_href(&html, &format!("/{SIGIL}/"));
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        stamped,
    )
        .into_response()
}

/// Serve the picker's PWA manifest (`/~/manifest.json`), re-stamped from the
/// bundled `dist/manifest.json` so the installed picker keeps workspace
/// navigation inside the standalone PWA.
///
/// The bundled manifest declares `start_url`/`scope` as `"."`. In the picker
/// context, that would resolve the installed PWA's scope to `/~/` alone, so
/// tapping a workspace (`/<slug>/`) would navigate out of scope and open a
/// browser instead of staying inside the standalone PWA.
fn serve_picker_manifest(state: &GatewayState) -> Response {
    serve_gateway_manifest(state, &format!("/{SIGIL}/"), &format!("/{SIGIL}/"))
}

/// Serve a gateway-fronted workspace's PWA manifest (`/<slug>/manifest.json`).
/// Direct engine access keeps the bundled relative manifest and therefore a
/// per-workspace scope; gateway access widens `scope` to `/` so in-app workspace
/// switches stay inside the PWA installed from the gateway's stable port.
fn serve_workspace_manifest(state: &GatewayState, slug: &str) -> Response {
    let workspace_url = format!("/{slug}/");
    serve_gateway_manifest(state, &workspace_url, &workspace_url)
}

fn serve_gateway_manifest(state: &GatewayState, start_url: &str, id: &str) -> Response {
    let Some(dir) = state.inner.static_dir.as_ref() else {
        return (StatusCode::NOT_FOUND, "no frontend configured").into_response();
    };
    let path = dir.join("manifest.json");
    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            // Degrade to a minimal valid manifest carrying the gateway scope —
            // a missing manifest would otherwise lose the in-app-navigation fix.
            crate::log!("[Gateway] manifest read failed ({}): {}", path.display(), e);
            String::new()
        }
    };
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        gateway_manifest_json(&source, start_url, id),
    )
        .into_response()
}

/// Re-stamp a bundled manifest for a GATEWAY-served install: force `scope` to
/// `/` so the installed PWA covers the picker and every `/<slug>/` workspace,
/// keeping workspace navigation in-app. `start_url` + `id` are supplied by the
/// caller: picker installs start at `/~/`, workspace installs start at their
/// workspace. Every other field (name, icons, theme) is preserved from the
/// source; relative icon refs stay relative to the manifest URL. A malformed or
/// empty source degrades to a minimal manifest carrying the gateway scope.
fn gateway_manifest_json(source: &str, start_url: &str, id: &str) -> String {
    let mut manifest = serde_json::from_str::<serde_json::Value>(source)
        .ok()
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    // Safe: `manifest` is an object by construction (parsed object or `{}`).
    let obj = manifest.as_object_mut().expect("manifest is an object");
    obj.insert("start_url".to_string(), serde_json::json!(start_url));
    obj.insert("scope".to_string(), serde_json::json!("/"));
    obj.insert("id".to_string(), serde_json::json!(id));
    manifest.to_string()
}

/// A `307 Temporary Redirect` to `location`.
fn redirect(location: &str) -> Response {
    Response::builder()
        .status(StatusCode::TEMPORARY_REDIRECT)
        .header(header::LOCATION, location)
        .body(Body::empty())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Insert `<base href="…">` as the first child of `<head>` so every relative
/// ref in the document resolves against it. Falls back to prepending if there
/// is no `<head>`. Mirrors the engine's own stamping (duplicated, ADR 0014 §1).
pub fn inject_base_href(html: &str, href: &str) -> String {
    let tag = format!("<base href=\"{href}\">");
    if let Some(pos) = find_head_open_end(html) {
        let mut out = String::with_capacity(html.len() + tag.len());
        out.push_str(&html[..pos]);
        out.push_str(&tag);
        out.push_str(&html[pos..]);
        out
    } else {
        format!("{tag}{html}")
    }
}

/// Byte offset just past the opening `<head …>` tag, case-insensitively.
fn find_head_open_end(html: &str) -> Option<usize> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<head")?;
    // Find the '>' that closes the opening tag.
    let close = lower[start..].find('>')? + start;
    Some(close + 1)
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Resolve the gateway's base dir: `LUCIDOS_GATEWAY_DATA` wins; else
/// `~/.lucidos/gateway` (dev). Packaged sets it to the OS app-data dir.
fn resolve_app_data() -> Result<PathBuf, BoxError> {
    if let Some(d) = std::env::var_os("LUCIDOS_GATEWAY_DATA") {
        return Ok(PathBuf::from(d));
    }
    let home = std::env::var_os("HOME").ok_or("HOME not set")?;
    Ok(PathBuf::from(home).join(".lucidos/gateway"))
}

/// Install graceful-stop handlers. The gateway exits on Ctrl+C / SIGUSR1 but
/// leaves the detached workspace engines running so a relaunch re-adopts them
/// (engine-statelessness). SIGTERM is ignored (same rationale as the engine).
fn install_shutdown(handle: axum_server::Handle) {
    tokio::spawn(async move {
        let ctrl_c = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        #[cfg(unix)]
        let usr1 = async {
            if let Ok(mut s) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1())
            {
                s.recv().await;
            }
        };
        #[cfg(not(unix))]
        let usr1 = std::future::pending::<()>();

        #[cfg(unix)]
        tokio::spawn(async {
            if let Ok(mut sigterm) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                loop {
                    sigterm.recv().await;
                    crate::log!("[Gateway] SIGTERM ignored — use SIGUSR1 to stop");
                }
            }
        });

        tokio::select! {
            _ = ctrl_c => {},
            _ = usr1 => {},
        }
        crate::log!("[Gateway] shutting down (workspace engines left running for re-adoption)");
        handle.graceful_shutdown(Some(Duration::from_secs(3)));
    });
}

/// Raise the file-descriptor limit — the gateway holds an inbound + outbound
/// socket per proxied connection, and SSE streams are long-lived.
fn raise_fd_limit() {
    #[cfg(unix)]
    {
        use std::mem::MaybeUninit;
        // SAFETY: getrlimit/setrlimit with a valid resource + initialized struct.
        unsafe {
            let mut rlim = MaybeUninit::<libc::rlimit>::uninit();
            if libc::getrlimit(libc::RLIMIT_NOFILE, rlim.as_mut_ptr()) == 0 {
                let mut rlim = rlim.assume_init();
                if rlim.rlim_cur < 8192 {
                    rlim.rlim_cur = rlim.rlim_max.min(8192);
                    libc::setrlimit(libc::RLIMIT_NOFILE, &rlim);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_base_href_inserts_after_head() {
        let html =
            "<!doctype html><html><head><meta charset=\"utf-8\"></head><body>x</body></html>";
        let out = inject_base_href(html, "/~/");
        assert!(out.contains("<head><base href=\"/~/\"><meta charset=\"utf-8\">"));
    }

    #[test]
    fn inject_base_href_handles_attributes_on_head() {
        let html = "<head data-x=\"1\"><title>t</title></head>";
        let out = inject_base_href(html, "/dev/");
        assert!(out.contains("<head data-x=\"1\"><base href=\"/dev/\"><title>"));
    }

    #[test]
    fn inject_base_href_prepends_when_no_head() {
        let html = "<p>no head here</p>";
        let out = inject_base_href(html, "/~/");
        assert!(out.starts_with("<base href=\"/~/\"><p>"));
    }

    fn request_with_headers(headers: &[(&str, &str)]) -> axum::extract::Request {
        let mut builder = axum::extract::Request::builder()
            .method(Method::GET)
            .uri("/dev/");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn document_navigation_wakes_stopped_workspace() {
        let req = request_with_headers(&[
            ("sec-fetch-mode", "navigate"),
            ("sec-fetch-dest", "document"),
            (header::ACCEPT.as_str(), "text/html,application/xhtml+xml"),
        ]);
        assert!(is_document_navigation(&req));
    }

    #[test]
    fn html_accept_falls_back_for_browsers_without_fetch_metadata() {
        let req = request_with_headers(&[(header::ACCEPT.as_str(), "text/html,*/*;q=0.8")]);
        assert!(is_document_navigation(&req));
    }

    #[test]
    fn api_fetch_does_not_wake_stopped_workspace() {
        let req =
            request_with_headers(&[("sec-fetch-mode", "cors"), (header::ACCEPT.as_str(), "*/*")]);
        assert!(!is_document_navigation(&req));
    }

    #[test]
    fn sse_reconnect_does_not_wake_stopped_workspace() {
        let req = request_with_headers(&[(header::ACCEPT.as_str(), "text/event-stream")]);
        assert!(!is_document_navigation(&req));
    }

    /// An unknown (deleted/stale) workspace slug on a document navigation must
    /// send the browser to the picker list (`/~/?pick`), not a 404 dead-end — the
    /// PWA cold-start head redirect opens the remembered workspace without an
    /// existence check, and `?pick` makes that head redirect stand down so the
    /// picker renders (no redirect loop). This locks the exact target (SIGIL +
    /// `?pick`) the frontend guard depends on.
    #[test]
    fn unknown_slug_document_nav_redirects_to_picker_list() {
        let resp = redirect(&format!("/{SIGIL}/?pick"));
        assert!(resp.status().is_redirection());
        let location = resp
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap();
        assert_eq!(location, "/~/?pick");
    }

    #[test]
    fn permissive_cors_is_disabled_unless_explicitly_enabled() {
        for value in [None, Some(""), Some("0"), Some("false"), Some("off")] {
            assert!(!permissive_cors_enabled_value(value), "value: {value:?}");
        }
        for value in [Some("1"), Some("true"), Some("yes"), Some("on")] {
            assert!(permissive_cors_enabled_value(value), "value: {value:?}");
        }
    }

    /// The bug: the bundled manifest's relative `scope: "."` would scope the
    /// installed picker PWA to `/~/`, so opening a workspace (`/<slug>/`) left the
    /// scope and iOS opened it in an in-app browser. The picker manifest must
    /// instead scope to `/` (covering every workspace) so navigation stays in-app.
    #[test]
    fn picker_manifest_widens_scope_and_starts_at_picker() {
        // The real bundled shape: relative start_url/scope + relative icons.
        let source = r#"{
            "name": "Lucidos",
            "short_name": "Lucidos",
            "start_url": ".",
            "scope": ".",
            "display": "standalone",
            "icons": [{ "src": "icons/icon-192.png", "sizes": "192x192", "type": "image/png" }]
        }"#;
        let out: serde_json::Value =
            serde_json::from_str(&gateway_manifest_json(source, "/~/", "/~/")).expect("valid JSON");

        // The fix: scope covers the whole origin, start_url + id are the picker.
        assert_eq!(out["scope"], "/");
        assert_eq!(out["start_url"], "/~/");
        assert_eq!(out["id"], "/~/");
        // Everything else is preserved, so the installed picker keeps its
        // name/icons/display. Icons stay relative (resolve against /~/manifest.json).
        assert_eq!(out["name"], "Lucidos");
        assert_eq!(out["display"], "standalone");
        assert_eq!(out["icons"][0]["src"], "icons/icon-192.png");
    }

    #[test]
    fn boot_window_stalled_only_past_budget() {
        // No boot window (healthy / stopped) is never stalled.
        assert!(!boot_window_stalled(None));
        // Within budget → still the auto-refreshing starting splash.
        assert!(!boot_window_stalled(Some(Duration::from_secs(0))));
        assert!(!boot_window_stalled(Some(BOOT_ESCAPE_BUDGET - Duration::from_secs(1))));
        // At/past budget → escape to the manual "Back to workspaces" page.
        assert!(boot_window_stalled(Some(BOOT_ESCAPE_BUDGET)));
        assert!(boot_window_stalled(Some(BOOT_ESCAPE_BUDGET + Duration::from_secs(60))));
    }

    /// Installing from `http(s)://host:<gateway-port>/<workspace>/` must also stay in the
    /// gateway's full-origin scope. Otherwise the workspace switcher navigates
    /// out of the installed PWA and opens a browser.
    #[test]
    fn workspace_manifest_widens_scope_but_launches_workspace() {
        let source = r#"{
            "name": "Lucidos",
            "start_url": ".",
            "scope": ".",
            "display": "standalone"
        }"#;
        let out: serde_json::Value =
            serde_json::from_str(&gateway_manifest_json(source, "/dev/", "/dev/"))
                .expect("valid JSON");

        assert_eq!(out["scope"], "/");
        assert_eq!(out["start_url"], "/dev/");
        assert_eq!(out["id"], "/dev/");
        assert_eq!(out["display"], "standalone");
    }

    #[test]
    fn picker_manifest_degrades_on_unparseable_source() {
        // A missing/garbage manifest must still carry the picker scope so the
        // in-app-navigation fix survives even when the bundle can't be read.
        for source in ["", "not json", "[]", "42"] {
            let out: serde_json::Value =
                serde_json::from_str(&gateway_manifest_json(source, "/~/", "/~/"))
                    .expect("valid JSON");
            assert_eq!(out["scope"], "/", "source: {source:?}");
            assert_eq!(out["start_url"], "/~/", "source: {source:?}");
            assert_eq!(out["id"], "/~/", "source: {source:?}");
        }
    }

    // ── Gateway health-supervisor cull policy (respawn_decision) ───────────
    // The 2026-06-24 respawn storm recurred under sustained contention
    // (2026-06-27): the patient-but-still-culling policy kept respawning
    // alive-but-Slow engines, interrupting threads and feeding the cascade. The
    // policy is now: NEVER cull an alive engine — respawn ONLY a process that has
    // actually exited. These tests pin that contract.

    /// `since_spawn` comfortably past the boot grace → an "established" engine
    /// (also past the respawn backoff, since BOOT_GRACE > RESPAWN_BACKOFF).
    fn established() -> Duration {
        BOOT_GRACE + Duration::from_secs(1)
    }

    #[test]
    fn healthy_probe_keeps_engine() {
        assert_eq!(
            respawn_decision(ProbeOutcome::Healthy, true, established(), 0, 0),
            SuperviseAction::Healthy
        );
    }

    #[test]
    fn alive_within_boot_grace_is_booting_not_culled() {
        // Even with a pile of misses, a live process still cold-booting is spared.
        for outcome in [
            ProbeOutcome::Slow,
            ProbeOutcome::Unreachable,
            ProbeOutcome::Other,
        ] {
            assert_eq!(
                respawn_decision(outcome, true, Duration::from_secs(1), 999, 0),
                SuperviseAction::Booting,
                "outcome={outcome:?}"
            );
        }
    }

    #[test]
    fn within_backoff_waits() {
        // Just (re)spawned, not yet healthy → don't respawn again immediately.
        assert_eq!(
            respawn_decision(
                ProbeOutcome::Unreachable,
                false,
                Duration::from_secs(0),
                99,
                0
            ),
            SuperviseAction::Wait
        );
    }

    #[test]
    fn alive_engine_is_never_culled_regardless_of_outcome_or_misses() {
        // The core contract: an established, alive engine is never respawned —
        // not on one slow probe, not on a long run of misses, whatever the probe
        // outcome (Slow, Unreachable-while-alive, or Other). Respawning a live
        // engine interrupts its threads and feeds the cross-workspace respawn
        // cascade (ADR 0014, 2026-06-27).
        for outcome in [
            ProbeOutcome::Slow,
            ProbeOutcome::Unreachable,
            ProbeOutcome::Other,
        ] {
            for misses in [1, 5, 50, 9999] {
                assert_eq!(
                    respawn_decision(outcome, true, established(), misses, 0),
                    SuperviseAction::Wait,
                    "outcome={outcome:?} misses={misses}"
                );
            }
        }
    }

    #[test]
    fn alive_engine_at_restart_cap_still_waits_never_unhealthy() {
        // Liveness gates everything: an alive engine is left alone even with the
        // restart cap's worth of attempts accumulated — never marked Unhealthy.
        assert_eq!(
            respawn_decision(
                ProbeOutcome::Unreachable,
                true,
                established(),
                9999,
                RESTART_CAP
            ),
            SuperviseAction::Wait
        );
    }

    #[test]
    fn unreachable_engine_culled_promptly() {
        // A refused connection is a strong "down" signal → small threshold.
        assert_eq!(
            respawn_decision(
                ProbeOutcome::Unreachable,
                false,
                established(),
                DEAD_MISS_THRESHOLD - 1,
                0
            ),
            SuperviseAction::Wait
        );
        assert_eq!(
            respawn_decision(
                ProbeOutcome::Unreachable,
                false,
                established(),
                DEAD_MISS_THRESHOLD,
                0
            ),
            SuperviseAction::Respawn
        );
    }

    #[test]
    fn dead_process_uses_the_fast_threshold_even_if_slow() {
        // Probe timed out but the process has EXITED → treat as dead, not busy,
        // so a crash recovers promptly instead of waiting the slow threshold.
        assert_eq!(
            respawn_decision(
                ProbeOutcome::Slow,
                false,
                established(),
                DEAD_MISS_THRESHOLD,
                0
            ),
            SuperviseAction::Respawn
        );
    }

    #[test]
    fn restart_cap_marks_unhealthy_instead_of_respawning() {
        assert_eq!(
            respawn_decision(
                ProbeOutcome::Unreachable,
                false,
                established(),
                DEAD_MISS_THRESHOLD,
                RESTART_CAP
            ),
            SuperviseAction::MarkUnhealthy
        );
    }

    // ── Probe staleness guard (probe_result_is_stale) ──────────────────────
    // The supervisor drops the stack lock across the (up-to-5s) probe so the
    // picker's `list_status` never blocks behind it. On re-lock the stack may
    // have changed under us; these pin which results must be DISCARDED.

    #[test]
    fn removed_stack_result_is_stale() {
        // A concurrent stop/delete removed the stack from the map → never apply
        // (never resurrect a stopped/deleted engine), regardless of generation.
        let t0 = Instant::now();
        assert!(probe_result_is_stale(false, Some(t0), Some(t0), Health::Booting));
        assert!(probe_result_is_stale(false, None, None, Health::Healthy));
    }

    #[test]
    fn respawned_stack_result_is_stale() {
        // `last_spawn` moved during the probe → the probe described the OLD
        // engine; applying it would bounce a just-restarted workspace.
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(1);
        assert!(probe_result_is_stale(true, Some(t0), Some(t1), Health::Booting));
        // A re-adopted engine (None) that respawned during the probe (Some) too.
        assert!(probe_result_is_stale(true, None, Some(t1), Health::Booting));
    }

    #[test]
    fn capped_unhealthy_result_is_stale() {
        // Capped Unhealthy while the probe was in flight → left for manual retry.
        let t0 = Instant::now();
        assert!(probe_result_is_stale(true, Some(t0), Some(t0), Health::Unhealthy));
    }

    #[test]
    fn unchanged_present_result_applies() {
        // Still present, same generation, not capped → apply the probe result.
        let t0 = Instant::now();
        assert!(!probe_result_is_stale(true, Some(t0), Some(t0), Health::Healthy));
        assert!(!probe_result_is_stale(true, Some(t0), Some(t0), Health::Booting));
    }

    #[test]
    fn readopted_engine_unchanged_applies() {
        // A re-adopted engine keeps `last_spawn == None`; None == None counts as
        // "unchanged", so its healthy probes still apply (else it never leaves
        // Booting).
        assert!(!probe_result_is_stale(true, None, None, Health::Booting));
        assert!(!probe_result_is_stale(true, None, None, Health::Healthy));
    }
}
