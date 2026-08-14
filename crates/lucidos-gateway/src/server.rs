//! Gateway state, supervision, lifecycle, and HTTP routing. `fallback` holds
//! the route table (ADR 0014 §2/§3/§10).
//!
//! A workspace slug can never start with the sigil (slugs are `[a-z0-9-]`), so
//! the first path segment is unambiguous with no reserved-word list.

use crate::boot_failure::BootFailure;
use crate::boot_phase::{self, BootPhase};
use crate::error::ApiError;
use crate::net_config;
use crate::next_boot;
use crate::postgres::{self, PgBackend, PgHandle, ProvisionError, ProvisionErrorKind};
use crate::proxy;
use crate::registry::{self, Registry, Workspace, REGISTRY_VERSION, SIGIL};
use crate::stack::{self, Health, ProbeOutcome, StackRuntime, WorkspaceStatus};
use crate::BoxError;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;
use tower::ServiceExt;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

/// The dev default, and only the no-env fallback: both real launch paths pass
/// `LUCIDOS_API_PORT`. Dev injects 5251 and the packaged app 5252, so the two
/// coexist out of the box.
const DEFAULT_GATEWAY_PORT: u16 = 5251;

/// Build id baked in by `build.rs`: git short SHA plus a hash of any
/// uncommitted gateway-source diff. Deterministic for identical source, so a
/// no-op rebuild does not raise the picker's "new gateway available" badge.
pub const GATEWAY_BUILD_ID: &str = env!("GATEWAY_BUILD_ID");

/// In-memory state of the picker's restore-from-backup flow. One slot, so one
/// restore at a time, polled at `GET /~/api/v1/control/restore-status`. Never
/// persisted: a restore that dies with the gateway is gone, and its
/// half-provisioned workspace is cleaned up.
#[derive(Clone, serde::Serialize, Default, Debug, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RestoreStatus {
    #[default]
    Idle,
    /// `phase` mirrors the engine CLI's `LUCIDOS_RESTORE_PHASE` ticks (starting,
    /// restoring, decrypting, decompressing, initializing, restoring_db, done).
    Running {
        id: String,
        name: String,
        phase: String,
    },
    /// Registered, and started on a best-effort basis.
    Completed { id: String, name: String },
    /// Failed before committing, so nothing was registered: cleanup removed the
    /// half-provisioned workspace. `error` is the user-facing message.
    Failed { name: String, error: String },
}

/// Supervisor cadence + thresholds.
const SUPERVISE_INTERVAL: Duration = Duration::from_secs(2);
/// How long a freshly-(re)spawned engine whose PROCESS is alive has to answer
/// `/api/v1/health` before it counts as wedged. Cold boot runs pgvector init,
/// migrations and embedding warmup, so a still-booting engine must NOT be
/// respawned out from under itself.
const BOOT_GRACE: Duration = Duration::from_secs(120);
/// Minimum gap between respawn attempts for one stack, and the gap before the
/// FIRST retry (see [`respawn_backoff`]).
const RESPAWN_BACKOFF: Duration = Duration::from_secs(5);
/// Ceiling on the grown [`respawn_backoff`], so a workspace waiting out a long
/// external outage still re-checks about once a minute.
const RESPAWN_BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Auto-respawn attempts (since last healthy) before a stack is marked
/// unhealthy. Counts EVERY bring-up attempt, a failed Postgres provision
/// included: a workspace that cannot get a database is no more startable than
/// one whose engine keeps exiting.
const RESTART_CAP: u32 = 5;
/// Consecutive missed probes before respawning an engine whose process has
/// EXITED. An engine that is still ALIVE is never culled, whatever the probe
/// says (see [`respawn_decision`]), so there is no separate "slow" threshold.
const DEAD_MISS_THRESHOLD: u32 = 2;
/// How long a workspace may sit in the boot window before the splash escapes to
/// the manual "Back to workspaces" page (see [`proxy::stalled_page`]). Must
/// exceed a legitimate slow first-run boot: [`BOOT_GRACE`] plus migrations and
/// recovery. The embedding model is not part of that budget, since it loads in
/// the background. The escape exists because an alive-but-unreachable engine is
/// never marked `Unhealthy`, so without a time budget the splash would
/// meta-refresh forever.
const BOOT_ESCAPE_BUDGET: Duration = Duration::from_secs(240);

/// One workspace's live boot-window state. `since` is when the window OPENED,
/// and is preserved across phase updates so the [`BOOT_ESCAPE_BUDGET`] elapsed
/// time accumulates over the whole episode.
#[derive(Clone, Copy)]
struct BootProgress {
    phase: BootPhase,
    since: Instant,
}

/// Whether a boot window that opened `elapsed` ago has stalled long enough to
/// show the manual escape page instead of the auto-refreshing splash.
fn boot_window_stalled(elapsed: Option<Duration>) -> bool {
    elapsed.is_some_and(|e| e >= BOOT_ESCAPE_BUDGET)
}

/// Sum per-workspace unread counts into the dock-badge total. An unknown count
/// contributes 0, matching the picker's "running workspaces only" rule.
fn sum_unread(counts: impl IntoIterator<Item = Option<u64>>) -> u64 {
    counts.into_iter().flatten().sum()
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
    /// (`LUCIDOS_ENGINE_BIN`): the gateway's own `current_exe` is the gateway,
    /// not the engine (ADR 0014 §1).
    engine_bin: PathBuf,
    /// The built frontend dir (`dist/`) the gateway serves the picker from, and
    /// passes to engines via the inherited env so they serve it too.
    static_dir: Option<PathBuf>,
    /// Whether spawned engines bind loopback-only (packaged) or all interfaces
    /// (dev, so the workspace app is also reachable directly on its port). See
    /// `LUCIDOS_GATEWAY_ENGINE_LOOPBACK` and ADR 0014 "Dev runtime topology".
    engine_loopback: bool,
    /// Whether the spawned engine serves TLS on its port. A dev engine is the
    /// direct front and keeps its cert, so the gateway must proxy and
    /// health-probe it over https. A packaged engine serves plain HTTP on
    /// loopback and the gateway terminates TLS.
    engine_tls: bool,
    pg_backend: PgBackend,
    /// True under the packaged desktop runtime. Reported in
    /// `GET /~/api/v1/control/gateway/status` so the picker hides the dev-only
    /// self-reload control: a packaged binary swap goes through the app updater
    /// and a full service restart.
    packaged: bool,
    /// Shared-cluster provisioning is serialized across workspaces. Docker
    /// container creation and embedded `pg_ctl` startup are cluster-level, so
    /// concurrent per-workspace starts queue here, then create their own
    /// databases.
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
    /// Hot-path id to engine-port map for the proxy, so proxying never contends
    /// with a stack mutex held during a multi-second respawn.
    routes: RwLock<HashMap<String, u16>>,
    /// id to current cold-boot phase, rendered as the boot-splash label
    /// ([`crate::boot_phase`]). Written by the gateway as it provisions and
    /// spawns, and by the `boot-phase` control endpoint when the engine reports.
    /// Entries are removed once the workspace is healthy or stopped, so a later
    /// cold open never shows a stale phase.
    boot_phases: RwLock<HashMap<String, BootProgress>>,
    /// id to why this workspace's boot attempt failed ([`crate::boot_failure`]).
    /// A TERMINAL entry means this boot cannot succeed: the splash renders the
    /// message with no refresh and the supervisor stops respawning. A RETRYING
    /// one is the splash label while the gateway works through its budget.
    ///
    /// Separate from `boot_phases` because `MarkUnhealthy` clears the phase on
    /// purpose, and that is exactly the moment the failure must survive. Cleared
    /// only when the workspace boots healthy, is stopped, or a fresh attempt
    /// begins.
    boot_failures: RwLock<HashMap<String, BootFailure>>,
    /// Single-slot state of the picker's restore-from-backup flow (see
    /// [`RestoreStatus`]). Polled via the control API; never persisted.
    restore: RwLock<RestoreStatus>,
    /// Configured bind addresses this process wants but does not hold yet (see
    /// [`net_config::bind_plan`]). Normally empty. Non-empty means the gateway
    /// is serving loopback while it waits for an interface to appear, a
    /// reachability degradation reported in `/~/api/v1/health`.
    pending_binds: RwLock<BTreeSet<SocketAddr>>,
    /// Path of the binary this process was launched from, used by the reload
    /// control to re-exec onto the rebuilt binary and to stat for the
    /// "new gateway available" check.
    exe_path: Option<PathBuf>,
    /// Cached update check, re-run only when the binary's mtime moves, so the
    /// picker's 2s poll does not fork `current_exe --build-id` every tick.
    update_check: Mutex<UpdateCheck>,
}

/// Memoized "is a newer gateway binary on disk?" verdict, keyed by the binary's
/// last-seen mtime. A `None` mtime means not yet checked.
#[derive(Default)]
struct UpdateCheck {
    last_mtime: Option<std::time::SystemTime>,
    update_available: bool,
}

impl GatewayState {
    fn app_data(&self) -> &PathBuf {
        &self.inner.app_data
    }

    /// Loopback port for `id`, if registered. Hot path, so no async locks.
    fn route(&self, id: &str) -> Option<u16> {
        self.inner.routes.read().ok()?.get(id).copied()
    }

    /// The scheme the spawned engine serves on its port. The literals come from
    /// `net_config` rather than being spelled here, so this side of the hop
    /// cannot drift from the rule that decided `engine_tls`.
    fn engine_scheme(&self) -> &'static str {
        if self.inner.engine_tls {
            net_config::SCHEME_HTTPS
        } else {
            net_config::SCHEME_HTTP
        }
    }

    /// Tell ONE stack's engine that a person asked for the teardown it is about
    /// to be signalled for, over that stack's own port.
    ///
    /// Called from the two control-plane entry points, never from
    /// [`Self::respawn_stack`], which is also the supervisor's health-respawn
    /// path. An engine the supervisor culls was not stopped by anyone.
    /// Attributing it to a device would auto-resume work after a crash, which
    /// cause-gated resume exists to prevent.
    async fn notify_restart_intent(
        &self,
        s: &StackRuntime,
        requested_by: Option<&str>,
    ) -> stack::RestartIntentNotify {
        stack::notify_restart_intent(
            &self.inner.health_client,
            self.engine_scheme(),
            s.ws.port,
            requested_by,
        )
        .await
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

    /// Record the current cold-boot phase for `id`. Hot path, so no async locks.
    pub fn set_boot_phase(&self, id: &str, phase: BootPhase) {
        if let Ok(mut p) = self.inner.boot_phases.write() {
            // Preserve `since` across phase updates: it marks when THIS boot
            // window opened, so the escape budget accumulates over the whole
            // episode rather than resetting on every phase advance.
            p.entry(id.to_string())
                .and_modify(|bp| bp.phase = phase)
                .or_insert(BootProgress {
                    phase,
                    since: Instant::now(),
                });
        }
    }

    /// Drop any boot phase for `id`, so a later cold open starts from the
    /// default label rather than a stale phase from the previous boot.
    pub fn clear_boot_phase(&self, id: &str) {
        if let Ok(mut p) = self.inner.boot_phases.write() {
            p.remove(id);
        }
    }

    /// Record a boot failure for `id` (see [`GatewayInner::boot_failures`]). Two
    /// producers: the `boot-failure` control endpoint, always terminal, and the
    /// provisioning paths here, terminal or retrying.
    pub fn set_boot_failure(&self, id: &str, failure: BootFailure) {
        crate::log!(
            "[Gateway] '{}' boot failure ({}): {}",
            id,
            if failure.is_terminal() {
                "terminal"
            } else {
                "retrying"
            },
            failure.message()
        );
        if let Ok(mut f) = self.inner.boot_failures.write() {
            f.insert(id.to_string(), failure);
        }
    }

    /// Drop any boot failure for `id`. Never call this from `MarkUnhealthy`,
    /// which is the state the message exists to explain.
    fn clear_boot_failure(&self, id: &str) {
        if let Ok(mut f) = self.inner.boot_failures.write() {
            f.remove(id);
        }
    }

    /// The boot failure recorded for `id` this boot episode, if any.
    fn boot_failure(&self, id: &str) -> Option<BootFailure> {
        self.inner.boot_failures.read().ok()?.get(id).cloned()
    }

    /// The boot-splash label for `id`. A TERMINAL failure never reaches here:
    /// the caller routes it to [`proxy::failed_page`], which drops the
    /// auto-refresh. See [`splash_label`] for the ranking.
    fn boot_splash_label(&self, id: &str) -> String {
        let phase = self
            .inner
            .boot_phases
            .read()
            .ok()
            .and_then(|p| p.get(id).map(|bp| bp.phase));
        splash_label(self.boot_failure(id).as_ref(), phase)
    }

    /// How long `id`'s current boot window has been open. Cheap `RwLock` read
    /// off the stack-mutex path, so it is safe on the proxy hot path.
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
                    // No engine to poll, so no badge.
                    unread_count: None,
                });
            }
        }
        out
    }

    /// Fresh aggregate unread total, fanned out on demand rather than read from
    /// the `last_unread` the supervise loop caches. This drives the dock badge's
    /// instant update when a notification is read: at nudge time the supervise
    /// loop has not yet re-probed, so its cache still shows the pre-read count.
    /// The gateway holds no DB handle (ADR 0014 §1), so polling running engines
    /// is the only count path.
    pub async fn fresh_unread_total(&self) -> u64 {
        // Snapshot the stack handles, then read each briefly for (health, port).
        // Mirrors `list_status`'s lock discipline so a supervisor probe holding a
        // stack mutex never stalls this read across the whole map.
        let stacks: HashMap<String, Arc<AsyncMutex<StackRuntime>>> =
            self.inner.stacks.lock().await.clone();
        let mut ports = Vec::with_capacity(stacks.len());
        for stack in stacks.values() {
            let s = stack.lock().await;
            if s.health == Health::Healthy {
                ports.push(s.ws.port);
            }
        }
        let scheme = self.engine_scheme();
        let client = &self.inner.health_client;
        let counts = futures::future::join_all(
            ports
                .iter()
                .map(|&port| async move { stack::fetch_unread_count(client, scheme, port).await }),
        )
        .await;
        sum_unread(counts)
    }

    // ── Self-update (reload onto a rebuilt binary) ─────────────────────────────

    /// This process's baked build id.
    pub fn build_id(&self) -> &'static str {
        GATEWAY_BUILD_ID
    }

    /// True under the packaged desktop runtime. The picker hides the dev-only
    /// self-reload control when this is set.
    pub fn packaged(&self) -> bool {
        self.inner.packaged
    }

    /// Whether the on-disk gateway binary is NEWER than this running process,
    /// so a rebuild is waiting to be adopted via [`Self::reload_gateway`]. Cheap
    /// on the steady path: it forks `current_exe --build-id` only when the
    /// binary's mtime has moved, and the direction probe rides the same cache.
    ///
    /// **Newer, not merely different** ([`crate::build_id::disk_id_is_upgrade`]).
    /// `reload_gateway` re-execs onto whatever is on disk, so a bare
    /// `disk != running` would walk the machine's only gateway BACKWARDS onto an
    /// older binary another build left in `target/`. Anything indeterminate
    /// keeps the difference test, so this removes a false positive without
    /// adding a way to miss a real update.
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
            // Cannot read the on-disk id (binary mid-rewrite, spawn failure), so
            // report no update and let the next poll retry once mtime settles.
            _ => return false,
        };
        let update_available =
            crate::build_id::disk_id_is_upgrade(&exe, &disk_id, GATEWAY_BUILD_ID).await;
        let mut cache = self.inner.update_check.lock().unwrap();
        cache.last_mtime = mtime;
        cache.update_available = update_available;
        update_available
    }

    /// Re-exec this process onto the on-disk binary, keeping the PID so the
    /// supervisor keeps `wait`ing and the pidfile stays valid. The fresh
    /// `main()` re-adopts the running engines (see [`Self::boot_all`]). This is
    /// the ONLY in-place gateway restart: SIGUSR1 is its permanent stop, not a
    /// restart (see `scripts/lib/gateway_supervisor.sh`).
    ///
    /// Returns after scheduling the exec so the HTTP caller still gets its
    /// response. `execv` only returns on failure, and then we keep the current
    /// image.
    pub fn reload_gateway(&self) -> Result<(), BoxError> {
        let exe = self
            .inner
            .exe_path
            .clone()
            .ok_or("current_exe unavailable — cannot reload")?;
        // Refuse to adopt a binary living in a coding-agent worktree (ADR 0021).
        // Unlike the shell entry points there is no operator here to read an
        // error, so keep the CURRENT image and log loudly.
        if crate::stack::path_is_in_cc_worktree(&exe) {
            crate::log!(
                "[Gateway] refusing to reload onto a coding-agent worktree binary: {} \
                 — staying on the current image. Relaunch the stack from the real \
                 checkout (./scripts/web-dev.sh -w <workspace> -b).",
                exe.display()
            );
            return Err("gateway binary lives in a coding-agent worktree — \
                        relaunch from the real checkout"
                .into());
        }
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
    /// failure-isolated. [`should_bring_up`] decides which. One left stopped is
    /// still listed in the picker and starts on an explicit open
    /// ([`Self::lazy_start`]).
    async fn boot_all(&self) {
        // Consumed exactly once, here, before anything is brought up. Empty in
        // dev, where nothing writes the record.
        let restore: Arc<HashSet<String>> =
            Arc::new(next_boot::take(self.app_data()).into_iter().collect());
        if !restore.is_empty() {
            let mut ids: Vec<&str> = restore.iter().map(String::as_str).collect();
            ids.sort_unstable();
            crate::log!(
                "[Gateway] restoring {} workspace(s) the last shutdown stopped: {}",
                ids.len(),
                ids.join(", ")
            );
        }
        let workspaces: Vec<Workspace> = {
            let reg = self.inner.registry.lock().unwrap();
            reg.workspaces.clone()
        };
        let futures = workspaces.into_iter().map(|ws| {
            let me = self.clone();
            let restore = Arc::clone(&restore);
            async move {
                let running =
                    stack::probe_health(&me.inner.health_client, me.engine_scheme(), ws.port).await
                        == ProbeOutcome::Healthy;
                if should_bring_up(&ws, running, &restore) {
                    // bring_up itself re-adopts a healthy engine and only spawns
                    // when none is running, so this is correct for both cases.
                    me.bring_up(ws).await;
                }
            }
        });
        futures::future::join_all(futures).await;
    }

    /// Re-read the on-disk registry into memory. The dev launcher writes the
    /// shared registry file directly, so a running gateway's copy can lag a
    /// freshly-launched workspace. A bad read is logged, not propagated: a
    /// transient parse error must not break a restart of a known workspace.
    fn sync_registry_from_disk(&self) {
        match Registry::load(&self.inner.registry_path) {
            Ok(reg) => *self.inner.registry.lock().unwrap() = reg,
            Err(e) => crate::log!("[Gateway] registry reload failed: {}", e),
        }
    }

    /// Lazily start a registered-but-stopped workspace for a document
    /// navigation. Guarded by [`GatewayInner::starting`] so a burst of
    /// concurrent requests cannot each spawn a duplicate engine before the stack
    /// lands in `stacks`.
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
    /// register its runtime stack + route. Never panics.
    ///
    /// A failure yields a stack whose shape depends on whether retrying it is
    /// worth anything ([`provision_failure_action`]). A RETRYABLE one stays
    /// `Booting` and supervised, so the health monitor works through the restart
    /// budget. A TERMINAL one latches `Unhealthy` at once, and the picker's
    /// Retry or delete is the escape. Either way the reason is recorded as a
    /// boot failure so the splash can explain itself. See ADR 0014.
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
                // This was attempt 1 of the budget.
                let attempts = 1;
                let latch = self.record_provision_failure(&ws.id, &e, attempts)
                    == ProvisionFailureAction::Latch;
                StackRuntime {
                    ws: ws.clone(),
                    resolved_dir,
                    pg: PgHandle::External,
                    engine: None,
                    // A retryable failure must stay OUT of `Unhealthy`: the
                    // supervisor skips an unhealthy stack entirely, so that state
                    // is the latch, not merely a label.
                    health: if latch {
                        Health::Unhealthy
                    } else {
                        Health::Booting
                    },
                    restart_attempts: if latch { RESTART_CAP } else { attempts },
                    health_misses: 0,
                    // Dates the attempt either way, so the first retry waits out
                    // a full backoff instead of firing on the next 2s tick.
                    last_spawn: Some(Instant::now()),
                    last_error: Some(e.message),
                    last_unread: None,
                }
            }
        };

        self.set_route(&ws.id, ws.port);
        self.install_stack(&ws.id, stack).await;
        crate::log!("[Gateway] workspace '{}' engine on :{}", ws.id, ws.port);
    }

    /// Put a stack in the map under `id`, tearing down whatever engine that slot
    /// already held.
    ///
    /// Dropping a `StackRuntime` neither stops nor reaps its engine, because
    /// `std::process::Child` does neither on drop. A displaced engine would keep
    /// running on the port of the one just spawned AND become an unreapable
    /// zombie when it exits. No caller reaches this with an occupied slot today,
    /// which is why the guarantee belongs here rather than in each caller.
    ///
    /// The map lock is released before the stack lock, the ordering every other
    /// map-then-stack site uses (the supervisor holds stack then map).
    async fn install_stack(&self, id: &str, stack: StackRuntime) {
        let displaced = self
            .inner
            .stacks
            .lock()
            .await
            .insert(id.to_string(), Arc::new(AsyncMutex::new(stack)));
        if let Some(old) = displaced {
            crate::log!(
                "[Gateway] '{}' already had a running stack, stopping the displaced engine",
                id
            );
            stop_engine_process(&mut *old.lock().await);
        }
    }

    /// Record a failed provisioning attempt for `id` and report whether the
    /// workspace must now latch. Shared by the FIRST attempt ([`Self::bring_up`])
    /// and every retry ([`Self::respawn_stack`]), so the two cannot drift into
    /// classifying or narrating the same failure differently.
    ///
    /// `attempts` is how many bring-up attempts have been made since the stack
    /// was last healthy, this one included.
    fn record_provision_failure(
        &self,
        id: &str,
        e: &ProvisionError,
        attempts: u32,
    ) -> ProvisionFailureAction {
        let action = provision_failure_action(e.kind, attempts);
        self.set_boot_failure(
            id,
            match action {
                ProvisionFailureAction::Latch => BootFailure::terminal(&e.message),
                ProvisionFailureAction::Retry => {
                    BootFailure::retrying(&e.message, attempts, RESTART_CAP)
                }
            },
        );
        if action == ProvisionFailureAction::Latch {
            // Nothing more will be attempted, so a phase label would only
            // describe a step that is not running. The failure message is what
            // the splash renders now.
            self.clear_boot_phase(id);
        }
        action
    }

    /// Ensure Postgres, then adopt a healthy already-running engine or spawn a
    /// fresh one.
    ///
    /// The error carries a [`ProvisionErrorKind`] so [`Self::bring_up`] can tell
    /// a condition that will clear from one that never will.
    async fn provision_and_start(
        &self,
        ws: &Workspace,
        resolved_dir: &Path,
    ) -> Result<(PgHandle, Option<std::process::Child>, Health), ProvisionError> {
        // First splash phase: provisioning Postgres can pull or start a
        // container, or run `initdb` on a first-ever open.
        self.set_boot_phase(&ws.id, BootPhase::ProvisioningDatabase);
        // A new boot episode starts with a clean slate, so a stale message
        // cannot outlive the condition it described.
        self.clear_boot_failure(&ws.id);
        let prov = self.ensure_postgres(ws).await?;

        if stack::probe_health(&self.inner.health_client, self.engine_scheme(), ws.port).await
            == ProbeOutcome::Healthy
        {
            crate::log!("[Gateway] re-adopting healthy engine for '{}'", ws.id);
            // Already up — no boot window to narrate, and nothing failed.
            self.clear_boot_phase(&ws.id);
            self.clear_boot_failure(&ws.id);
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
        )
        // Retryable: the common causes (a binary mid-rebuild, a transient
        // resource limit) clear on their own, and the budget bounds the rest.
        .map_err(|e| ProvisionError::transient(format!("could not spawn the engine: {e}")))?;
        // The engine now reports finer phases through the boot-phase control
        // endpoint until its first healthy probe clears the phase.
        self.set_boot_phase(&ws.id, BootPhase::StartingEngine);
        Ok((prov.handle, Some(child), Health::Booting))
    }

    /// Create a new workspace: reserve a registry entry (id + free port), persist
    /// it, then provision + start the stack. The entry is committed before
    /// provisioning so a provisioning failure leaves a recoverable `Unhealthy`
    /// workspace (retry / delete from the picker) rather than vanishing.
    ///
    /// New workspaces get `autostart = true`: the always-on service only keeps
    /// its promise (triggers, scheduled tasks, push) for a workspace whose
    /// engine is up. The picker toggle turns it off per workspace. This is the
    /// sole *create* path, and [`Self::restore_workspace`] is the other way an
    /// entry is born.
    ///
    /// Refuses a display name another workspace already carries, because two
    /// rows the user cannot tell apart is not a state worth creating. The
    /// ADDRESS may still be taken while the name is free, and that one is
    /// suffixed rather than refused.
    pub async fn create_workspace(&self, name: &str) -> Result<WorkspaceStatus, ApiError> {
        let ws = {
            let mut reg = self.inner.registry.lock().unwrap();
            if let Some(existing) = reg.find_by_display_name(name, None) {
                return Err(ApiError::conflict(name_taken_message(&existing.name)));
            }
            // A restore in flight has reserved its name for minutes without a
            // registry entry to show for it: see `restore_reserved_name`.
            if names_match(&self.restore_reserved_name(), name) {
                return Err(ApiError::conflict(name_being_restored_message(name)));
            }
            let base = registry::slugify(name);
            let id = registry::unique_slug(&base, &|s| reg.contains(s));
            let port = reg
                .allocate_port()
                .map_err(|e| ApiError::internal(e.to_string()))?;
            let ws = Workspace::gateway_provisioned(id, name.to_string(), port);
            reg.add(ws.clone())
                .map_err(|e| ApiError::internal(e.to_string()))?;
            reg.save(&self.inner.registry_path)
                .map_err(|e| ApiError::internal(e.to_string()))?;
            ws
        };

        self.bring_up(ws.clone()).await;
        // Clone the Arc out and drop the map lock before locking the stack. The
        // supervisor holds stack then map, so the reverse order here would
        // deadlock (same ordering as restart, stop and delete).
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

    /// The display name an in-flight restore has reserved, if any.
    ///
    /// A restore holds its name from acceptance until its registry entry is
    /// committed, which is minutes later. For that whole window the name is
    /// nowhere in the registry. Create and rename must consult this too, or
    /// they hand out a name the restore is about to commit. `Registry::add`
    /// does not catch that, because it checks ids only.
    ///
    /// Callers hold the registry lock across this read, which is what makes the
    /// pair atomic. Lock order is always registry then restore.
    fn restore_reserved_name(&self) -> Option<String> {
        match self.inner.restore.read().ok().as_deref() {
            Some(RestoreStatus::Running { name, .. }) => Some(name.clone()),
            _ => None,
        }
    }

    /// Update the `phase` of an in-flight restore (no-op if not Running).
    fn set_restore_phase(&self, phase: &str) {
        if let Ok(mut st) = self.inner.restore.write() {
            if let RestoreStatus::Running { phase: p, .. } = &mut *st {
                *p = phase.to_string();
            }
        }
    }

    /// Begin restoring a local backup archive into a NEW workspace. Validates
    /// synchronously, then spawns the heavy restore in the background and
    /// returns `{id, name}` with a 202. The picker polls
    /// [`Self::restore_status`].
    ///
    /// The registry entry is committed only AFTER the archive is restored into
    /// the dir and database. So a failure before then leaves nothing registered,
    /// and the cleanup removes only what this attempt created, never a
    /// pre-existing workspace.
    ///
    /// The entry it commits auto-starts, like [`Self::create_workspace`]'s, so a
    /// restored workspace resumes its triggers and push at the next login.
    pub async fn restore_workspace(
        &self,
        archive_tmp: PathBuf,
        archive_filename: String,
        key_b64: String,
        requested_name: Option<String>,
    ) -> Result<(String, String), ApiError> {
        // An explicit name wins, otherwise parse it from the archive filename.
        // A non-archive filename with no explicit name is a 400 telling the
        // picker to ask for one.
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

        // Validate the name, reserve the id and port, and CLAIM THE RESTORE
        // SLOT, all while holding the registry lock.
        //
        // The claim belongs inside this critical section, not after it. The
        // registry entry is committed only minutes later, and until then nothing
        // but the claim holds the name, so a rename could take it meanwhile.
        // The claim IS the reservation (see `restore_reserved_name`).
        //
        // Lock order is registry then restore, everywhere, so the two can never
        // deadlock against each other.
        let (id, port) = {
            let reg = self.inner.registry.lock().unwrap();
            let mut st = self.inner.restore.write().map_err(|_| {
                let _ = std::fs::remove_file(&archive_tmp);
                ApiError::internal("restore state poisoned")
            })?;
            // Check-and-set the single slot, so two near-simultaneous restores
            // cannot both pass. The control handler's pre-check is a
            // best-effort fast-fail; this is the authoritative gate.
            if matches!(*st, RestoreStatus::Running { .. }) {
                let _ = std::fs::remove_file(&archive_tmp);
                return Err(ApiError::conflict("A restore is already in progress"));
            }
            // The name first, then the address it derives. A duplicate NAME is
            // the one the user can see, so say that when both are taken.
            if let Some(existing) = reg.find_by_display_name(&name, None) {
                let _ = std::fs::remove_file(&archive_tmp);
                return Err(ApiError::conflict(name_taken_message(&existing.name)));
            }
            let slug = registry::slugify(&name);
            if let Some(existing) = reg.get(&slug) {
                let _ = std::fs::remove_file(&archive_tmp);
                return Err(ApiError::conflict(address_taken_message(
                    &slug,
                    &existing.name,
                )));
            }
            let port = reg.allocate_port().map_err(|e| {
                let _ = std::fs::remove_file(&archive_tmp);
                ApiError::internal(e.to_string())
            })?;
            *st = RestoreStatus::Running {
                id: slug.clone(),
                name: name.clone(),
                phase: "starting".to_string(),
            };
            (slug, port)
        };

        let ws = Workspace::gateway_provisioned(id.clone(), name.clone(), port);
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

    /// The heavy restore: provision Postgres ONCE, run the engine
    /// `restore-archive` CLI into that dir and database, then commit the
    /// registry entry and spawn the engine. Cleanup on a pre-commit failure
    /// removes only what this attempt created.
    async fn run_restore(
        &self,
        ws: Workspace,
        archive_tmp: &Path,
        key_b64: &str,
    ) -> Result<(), String> {
        let resolved_dir = ws.resolve_dir(self.app_data());
        std::fs::create_dir_all(resolved_dir.join("data"))
            .map_err(|e| format!("create workspace dir: {e}"))?;

        // Provision Postgres ONCE and reuse its URL for BOTH the CLI restore
        // and the engine server. Re-provisioning would, on the embedded
        // backend, restart the cluster on a new port between the two steps.
        // So spawn the engine directly here rather than calling `bring_up`.
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

        // Spawn the engine on the already-restored database. Its construction
        // runs migrations, upgrading an older-schema restore. A spawn failure
        // here is not fatal: the workspace is registered and restored, so the
        // picker lists it and an Open lazy-starts it. Never tear down the
        // user's just-restored data.
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
                self.install_stack(&ws.id, stack).await;
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

    /// Shell out to `lucidos-engine restore-archive` to decrypt, unpack and
    /// pg_restore the archive. The key and database URL go via env, out of
    /// argv. Stdout `LUCIDOS_RESTORE_PHASE=<phase>:<pct>` lines drive the
    /// picker's phase, and the tail of stderr is the failure message.
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
    /// provisioned and remove the freshly-created workspace dir. Only ever
    /// touches state THIS attempt created, because the id is freshly allocated
    /// and the registry entry is committed only after a successful restore.
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

    async fn ensure_postgres(
        &self,
        ws: &Workspace,
    ) -> Result<postgres::Provisioned, ProvisionError> {
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

    /// Rename edits the display name only, in the registry and the runtime. No
    /// dir move, database reconnect, or port change.
    ///
    /// Refuses a name another workspace already carries, so a rename cannot
    /// produce the pair of identical-looking rows that create and restore
    /// refuse. Renaming a workspace to what it is already called, or a case
    /// edit of that, is not a collision with itself.
    pub async fn rename_workspace(&self, id: &str, name: &str) -> Result<(), ApiError> {
        {
            let mut reg = self.inner.registry.lock().unwrap();
            if let Some(existing) = reg.find_by_display_name(name, Some(id)) {
                return Err(ApiError::conflict(name_taken_message(&existing.name)));
            }
            if names_match(&self.restore_reserved_name(), name) {
                return Err(ApiError::conflict(name_being_restored_message(name)));
            }
            let ws = reg
                .get_mut(id)
                .ok_or_else(|| ApiError::bad_request(format!("workspace '{id}' not found")))?;
            ws.name = name.to_string();
            reg.save(&self.inner.registry_path)
                .map_err(|e| ApiError::internal(e.to_string()))?;
        }
        // Map lock released before the stack lock, as everywhere else here.
        let stack = self.inner.stacks.lock().await.get(id).cloned();
        if let Some(stack) = stack {
            stack.lock().await.ws.name = name.to_string();
        }
        crate::log!("[Gateway] renamed '{}' -> '{}'", id, name);
        Ok(())
    }

    /// Start or restart one workspace. Resyncs the registry from disk first so
    /// a freshly-launched workspace is known. An existing stack is respawned
    /// onto the rebuilt binary, which is the Apply case; otherwise the registry
    /// entry is brought up.
    ///
    /// `requested_by` is the device id the picker sent, and `None` for every
    /// caller that is not a person clicking. Present, it is handed to the engine
    /// just before the signal so its in-flight threads settle as a user restart
    /// rather than a crash. See [`crate::stack::notify_restart_intent`].
    pub async fn restart_workspace(
        &self,
        id: &str,
        requested_by: Option<&str>,
    ) -> Result<(), BoxError> {
        // The dev launcher writes the shared registry file directly before
        // POSTing here, so pick up any newly-seeded entry or flag change.
        self.sync_registry_from_disk();
        let existing = self.inner.stacks.lock().await.get(id).cloned();
        match existing {
            Some(stack) => {
                let mut s = stack.lock().await;
                s.restart_attempts = 0;
                // BEFORE the respawn, whose first act is to signal the engine.
                // Awaited rather than spawned: the actor must be stashed by the
                // time the engine reaches its shutdown handler, and losing that
                // race gives the teardown a crash-shaped attribution.
                self.notify_restart_intent(&s, requested_by).await;
                self.respawn_stack(&mut s).await;
                Ok(())
            }
            None => {
                // Route through the guarded `lazy_start` so a concurrent
                // proxy-hit lazy-start cannot double-spawn the engine.
                if !self.inner.registry.lock().unwrap().contains(id) {
                    return Err(format!("workspace '{id}' not found").into());
                }
                self.lazy_start(id).await;
                Ok(())
            }
        }
    }

    /// Stop one workspace's engine and drop its runtime stack, but KEEP its
    /// registry entry: membership is "all ever launched", so it stays listed in
    /// the picker as stopped. Dropping the stack is also what makes the
    /// supervisor stop respawning the engine. Postgres is left untouched.
    ///
    /// `requested_by` carries the picker's device id when a person clicked
    /// Stop, and is `None` for every other caller. Same contract as
    /// [`Self::restart_workspace`]: a named device makes the engine's in-flight
    /// threads settle as a deliberate pause rather than a crash.
    pub async fn stop_workspace(
        &self,
        id: &str,
        requested_by: Option<&str>,
    ) -> Result<(), BoxError> {
        let removed = self.inner.stacks.lock().await.remove(id);
        if let Some(stack) = removed {
            let mut s = stack.lock().await;
            self.notify_restart_intent(&s, requested_by).await;
            stop_engine_process(&mut s);
        }
        self.clear_route(id);
        // No live route means the next open lazy-starts fresh. Drop any stale
        // boot phase so that open begins from the default splash label, and any
        // terminal failure so the retry is judged on its own merits.
        self.clear_boot_phase(id);
        self.clear_boot_failure(id);
        crate::log!("[Gateway] stopped '{}' (kept in registry)", id);
        Ok(())
    }

    /// Flip a workspace's `autostart` flag. Registry only: this does NOT start
    /// or stop the engine. Persisted so it survives a gateway restart.
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
        // DROP the map lock before locking the stack: the supervisor takes
        // stack then map, so the reverse order here is an AB-BA deadlock.
        let stack = self.inner.stacks.lock().await.get(id).cloned();
        if let Some(stack) = stack {
            stack.lock().await.ws.autostart = enabled;
        }
        crate::log!("[Gateway] '{}' autostart = {}", id, enabled);
        Ok(())
    }

    /// Delete-to-trash: optionally confirm the typed name, stop the stack,
    /// unregister, then move the dir to `<app-data>/deleted/<id>-<ts>/`, where
    /// it is recoverable until purged. Never an immediate `rm`.
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

        // The database is dropped below by registry id, so a stopped workspace
        // and an unhealthy stack with no `PgHandle` are cleaned up the same
        // way. The shared Postgres cluster stays up for peers.
        //
        // Take the stack out from under the map lock first, so the blocking
        // process signal does not pin the whole map.
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
    /// shared-cluster database, spawn fresh. Records the error on failure
    /// rather than panicking.
    async fn respawn_stack(&self, s: &mut StackRuntime) {
        stop_engine_process(s);
        s.last_spawn = Some(Instant::now());
        s.restart_attempts += 1;
        // The respawn acts on the accumulated misses, so clear them and let the
        // fresh engine start its boot window with a clean count.
        s.health_misses = 0;

        self.set_boot_phase(&s.ws.id, BootPhase::ProvisioningDatabase);
        // Fresh attempt, so the previous attempt's reason no longer describes
        // anything. See the matching clear in `provision_and_start`.
        self.clear_boot_failure(&s.ws.id);
        let prov = match self.ensure_postgres(&s.ws).await {
            Ok(p) => p,
            Err(e) => {
                // `restart_attempts` was incremented above, so it is this
                // attempt's number.
                s.health = match self.record_provision_failure(&s.ws.id, &e, s.restart_attempts) {
                    ProvisionFailureAction::Latch => Health::Unhealthy,
                    // Put the stack back under supervision. That matters when
                    // this respawn came from the picker's Retry on an already
                    // LATCHED workspace: leaving it `Unhealthy` makes the
                    // supervisor skip it again, so the retrying message just
                    // recorded would promise attempts that never come.
                    ProvisionFailureAction::Retry => Health::Booting,
                };
                s.last_error = Some(format!("postgres: {}", e.message));
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

    /// One supervision pass over every stack. Liveness is not health: an engine
    /// whose PROCESS is alive but not yet answering `/api/v1/health` is still
    /// BOOTING and is left alone within [`BOOT_GRACE`]. See
    /// [`respawn_decision`] for what happens after that.
    async fn supervise_once(&self) {
        let stacks: Vec<Arc<AsyncMutex<StackRuntime>>> =
            { self.inner.stacks.lock().await.values().cloned().collect() };

        // Snapshot phase: capture each candidate stack's probe inputs under a
        // BRIEF lock, then RELEASE it. The probe must not run with the stack
        // lock held. The picker's poll takes that same per-stack lock, so
        // holding it across the up-to-5s probe stalls the picker whenever an
        // engine is slow to answer. The `StackRuntime` mutex protects in-memory
        // state, not the network round-trip.
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
            // A concurrent delete may have removed this stack from the map, and
            // trashed its dir, while we held only the snapshot Arc. Never
            // resurrect a deleted workspace's engine and Postgres.
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
        // The unread count rides the same pass, and only for an engine that
        // answers healthy. This is the sole unread-count path, because the
        // gateway holds no DB handle (ADR 0014 §1). A stopped workspace
        // therefore yields `None` and shows no badge.
        let outcomes: Vec<(ProbeOutcome, Option<u64>)> =
            futures::future::join_all(candidates.iter().map(|t| async move {
                let outcome = stack::probe_health(client, scheme, t.port).await;
                let unread = if outcome == ProbeOutcome::Healthy {
                    stack::fetch_unread_count(client, scheme, t.port).await
                } else {
                    None
                };
                (outcome, unread)
            }))
            .await;

        // Apply phase: re-acquire each stack briefly to write the result back.
        for (t, (outcome, unread)) in candidates.into_iter().zip(outcomes) {
            let mut s = t.stack.lock().await;
            // The lock was dropped across the probe, so the stack may have
            // changed under us (see `probe_result_is_stale`). `contains_key` is
            // checked WHILE holding the stack lock, matching the delete path's
            // ordering so a concurrent delete is observed as absent.
            let present = self.inner.stacks.lock().await.contains_key(&t.id);
            if probe_result_is_stale(present, t.last_spawn, s.last_spawn, s.health) {
                continue;
            }
            if outcome == ProbeOutcome::Healthy {
                s.health = Health::Healthy;
                s.restart_attempts = 0;
                s.health_misses = 0;
                s.last_error = None;
                // `None` when the fetch failed even though health passed: show
                // no badge rather than a stale one.
                s.last_unread = unread;
                // Drop the boot phase so a later cold open starts clean, and
                // the failure message with it: this boot demonstrably worked.
                self.clear_boot_phase(&t.id);
                self.clear_boot_failure(&t.id);
                continue;
            }

            // Not healthy, so no trustworthy count: clear the badge this tick.
            s.last_unread = None;

            let since_spawn = s.last_spawn.map(|t| t.elapsed()).unwrap_or(Duration::MAX);
            let alive = engine_process_alive(&mut s);
            // Count this miss; a healthy probe resets it. Only a DEAD process
            // is ever culled, so a load spike cannot cull a working engine.
            s.health_misses = s.health_misses.saturating_add(1);

            let boot_failure = self.boot_failure(&t.id);

            match respawn_decision(
                outcome,
                alive,
                since_spawn,
                s.health_misses,
                s.restart_attempts,
                // Only a TERMINAL failure short-circuits. A retrying one is the
                // gateway narrating its own backoff; treating it as terminal
                // would latch exactly the workspace it exists to keep alive.
                boot_failure.as_ref().is_some_and(BootFailure::is_terminal),
            ) {
                // Healthy is handled above; treat defensively as a no-op.
                SuperviseAction::Healthy => {}
                SuperviseAction::Booting => s.health = Health::Booting,
                SuperviseAction::Wait => {}
                SuperviseAction::MarkUnhealthy => {
                    s.health = Health::Unhealthy;
                    // Gave up auto-respawning, so drop the phase: the last
                    // label would otherwise lie about a dead engine.
                    // `boot_failures` is NOT cleared here, so a reported cause
                    // outlives the phase and still reaches the splash.
                    self.clear_boot_phase(&t.id);
                    match &boot_failure {
                        // A recorded cause always beats the generic "gave up"
                        // string: it is the specific, actionable text, and it
                        // doubles as the picker's health-dot tooltip. A
                        // retrying failure is promoted first, because the
                        // budget is spent and the splash must stop
                        // auto-refreshing under a promise of another attempt.
                        Some(failure) => {
                            let final_failure = failure.gave_up(s.restart_attempts);
                            s.last_error = Some(final_failure.message());
                            if !failure.is_terminal() {
                                self.set_boot_failure(&t.id, final_failure);
                            }
                        }
                        None if s.last_error.is_none() => {
                            s.last_error = Some(
                                "engine failed to become healthy after repeated restarts"
                                    .to_string(),
                            )
                        }
                        None => {}
                    }
                    crate::log!(
                        "[Gateway] '{}' marked unhealthy after {} attempts{}",
                        t.id,
                        s.restart_attempts,
                        // Which of the two ways to stop this was. Saying "not
                        // retried" of a workspace that merely ran out of
                        // attempts would send the reader hunting for a report
                        // that was never made.
                        match &boot_failure {
                            Some(f) if f.is_terminal() => " (terminal boot failure, not retried)",
                            Some(_) => " (retry budget spent)",
                            None => "",
                        }
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
/// runs WITHOUT the stack lock held. `last_spawn` is the generation marker that
/// discards a result whose engine was respawned during the unlocked window.
struct ProbeTarget {
    stack: Arc<AsyncMutex<StackRuntime>>,
    id: String,
    port: u16,
    last_spawn: Option<Instant>,
}

/// Whether a health-probe result must be DISCARDED rather than applied to its
/// stack. `supervise_once` releases the stack lock across the network probe, so
/// on re-lock the stack may have changed. Discard when:
///   * a concurrent stop or delete removed the stack from the map, so never
///     resurrect its engine;
///   * `last_spawn` moved during the probe, so the result describes the OLD
///     engine and applying it would bounce a just-restarted workspace;
///   * the stack is now capped `Unhealthy`, left for a manual retry or delete.
///
/// `None == None` is a re-adopted engine that has never respawned. It counts as
/// unchanged, so its healthy probes still apply.
fn probe_result_is_stale(
    present: bool,
    last_spawn_before: Option<Instant>,
    last_spawn_now: Option<Instant>,
    health: Health,
) -> bool {
    !present || last_spawn_before != last_spawn_now || health == Health::Unhealthy
}

/// What the supervisor should do with one stack after a single health probe.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SuperviseAction {
    /// Probe succeeded. Handled before [`respawn_decision`] is called, and
    /// returned only for completeness.
    Healthy,
    /// Process alive, still inside its cold-boot window, so leave it alone.
    Booting,
    /// Not enough evidence to cull yet: within backoff, or below the miss
    /// threshold for this outcome.
    Wait,
    /// Cull and respawn the engine.
    Respawn,
    /// Hit the restart cap: mark Unhealthy and stop auto-respawning.
    MarkUnhealthy,
}

/// Pure cull-or-keep decision for one stack. `misses` is the consecutive-miss
/// count INCLUDING the current probe.
///
/// **An ALIVE engine is never culled**, whether booting, busy, `Slow`, or
/// apparently wedged. Respawning a live engine interrupts its in-flight threads
/// and, under sustained contention, feeds a cross-workspace respawn cascade. A
/// timed-out HTTP probe cannot tell "hung forever" from "busy right now". So
/// the supervisor respawns ONLY a process that has actually exited, which still
/// preserves crash recovery and lazy-start. A deadlocked-but-alive engine is
/// the rare accepted cost, and waits for a manual restart. See ADR 0014.
fn respawn_decision(
    outcome: ProbeOutcome,
    alive: bool,
    since_spawn: Duration,
    misses: u32,
    restart_attempts: u32,
    terminal_boot_failure: bool,
) -> SuperviseAction {
    if outcome == ProbeOutcome::Healthy {
        return SuperviseAction::Healthy;
    }
    // The engine reported that this boot cannot succeed, canonically a database
    // migrated by a newer Lucidos. Retrying re-runs the identical failure, so go
    // straight to Unhealthy instead of burning the restart cap first. Ordered
    // ahead of the alive check because a dying engine can still be mid-exit when
    // this probe lands. The never-cull rule protects a busy process, not one
    // that has declared itself dead.
    if terminal_boot_failure {
        return SuperviseAction::MarkUnhealthy;
    }
    // Never cull an alive process. Inside the cold-boot window it is BOOTING;
    // past it, it is busy, slow or wedged, and still left alone.
    if alive {
        return if since_spawn < BOOT_GRACE {
            SuperviseAction::Booting
        } else {
            SuperviseAction::Wait
        };
    }
    // The process has EXITED, so this is crash recovery: backoff, then cap.
    if since_spawn < respawn_backoff(restart_attempts) {
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

/// Which of the two boot-window narrations the splash shows.
///
/// A RETRYING failure wins, because it carries the phase's failure AND how far
/// through the budget we are. A TERMINAL failure is not rendered here at all:
/// the caller sends it to [`proxy::failed_page`]. So a `None` phase alongside
/// one falls through to the neutral default, rather than borrowing terminal
/// text into an auto-refreshing page.
fn splash_label(failure: Option<&BootFailure>, phase: Option<BootPhase>) -> String {
    match failure.filter(|f| !f.is_terminal()) {
        Some(retrying) => retrying.message(),
        None => phase
            .map(BootPhase::label)
            .unwrap_or(boot_phase::DEFAULT_LABEL)
            .to_string(),
    }
}

/// How long to wait before the next attempt at a stack that has already failed
/// `restart_attempts` times since it was last healthy: [`RESPAWN_BACKOFF`]
/// doubling per attempt, capped at [`RESPAWN_BACKOFF_MAX`].
///
/// The first gap stays short, so a one-off engine crash recovers promptly. The
/// growth is for a REPEATING failure: without it, a workspace waiting for
/// Docker Desktop would re-run the whole provisioning sequence every five
/// seconds. Growth also buys the budget a longer wall-clock reach, which has to
/// outlast a cold Docker Desktop start rather than just five ticks.
fn respawn_backoff(restart_attempts: u32) -> Duration {
    // Shift-clamped before the multiply: 2^6 * 5s is already past the cap, so
    // anything beyond that would only risk an overflow for no behavior change.
    let grown = RESPAWN_BACKOFF
        .as_secs()
        .saturating_mul(1u64 << restart_attempts.min(6));
    Duration::from_secs(grown.min(RESPAWN_BACKOFF_MAX.as_secs()))
}

/// What the gateway does with a stack whose provisioning attempt just failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ProvisionFailureAction {
    /// Leave the stack supervised so the health monitor tries again after a
    /// backoff.
    Retry,
    /// Mark it `Unhealthy` and stop: nothing further will be attempted until the
    /// user retries from the picker.
    Latch,
}

/// Whether a failed provisioning attempt is worth another try.
///
/// Two ways to stop. A TERMINAL failure latches on the first attempt, for the
/// same reason a reported terminal boot failure does (ADR 0014): retrying
/// re-runs the identical failure, so burning the budget first only delays the
/// message. Otherwise the budget is the bound. `attempts` counts every bring-up
/// attempt since the stack was last healthy, and at [`RESTART_CAP`] the
/// workspace latches with the last cause.
fn provision_failure_action(kind: ProvisionErrorKind, attempts: u32) -> ProvisionFailureAction {
    if kind == ProvisionErrorKind::Terminal || attempts >= RESTART_CAP {
        ProvisionFailureAction::Latch
    } else {
        ProvisionFailureAction::Retry
    }
}

/// Whether a stack's engine process is currently alive.
///
/// Both arms must stay zombie-aware. `kill(pid, 0)` succeeds for a process that
/// has already exited, and an engine wrongly read as alive is never culled by
/// [`respawn_decision`]. It would sit on the boot splash forever.
fn engine_process_alive(s: &mut StackRuntime) -> bool {
    if let Some(child) = s.engine.as_mut() {
        return matches!(child.try_wait(), Ok(None));
    }
    match stack::read_pidfile(&s.resolved_dir) {
        Some(pid) => stack::pid_is_live(pid),
        None => false,
    }
}

/// Stop a stack's engine process with SIGUSR1, which the engine stops on where
/// it ignores SIGTERM. Reaped off-thread so the supervisor is not blocked by
/// the engine's graceful drain.
///
/// **Both arms reap, and that is the invariant.** The gateway is the parent of
/// every engine it spawns, and a signal is not a wait: an engine torn down
/// without one stays `<defunct>` until the gateway exits. Which arm runs says
/// only whether we still hold the `Child`, never whether the process is ours.
/// It usually still is even in the `None` arm, because `reload_gateway` re-execs
/// in place and keeps the pid. Any new teardown path must go through one of
/// these two.
fn stop_engine_process(s: &mut StackRuntime) {
    match s.engine.take() {
        Some(mut child) => {
            // If the child already exited, do NOT signal its PID — the OS may
            // have recycled it to an unrelated process. `try_wait` has reaped it
            // by returning `Ok(Some(_))`, so dropping the handle leaks nothing.
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            #[cfg(unix)]
            // SAFETY: signalling a still-running child pid; ESRCH if it just died.
            unsafe {
                libc::kill(child.id() as libc::pid_t, libc::SIGUSR1);
            }
            // Reap without blocking the supervisor: a graceful drain can take
            // seconds. A plain thread rather than `spawn_blocking`, matching
            // the handle-less arm, so this sync helper needs no tokio runtime.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        None => stack::reclaim_stale_engine(&s.resolved_dir),
    }
}

// ── Entry point ──────────────────────────────────────────────────────────────

/// Validate `LUCIDOS_ENGINE_BIN` points at a real, executable regular file.
/// Checking only that the var is SET is not enough: a missing, corrupt or
/// quarantined binary then surfaces as a per-workspace spawn error that
/// meta-refreshes the boot splash until the escape budget runs out.
fn validate_engine_bin(path: &Path) -> Result<(), BoxError> {
    // An engine binary inside a coding-agent worktree pins every spawned engine
    // to a throwaway checkout frozen at one commit (ADR 0021). Fail at boot with
    // the corrective command rather than serving stale code forever.
    // Unconditional: `LUCIDOS_ALLOW_WORKTREE_STACK` covers a session-scoped
    // direct engine only, never the machine-global gateway.
    if crate::stack::path_is_in_cc_worktree(path) {
        return Err(format!(
            "LUCIDOS_ENGINE_BIN points into a coding-agent worktree: {} — a worktree is a \
             throwaway checkout pinned to one commit, so the stack would serve a frozen \
             engine and frontend forever. Relaunch from the real checkout \
             (./scripts/web-dev.sh -w <workspace> -b). There is no opt-out here: the \
             gateway is machine-global and outlives the session that launched it.",
            path.display()
        )
        .into());
    }
    let meta = std::fs::metadata(path).map_err(|e| {
        format!(
            "LUCIDOS_ENGINE_BIN does not exist: {} ({e})",
            path.display()
        )
    })?;
    if !meta.is_file() {
        return Err(format!(
            "LUCIDOS_ENGINE_BIN is not a regular file: {}",
            path.display()
        )
        .into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return Err(format!("LUCIDOS_ENGINE_BIN is not executable: {}", path.display()).into());
        }
    }
    Ok(())
}

/// Validate and resolve `LUCIDOS_STATIC_DIR`, the picker frontend. When set,
/// its `index.html` must exist. Otherwise the picker fails while
/// `/~/api/v1/health` still returns 200, so the packaged service supervises a
/// gateway that cannot render its own picker. Unset is fatal under a packaged
/// build, and allowed in dev.
fn resolve_static_dir(dir: Option<PathBuf>, packaged: bool) -> Result<Option<PathBuf>, BoxError> {
    match dir {
        Some(dir) => {
            let index = dir.join("index.html");
            if !index.is_file() {
                // Packaged: a missing index.html is a staging defect, so fail
                // fast. Dev: the gateway boots BEFORE the frontend build, so
                // `dist/index.html` may legitimately not exist yet on a cold
                // start. Warn rather than abort, which would wedge startup.
                if packaged {
                    return Err(format!(
                        "LUCIDOS_STATIC_DIR is set to {} but {} is missing — the picker frontend \
                         is not staged",
                        dir.display(),
                        index.display()
                    )
                    .into());
                }
                crate::log!(
                    "[Gateway] LUCIDOS_STATIC_DIR {} has no index.html yet — picker unavailable \
                     until the frontend build completes",
                    dir.display()
                );
            }
            Ok(Some(dir))
        }
        None if packaged => Err(
            "LUCIDOS_STATIC_DIR must be set in a packaged build (the picker frontend resource)"
                .into(),
        ),
        None => Ok(None),
    }
}

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
    // Resolved before the resource validation, because it drives both the
    // picker's dev-only self-reload control and the static-dir check below.
    let packaged = matches!(
        std::env::var("LUCIDOS_PACKAGED").unwrap_or_default().trim(),
        "1" | "true" | "yes" | "on"
    );

    // Validate the resources the gateway REQUIRES at boot: present and
    // executable, not merely named by a set env var. Fail fast with a
    // path-bearing reason.
    let engine_bin = std::env::var_os("LUCIDOS_ENGINE_BIN")
        .map(PathBuf::from)
        .ok_or("LUCIDOS_ENGINE_BIN must point at the lucidos-engine binary")?;
    validate_engine_bin(&engine_bin)?;
    let static_dir = resolve_static_dir(
        std::env::var_os("LUCIDOS_STATIC_DIR").map(PathBuf::from),
        packaged,
    )?;
    // Engines bind loopback-only by default (packaged security posture); dev sets
    // `LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0` so the engine is reachable directly on
    // its user-facing port too (ADR 0014 "Dev runtime topology").
    let engine_loopback = !matches!(
        std::env::var("LUCIDOS_GATEWAY_ENGINE_LOOPBACK")
            .unwrap_or_default()
            .trim(),
        "0" | "false" | "no" | "off"
    );
    // A dev engine keeps the inherited TLS cert and serves https on its own
    // port (ADR 0014 §4). The gateway must proxy and probe it over https. A
    // packaged engine serves plain http on loopback.
    //
    // Resolved through `net_config::serves_tls`, the same both-present-and-
    // non-empty rule the ENGINE applies. A cert with no key leaves the engine
    // on http. A cert-only test here would put the gateway on https, and the
    // loopback hop would then fail to connect.
    let engine_tls = !engine_loopback
        && net_config::serves_tls(
            std::env::var("LUCIDOS_TLS_CERT").ok().as_deref(),
            std::env::var("LUCIDOS_TLS_KEY").ok().as_deref(),
        );
    // The gateway fronts every workspace API and its own destructive control
    // plane, so `net_config` resolves its bind loopback-first. With nothing set
    // the default is loopback-only, and a malformed value fails safe to
    // loopback, never to all interfaces.
    let network = net_config::read_network_toml();
    let gateway_bind_addr_env = std::env::var("LUCIDOS_GATEWAY_BIND_ADDR").ok();
    let gateway_bind_all_env = std::env::var("LUCIDOS_GATEWAY_BIND_ALL").ok();
    let gateway_bind_choice = net_config::resolve_gateway_bind(
        gateway_bind_addr_env.as_deref(),
        gateway_bind_all_env.as_deref(),
        network.gateway_bind.as_deref(),
    );
    let pg_backend = PgBackend::from_env()?;
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

    let mut registry = Registry::load(&registry_path)?;
    // Packaged only. Dev seeds `autostart: false` deliberately, so migrating
    // there would spawn every workspace the launcher has ever registered. A
    // failed save is logged rather than fatal, and retries next boot.
    if packaged {
        if let Some(changed) = registry.migrate_to_current() {
            crate::log!(
                "[Gateway] registry migrated to v{REGISTRY_VERSION}: {changed} workspace(s) now \
                 start with the background service (change it per workspace in the picker)"
            );
            if let Err(e) = registry.save(&registry_path) {
                crate::log!("[Gateway] could not save the migrated registry: {e}");
            }
        }
    }
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
            boot_failures: RwLock::new(HashMap::new()),
            restore: RwLock::new(RestoreStatus::default()),
            pending_binds: RwLock::new(BTreeSet::new()),
            exe_path: std::env::current_exe().ok(),
            update_check: Mutex::new(UpdateCheck::default()),
        }),
    };
    crate::log!("[Gateway] build id: {}", GATEWAY_BUILD_ID);

    // Re-adopt running engines and spawn the auto-start workspaces (ADR 0014).
    // A first-run empty registry is a no-op: nothing is auto-created, so the
    // smart root serves the picker and the user names their first workspace.
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

/// Build the gateway router and serve it, with TLS when certs are configured.
/// `/~/api/v1/health` and `/~/api/v1/control/*` are the gateway's own, and
/// every other path falls through to [`fallback`].
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
        .with_state(state.clone());
    let router = if permissive_cors_enabled() {
        crate::log!("[Gateway] permissive CORS enabled by LUCIDOS_PERMISSIVE_CORS");
        router.layer(CorsLayer::permissive())
    } else {
        router
    };

    // Every address to listen on, split by what a bind failure MEANS. A
    // specific `Address` ALSO binds loopback, so the dev launch scripts and
    // each engine's Apply-restart callback keep reaching the gateway over
    // `127.0.0.1`. `bind_plan` makes loopback the REQUIRED half and the
    // configured address the retryable one, since at boot it may not exist yet.
    let plan = net_config::bind_plan(&bind_choice, port);
    let handle = axum_server::Handle::new();
    install_shutdown(handle.clone());

    let tls_cert = std::env::var("LUCIDOS_TLS_CERT").ok();
    let tls_key = std::env::var("LUCIDOS_TLS_KEY").ok();
    let tls = match (tls_cert, tls_key) {
        (Some(cert), Some(key)) => {
            Some(axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key).await?)
        }
        _ => None,
    };

    // The required addresses, bound BEFORE anything is served so a failure is
    // fatal and reported as one.
    let mut serving = Vec::with_capacity(plan.required.len());
    for addr in plan.required {
        let listener = bind_and_log(addr, tls.is_some())?;
        serving.push(serve_listener(
            listener,
            router.clone(),
            handle.clone(),
            tls.clone(),
        ));
    }

    // The optional ones, each retried in its own task until the address exists.
    // Recorded as pending first so `/~/api/v1/health` is honest from the very
    // first request, rather than only after the first failed attempt.
    if !plan.optional.is_empty() {
        if let Ok(mut pending) = state.inner.pending_binds.write() {
            pending.extend(plan.optional.iter().copied());
        }
        for addr in plan.optional {
            tokio::spawn(serve_optional_address(
                addr,
                router.clone(),
                handle.clone(),
                tls.clone(),
                state.clone(),
            ));
        }
    }

    // Serve the required addresses concurrently under the one shared shutdown
    // `Handle`. Every one is already bound, so this ends only on shutdown or on
    // a listener failing mid-flight.
    futures::future::try_join_all(serving).await?;
    Ok(())
}

/// Bind one address and announce it, in that order. Announcing first would
/// leave a gateway that died on `EADDRNOTAVAIL` claiming to listen on the
/// address it had just failed to acquire.
fn bind_and_log(addr: SocketAddr, tls: bool) -> std::io::Result<std::net::TcpListener> {
    let listener = std::net::TcpListener::bind(addr)?;
    if tls {
        crate::log!("[Gateway] listening on https://{addr} (TLS)");
    } else {
        crate::log!("[Gateway] listening on http://{addr}");
    }
    Ok(listener)
}

/// Serve an already-bound listener, with or without TLS. Boxed because the two
/// arms are different `axum_server` acceptor types and the caller keeps them in
/// one collection.
fn serve_listener(
    listener: std::net::TcpListener,
    router: Router,
    handle: axum_server::Handle,
    tls: Option<axum_server::tls_rustls::RustlsConfig>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>> {
    match tls {
        Some(cfg) => Box::pin(
            axum_server::from_tcp_rustls(listener, cfg)
                .handle(handle)
                .serve(router.into_make_service()),
        ),
        None => Box::pin(
            axum_server::from_tcp(listener)
                .handle(handle)
                .serve(router.into_make_service()),
        ),
    }
}

/// First re-attempt delay for an optional bind, and the ceiling the backoff
/// doubles up to. A tailnet interface takes seconds to appear after login, so
/// the first retries are quick; the ceiling keeps a machine that never joins
/// from spinning.
const OPTIONAL_BIND_RETRY_START: Duration = Duration::from_secs(1);
const OPTIONAL_BIND_RETRY_MAX: Duration = Duration::from_secs(30);

/// Backoff schedule for an optional bind: double, capped. Pure so the schedule
/// is pinned by a test rather than by reading the loop.
fn next_bind_retry_delay(current: Duration) -> Duration {
    std::cmp::min(current * 2, OPTIONAL_BIND_RETRY_MAX)
}

/// Hold one optional address for the life of the process: bind it when it
/// exists, serve it, and go back to waiting if it goes away.
///
/// This is what makes a configured tailnet or LAN address non-fatal. At boot,
/// launchd starts the service before `tailscaled` has assigned the machine's
/// `100.x` address, so binding it fails with `EADDRNOTAVAIL`. Retrying here
/// means the listener appears a few seconds later, with nothing for the user to
/// do, while loopback has been serving the whole time.
///
/// Logging is one line per state change rather than one per attempt. The first
/// failure states the reason AND the retry cadence, so the silence that follows
/// reads as "still retrying" rather than as a process that gave up.
async fn serve_optional_address(
    addr: SocketAddr,
    router: Router,
    handle: axum_server::Handle,
    tls: Option<axum_server::tls_rustls::RustlsConfig>,
    state: GatewayState,
) {
    let mut delay = OPTIONAL_BIND_RETRY_START;
    let mut reported: Option<std::io::ErrorKind> = None;
    loop {
        match bind_and_log(addr, tls.is_some()) {
            Ok(listener) => {
                if let Ok(mut pending) = state.inner.pending_binds.write() {
                    pending.remove(&addr);
                }
                reported = None;
                // `Ok` is the shutdown handle firing, so the whole process is
                // going down. An `Err` is the listener failing because the
                // interface went away, so fall through to re-binding.
                match serve_listener(listener, router.clone(), handle.clone(), tls.clone()).await {
                    Ok(()) => return,
                    Err(e) => {
                        crate::log!("[Gateway] stopped listening on {addr} ({e}); re-binding");
                        if let Ok(mut pending) = state.inner.pending_binds.write() {
                            pending.insert(addr);
                        }
                    }
                }
                // Backoff is NOT reset by a successful bind, only by a serve
                // that ran to shutdown, which returns above. An address that
                // binds and then fails to serve would otherwise re-bind and
                // re-fail with no delay, spinning a core. Keeping the schedule
                // makes a flapping interface back off like an absent one.
                tokio::time::sleep(delay).await;
                delay = next_bind_retry_delay(delay);
            }
            Err(e) => {
                if reported != Some(e.kind()) {
                    reported = Some(e.kind());
                    crate::log!(
                        "[Gateway] cannot bind {addr} yet ({e}); serving loopback meanwhile and \
                         retrying every {}s at most until it appears",
                        OPTIONAL_BIND_RETRY_MAX.as_secs()
                    );
                }
                tokio::time::sleep(delay).await;
                delay = next_bind_retry_delay(delay);
            }
        }
    }
}

fn permissive_cors_enabled() -> bool {
    permissive_cors_enabled_value(std::env::var("LUCIDOS_PERMISSIVE_CORS").ok().as_deref())
}

fn permissive_cors_enabled_value(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("1" | "true" | "yes" | "on"))
}

/// Gateway-own health (`/~/api/v1/health`). The launcher polls this.
///
/// `status` stays `ok` while an address is pending: the gateway IS serving, and
/// the launcher's poll must not be held back by a tailnet that has not come up.
/// The degraded reachability is reported alongside, in `pending_binds`, so it
/// is inspectable without reading the log.
async fn gateway_health(State(state): State<GatewayState>) -> axum::Json<serde_json::Value> {
    let count = state.inner.routes.read().map(|r| r.len()).unwrap_or(0);
    let pending: Vec<String> = state
        .inner
        .pending_binds
        .read()
        .map(|p| p.iter().map(|a| a.to_string()).collect())
        .unwrap_or_default();
    axum::Json(serde_json::json!({
        "status": "ok",
        "role": "gateway",
        "release": crate::LUCIDOS_RELEASE,
        "workspaces": count,
        "pending_binds": pending,
    }))
}

/// Everything not handled by a gateway route:
///   * `/`, the smart root: redirect to a sole workspace, else the picker.
///   * `/~/…`, the picker shell and its assets, under the sigil namespace.
///   * `/<slug>/…`, proxied to the matching engine.
async fn fallback(State(state): State<GatewayState>, req: axum::extract::Request) -> Response {
    let path = req.uri().path().to_string();

    if path == "/" {
        // Exactly one workspace drops the user straight in.
        if let Some(slug) = state.sole_workspace() {
            return redirect(&format!("/{slug}/"));
        }
        return serve_picker_shell(&state);
    }

    if path == format!("/{SIGIL}") {
        return redirect(&format!("/{SIGIL}/"));
    }
    if let Some(rest) = path.strip_prefix(&format!("/{SIGIL}/")) {
        return serve_sigil(&state, rest, req).await;
    }

    // Proxy to that workspace's engine. The one exception is the manifest: a
    // gateway-fronted install must cover the whole gateway origin, not only
    // `/<slug>/`, or switching workspaces leaves the installed PWA's scope.
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
            // A document navigation to a workspace stuck past
            // [`BOOT_ESCAPE_BUDGET`] escapes to the manual "Back to workspaces"
            // page. An alive-but-unreachable engine is never marked
            // `Unhealthy`, so the time budget is the only honest signal.
            // Non-document traffic still proxies, and the frontend owns its own
            // disconnected recovery.
            if is_document_navigation(&req) {
                // A TERMINAL failure outranks the time budget, because we
                // already know this workspace will never come up. A RETRYING one
                // does not land here: it renders as the ordinary splash label
                // below, keeping the auto-refresh that carries the user in the
                // moment an attempt succeeds.
                if let Some(failure) = state.boot_failure(&slug).filter(|f| f.is_terminal()) {
                    return proxy::failed_page(&failure.message());
                }
                if boot_window_stalled(state.boot_elapsed(&slug)) {
                    return proxy::stalled_page();
                }
            }
            let target = format!("{}://127.0.0.1:{port}", state.engine_scheme());
            // The route is set the instant `bring_up` spawns the engine, while
            // it is still Booting. So a cold-open navigation lands HERE rather
            // than on the no-route branch below. The boot phase goes with it so
            // the proxy's connect-failure splash can narrate what the engine
            // reported. A transient restart has no phase set.
            let boot_label = state.boot_splash_label(&slug);
            proxy::proxy(&state.inner.proxy_client, &target, &slug, &boot_label, req).await
        }
        None => {
            // No live route. A registered-but-stopped workspace lazy-starts on
            // a document navigation and serves the boot window, whose
            // auto-refresh lands once the engine is healthy. Never lazy-start
            // on API, SSE, asset or service-worker traffic from an open tab.
            // Otherwise the picker's Stop shuts the engine down only for the
            // stopped app to resurrect it.
            let registered = state.inner.registry.lock().unwrap().contains(&slug);
            if registered {
                if rest == "manifest.json" {
                    return serve_workspace_manifest(&state, &slug);
                }
                if is_document_navigation(&req) {
                    // Kick the lazy-start in the background and return the boot
                    // window at once, rather than blocking this response on a
                    // multi-second provision and spawn. The page's auto-refresh
                    // lands once the engine is healthy.
                    let st = state.clone();
                    let id = slug.clone();
                    tokio::spawn(async move { st.lazy_start(&id).await });
                    // Default until the background lazy-start records a phase.
                    // The meta-refresh picks up later phases.
                    return proxy::starting_page(&state.boot_splash_label(&slug));
                }
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!("workspace '{slug}' is stopped"),
                )
                    .into_response();
            }
            // Unknown slug. On a document navigation, send the browser to the
            // picker list rather than a 404 dead-end: the PWA cold-start head
            // redirect opens the remembered workspace with no existence check,
            // so a since-deleted one must land somewhere recoverable. `?pick`
            // also makes that head redirect stand down, so the picker renders
            // its list instead of bouncing back here in a loop. Non-document
            // traffic still gets a 404.
            if is_document_navigation(&req) {
                return redirect(&format!("/{SIGIL}/?pick"));
            }
            (StatusCode::NOT_FOUND, format!("unknown workspace '{slug}'")).into_response()
        }
    }
}

/// The refusal when a create, rename or restore asks for a display name another
/// workspace already carries.
///
/// It quotes the existing name AS STORED rather than what the user typed. The
/// match is case- and space-insensitive, so "PersonAAA" must come back as the
/// "personaaa" they can see in the list.
fn name_taken_message(existing_name: &str) -> String {
    format!("You already have a workspace called \"{existing_name}\". Choose a different name.")
}

/// Do these two display names count as the same one? Trimmed and
/// case-insensitive, matching [`Registry::find_by_display_name`], so an
/// in-flight restore's reservation compares the way a registered name does.
fn names_match(reserved: &Option<String>, name: &str) -> bool {
    reserved
        .as_deref()
        .is_some_and(|r| r.trim().to_lowercase() == name.trim().to_lowercase())
}

/// The refusal when the name is not in the registry yet but a running restore is
/// about to commit it. Distinct wording from [`name_taken_message`] because
/// there is no workspace to point at yet, and because waiting is a real option.
fn name_being_restored_message(name: &str) -> String {
    format!(
        "A restore in progress is already creating a workspace called \"{name}\". \
         Choose a different name, or wait for it to finish."
    )
}

/// The refusal when a restore asks for an address another workspace already
/// holds.
///
/// It names the workspace AS THE PICKER LISTS IT and states the address they
/// collide on. Those are two different strings the moment anyone renames
/// anything, because the address is frozen at create time. Naming a workspace
/// no row on screen carries is unanswerable for the user. Mirrors what the
/// picker predicts client-side, so the two checks cannot tell different
/// stories.
fn address_taken_message(slug: &str, existing_name: &str) -> String {
    format!(
        "The address /{slug}/ is already taken by \"{existing_name}\". \
         Choose a different name, or delete that workspace first."
    )
}

/// Should [`GatewayState::boot_all`] bring this workspace up?
///
/// The three yeses are different questions. `healthy` is an engine that
/// outlived the gateway and needs re-adopting. `restore` is one the last
/// teardown stopped (see [`crate::next_boot`]), and `autostart` is the user's
/// boot posture. A restore does NOT consult `autostart`: a restart must return
/// what it took, whatever the flag says.
fn should_bring_up(ws: &Workspace, healthy: bool, restore: &HashSet<String>) -> bool {
    healthy || restore.contains(&ws.id) || ws.autostart
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

/// Serve a path under the sigil namespace, where `rest` is the path AFTER
/// `/~/`. A real bundled asset streams from `static_dir`, and anything else
/// falls back to the picker shell.
async fn serve_sigil(state: &GatewayState, rest: &str, req: axum::extract::Request) -> Response {
    let Some(dir) = state.inner.static_dir.clone() else {
        return (StatusCode::NOT_FOUND, "no frontend configured").into_response();
    };
    // Serving the raw `dist/index.html` here would carry no base tag, so the
    // bundle would render the app rather than the picker. Mirrors the engine's
    // own `serve_frontend` index special-case.
    if rest.is_empty() || rest == "index.html" {
        return serve_picker_shell(state);
    }
    // The PWA manifest needs a picker-specific re-stamp (see
    // `serve_picker_manifest`), so it must NOT be served verbatim.
    if rest == "manifest.json" {
        return serve_picker_manifest(state);
    }
    // Reconstruct the request with the sigil stripped so `ServeDir` resolves
    // the asset against `static_dir`.
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
        // No such asset. The picker is a SPA, so serve its shell.
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

/// Serve the picker's PWA manifest, re-stamped from the bundled one so the
/// installed picker keeps workspace navigation inside the standalone PWA.
///
/// The bundled manifest declares `start_url` and `scope` as `"."`. Here that
/// would scope the installed PWA to `/~/` alone, so tapping a workspace would
/// navigate out of scope and open a browser.
fn serve_picker_manifest(state: &GatewayState) -> Response {
    serve_gateway_manifest(state, &format!("/{SIGIL}/"), &format!("/{SIGIL}/"))
}

/// Serve a gateway-fronted workspace's PWA manifest. Direct engine access keeps
/// the bundled relative manifest, and therefore a per-workspace scope. Gateway
/// access widens `scope` to `/` so in-app workspace switches stay inside the
/// PWA installed from the gateway's stable port.
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
            // Degrade to a minimal valid manifest carrying the gateway scope,
            // so a missing one does not lose in-app navigation.
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
/// `/` so the installed PWA covers the picker and every `/<slug>/` workspace.
/// The caller supplies `start_url` and `id`. Every other field is preserved
/// from the source, and relative icon refs stay relative to the manifest URL. A
/// malformed or empty source degrades to a minimal manifest.
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
/// ref in the document resolves against it. Falls back to prepending when there
/// is no `<head>`. Deliberately duplicates the engine's stamping (ADR 0014 §1).
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

/// Resolve the gateway's base dir. `LUCIDOS_GATEWAY_DATA` wins, else
/// `~/.lucidos/gateway`. Packaged sets it to the OS app-data dir.
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

/// Raise the file-descriptor limit. The gateway holds an inbound and an
/// outbound socket per proxied connection, and SSE streams are long-lived.
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

    // ── Reaping a stopped engine ─────────────────────────────────────────────
    //
    // A teardown that SIGNALS without WAITING leaves a `<defunct>` engine behind
    // for the gateway's whole lifetime. Both arms of `stop_engine_process` are
    // exercised against a real fork: a zombie is a property of the process
    // table, and no test that stubs the process layer can produce one.

    /// A throwaway workspace dir, named per test and per process so parallel
    /// tests never share one.
    #[cfg(unix)]
    fn reap_scratch_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lucidos-gw-reap-{}-{}-{:?}",
            label,
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join(".lucidos")).unwrap();
        dir
    }

    /// Block until `pid` has left the process table ENTIRELY, rather than
    /// merely exited. `ps` prints a state for a zombie and nothing at all for a
    /// reaped pid, which is the distinction under test. Never calls `waitpid`
    /// itself, which would perform the reap it is checking for.
    ///
    /// An unspawnable `ps` PANICS rather than reading as gone. Empty means the
    /// pid is absent, so defaulting to it on an `Err` would make a broken probe
    /// report a clean process table. The sibling `wait_until_defunct` in
    /// `stack.rs` can default, because there empty means "not defunct yet",
    /// which fails safe.
    #[cfg(unix)]
    fn wait_until_reaped(pid: u32) -> bool {
        for _ in 0..400 {
            let out = std::process::Command::new("ps")
                .args(["-o", "state=", "-p", &pid.to_string()])
                .output()
                .expect("ps must run: without it this helper cannot observe the process table");
            let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if state.is_empty() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// A minimal stack pointed at `dir`. Only `engine` and `resolved_dir` matter
    /// to `stop_engine_process`; the rest is filler.
    #[cfg(unix)]
    fn reap_test_stack(engine: Option<std::process::Child>, dir: PathBuf) -> StackRuntime {
        StackRuntime {
            ws: Workspace {
                id: "reap-test".into(),
                name: "Reap Test".into(),
                dir: dir.to_string_lossy().into_owned(),
                port: 5199,
                database_url: None,
                autostart: false,
            },
            resolved_dir: dir,
            pg: PgHandle::External,
            engine,
            health: Health::Booting,
            restart_attempts: 0,
            health_misses: 0,
            last_spawn: None,
            last_error: None,
            last_unread: None,
        }
    }

    /// A stand-in engine: our own child, killed by the same SIGUSR1 the real one
    /// stops on (the default disposition terminates it).
    #[cfg(unix)]
    fn spawn_stand_in_engine() -> std::process::Child {
        std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep")
    }

    /// We hold the `Child`, so the wait rides it.
    #[cfg(unix)]
    #[test]
    fn stopping_an_engine_we_hold_the_handle_for_reaps_it() {
        let dir = reap_scratch_dir("handle");
        let child = spawn_stand_in_engine();
        let pid = child.id();
        let mut s = reap_test_stack(Some(child), dir.clone());

        stop_engine_process(&mut s);

        assert!(
            wait_until_reaped(pid),
            "an engine stopped through its Child handle is still in the process \
             table (pid {pid})"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `reload_gateway` re-execs the gateway image in place. The pid is
    /// unchanged and the engine is STILL our child, while the `Child` handle
    /// died with the old image. The fresh image re-adopts it with
    /// `engine: None` and only the pidfile.
    #[cfg(unix)]
    #[test]
    fn stopping_a_re_adopted_engine_reaps_it_too() {
        let dir = reap_scratch_dir("readopted");
        let child = spawn_stand_in_engine();
        let pid = child.id();
        std::fs::write(dir.join(".lucidos/engine.pid"), pid.to_string()).unwrap();
        // Exactly what `execv` does to the handle: dropped without a wait,
        // while the process carries on as our child.
        drop(child);

        let mut s = reap_test_stack(None, dir.clone());
        stop_engine_process(&mut s);

        assert!(
            wait_until_reaped(pid),
            "a re-adopted engine was signalled but never waited on, so pid {pid} \
             is now a zombie for the gateway's lifetime"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A pid that is NOT our child must not hold the reaping thread, and must
    /// not stall the caller. `waitpid` answers `ECHILD` at once for one, which is
    /// what makes a BLOCKING wait safe to aim at a pidfile a previous gateway
    /// process wrote. The fixture is a child we have already reaped ourselves,
    /// so the pid is genuinely not ours and the SIGUSR1 that precedes the wait
    /// can reach nothing (ESRCH).
    #[cfg(unix)]
    #[test]
    fn reclaiming_a_pid_that_is_not_our_child_returns_promptly() {
        let dir = reap_scratch_dir("foreign");
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        let pid = child.id();
        child.wait().expect("reap the fixture");
        std::fs::write(dir.join(".lucidos/engine.pid"), pid.to_string()).unwrap();

        let started = Instant::now();
        stack::reclaim_stale_engine(&dir);

        // Comfortably above the function's own 300ms port-release pause and
        // comfortably below "it blocked on a wait that will never return".
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "reclaiming a pid that is not our child must not block: took {:?}",
            started.elapsed()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── Retrying an optional bind ────────────────────────────────────────────

    #[test]
    fn the_optional_bind_backoff_doubles_and_caps() {
        let mut delay = OPTIONAL_BIND_RETRY_START;
        let mut seen = vec![delay];
        for _ in 0..12 {
            let next = next_bind_retry_delay(delay);
            assert!(
                next >= delay,
                "backoff must never shrink: {next:?} < {delay:?}"
            );
            assert!(
                next <= OPTIONAL_BIND_RETRY_MAX,
                "backoff must stay under the ceiling: {next:?}"
            );
            delay = next;
            seen.push(delay);
        }
        assert_eq!(
            delay, OPTIONAL_BIND_RETRY_MAX,
            "backoff must actually reach the ceiling, not creep toward it"
        );
        assert_eq!(seen[1], Duration::from_secs(2), "first re-attempt doubles");
    }

    #[test]
    fn the_first_optional_bind_retry_is_prompt() {
        // A tailnet interface appears seconds after login, so the schedule has to
        // start well below its ceiling or the common case waits for nothing.
        assert!(OPTIONAL_BIND_RETRY_START < OPTIONAL_BIND_RETRY_MAX);
        assert!(OPTIONAL_BIND_RETRY_START <= Duration::from_secs(1));
    }

    // ── What boot_all brings up ──────────────────────────────────────────────

    fn workspace(id: &str, autostart: bool) -> Workspace {
        Workspace {
            id: id.to_string(),
            name: id.to_string(),
            dir: format!("workspaces/{id}"),
            port: 5000,
            database_url: None,
            autostart,
        }
    }

    // A packaged Restart stops every engine, so nothing is healthy and
    // `autostart` alone would leave the workspace the user was sitting in
    // stopped. A restore must not consult the flag.
    #[test]
    fn a_workspace_the_last_teardown_stopped_comes_back_without_autostart() {
        let restore: HashSet<String> = ["myws".to_string()].into_iter().collect();
        assert!(should_bring_up(&workspace("myws", false), false, &restore));
    }

    #[test]
    fn a_healthy_or_autostart_workspace_comes_up_with_no_restore_record() {
        let empty = HashSet::new();
        assert!(
            should_bring_up(&workspace("adopted", false), true, &empty),
            "a surviving engine is re-adopted whatever the flag says",
        );
        assert!(should_bring_up(&workspace("always", true), false, &empty));
    }

    // ── Refusing a restore whose address is taken ────────────────────────────

    /// A workspace created as "personal" and later renamed to "personaal" still
    /// holds `/personal/`. The refusal must not name a workspace the picker
    /// does not list.
    #[test]
    fn the_refusal_names_the_workspace_the_user_can_see() {
        let msg = address_taken_message("personal", "personaal");
        assert!(msg.contains("personaal"), "{msg}");
        assert!(msg.contains("/personal/"), "{msg}");
        assert!(
            !msg.contains("named \"personal\""),
            "must not claim a name no workspace has: {msg}",
        );
    }

    // A restore holds its name for minutes with nothing in the registry to show
    // for it, so create and rename consult the running slot as well. Without
    // that, a rename during a restore lands a duplicate at commit time.
    #[test]
    fn an_in_flight_restore_reserves_its_name_the_same_way_the_registry_does() {
        let reserved = Some("Personal Notes".to_string());
        for probe in ["Personal Notes", "personal notes", "  PERSONAL NOTES  "] {
            assert!(names_match(&reserved, probe), "probe {probe:?}");
        }
        assert!(!names_match(&reserved, "something else"));
        // No restore running reserves nothing.
        assert!(!names_match(&None, "Personal Notes"));
    }

    #[test]
    fn the_restore_reservation_refusal_offers_waiting_as_the_other_way_out() {
        let msg = name_being_restored_message("personal");
        assert!(msg.contains("\"personal\""), "{msg}");
        assert!(msg.contains("restore in progress"), "{msg}");
        assert!(msg.contains("wait"), "{msg}");
    }

    #[test]
    fn the_duplicate_name_refusal_quotes_the_name_as_stored() {
        // The match is case- and space-insensitive, so typing "PersonAAA" must
        // be answered with the "personaaa" the picker actually lists.
        let msg = name_taken_message("personaaa");
        assert!(msg.contains("\"personaaa\""), "{msg}");
        assert!(msg.contains("different name"), "{msg}");
    }

    #[test]
    fn the_refusal_states_both_ways_out() {
        let msg = address_taken_message("work", "work");
        assert!(msg.contains("different name"), "{msg}");
        assert!(msg.contains("delete"), "{msg}");
    }

    // Stop must stick. A workspace the user stopped is not running at teardown,
    // so it is not in the record, and nothing else may start it.
    #[test]
    fn a_stopped_workspace_stays_stopped() {
        let restore: HashSet<String> = ["other".to_string()].into_iter().collect();
        assert!(!should_bring_up(
            &workspace("stopped", false),
            false,
            &restore
        ));
        assert!(!should_bring_up(
            &workspace("stopped", false),
            false,
            &HashSet::new()
        ));
    }

    /// A worktree-rooted engine binary is refused at boot with the corrective
    /// command (ADR 0021).
    #[test]
    fn validate_engine_bin_errors_on_coding_agent_worktree() {
        // The published launch path a worktree `-b` produces (ADR 0063).
        let path =
            Path::new("/w/dev/.lucidos/worktrees/thread-abc/.launch/debug/plain/lucidos-engine");
        let err = validate_engine_bin(path).expect_err("worktree engine bin must error");
        let msg = err.to_string();
        assert!(msg.contains("coding-agent worktree"), "{msg}");
        assert!(
            msg.contains("web-dev.sh"),
            "message must be actionable: {msg}"
        );
        // Fires before the existence check, so an already-deleted orphaned
        // worktree still reports the real reason rather than "does not exist".
        assert!(!msg.contains("does not exist"), "{msg}");
    }

    #[test]
    fn validate_engine_bin_errors_on_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("lucidos-engine");
        let err = validate_engine_bin(&missing).expect_err("missing engine must error");
        let msg = err.to_string();
        assert!(msg.contains("does not exist"), "{msg}");
        assert!(
            msg.contains(missing.to_str().unwrap()),
            "names the path: {msg}"
        );
    }

    #[test]
    fn validate_engine_bin_errors_on_directory() {
        let dir = tempfile::tempdir().unwrap();
        let err = validate_engine_bin(dir.path()).expect_err("a dir is not a regular file");
        assert!(err.to_string().contains("not a regular file"));
    }

    #[cfg(unix)]
    #[test]
    fn validate_engine_bin_errors_on_non_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("lucidos-engine");
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = validate_engine_bin(&bin).expect_err("non-exec must error");
        assert!(err.to_string().contains("not executable"));
    }

    #[cfg(unix)]
    #[test]
    fn validate_engine_bin_accepts_executable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("lucidos-engine");
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(validate_engine_bin(&bin).is_ok());
    }

    #[test]
    fn resolve_static_dir_requires_index_html_when_packaged() {
        let dir = tempfile::tempdir().unwrap();
        // packaged + dir exists but no index.html → fatal, naming the path.
        let err = resolve_static_dir(Some(dir.path().to_path_buf()), true)
            .expect_err("missing index.html must error when packaged");
        assert!(err.to_string().contains("index.html"), "{err}");
    }

    #[test]
    fn resolve_static_dir_tolerates_missing_index_in_dev() {
        // Dev boots the gateway before the frontend build, so a missing
        // index.html must NOT abort — it returns the dir and warns.
        let dir = tempfile::tempdir().unwrap();
        let resolved = resolve_static_dir(Some(dir.path().to_path_buf()), false)
            .expect("dev with no index.html yet must not abort");
        assert_eq!(resolved.as_deref(), Some(dir.path()));
    }

    #[test]
    fn resolve_static_dir_accepts_dir_with_index() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), b"<html>").unwrap();
        let resolved = resolve_static_dir(Some(dir.path().to_path_buf()), true).unwrap();
        assert_eq!(resolved.as_deref(), Some(dir.path()));
    }

    #[test]
    fn resolve_static_dir_unset_is_fatal_when_packaged_but_ok_in_dev() {
        assert!(
            resolve_static_dir(None, true).is_err(),
            "packaged build with no static dir must fail fast"
        );
        assert!(
            matches!(resolve_static_dir(None, false), Ok(None)),
            "dev with no static dir is allowed (picker reports 'no frontend configured')"
        );
    }

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

    /// Locks the exact redirect target the frontend guard depends on: the sigil
    /// plus `?pick`. The PWA cold-start head redirect opens the remembered
    /// workspace with no existence check. `?pick` makes it stand down, so the
    /// picker renders instead of looping.
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

    /// The bundled manifest's relative `scope: "."` would scope the installed
    /// picker PWA to `/~/`. Opening a workspace then leaves the scope and iOS
    /// opens an in-app browser, so the picker manifest must scope to `/`.
    #[test]
    fn picker_manifest_widens_scope_and_starts_at_picker() {
        // The real bundled shape: relative start_url, scope and icons.
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

        // Scope covers the whole origin; start_url and id are the picker.
        assert_eq!(out["scope"], "/");
        assert_eq!(out["start_url"], "/~/");
        assert_eq!(out["id"], "/~/");
        // Everything else is preserved. Icons stay relative, and resolve
        // against `/~/manifest.json`.
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
        assert!(!boot_window_stalled(Some(
            BOOT_ESCAPE_BUDGET - Duration::from_secs(1)
        )));
        // At/past budget → escape to the manual "Back to workspaces" page.
        assert!(boot_window_stalled(Some(BOOT_ESCAPE_BUDGET)));
        assert!(boot_window_stalled(Some(
            BOOT_ESCAPE_BUDGET + Duration::from_secs(60)
        )));
    }

    #[test]
    fn sum_unread_skips_unknown_counts() {
        // Empty / all-unknown → 0 (no running workspace contributes a badge).
        assert_eq!(sum_unread(Vec::<Option<u64>>::new()), 0);
        assert_eq!(sum_unread([None, None]), 0);
        // A stopped/unreachable workspace (None) contributes 0, not a stale value.
        assert_eq!(sum_unread([Some(3), None, Some(4)]), 7);
        assert_eq!(sum_unread([Some(0), Some(2)]), 2);
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
    // The contract these pin: NEVER cull an alive engine. Respawn ONLY a
    // process that has actually exited.

    /// `since_spawn` comfortably past the boot grace, so an established engine.
    /// Also past the respawn backoff, since `BOOT_GRACE` is the larger.
    fn established() -> Duration {
        BOOT_GRACE + Duration::from_secs(1)
    }

    /// [`respawn_decision`] with no terminal boot failure — the ordinary
    /// supervision policy every case below exercises. The terminal-failure
    /// override has its own tests.
    fn decide(
        outcome: ProbeOutcome,
        alive: bool,
        since_spawn: Duration,
        misses: u32,
        restart_attempts: u32,
    ) -> SuperviseAction {
        respawn_decision(outcome, alive, since_spawn, misses, restart_attempts, false)
    }

    /// A reported terminal boot failure short-circuits to Unhealthy on the
    /// FIRST probe: no backoff, no restart cap. Retrying re-runs the identical
    /// failure.
    #[test]
    fn terminal_boot_failure_marks_unhealthy_immediately() {
        for outcome in [
            ProbeOutcome::Slow,
            ProbeOutcome::Unreachable,
            ProbeOutcome::Other,
        ] {
            for alive in [true, false] {
                assert_eq!(
                    respawn_decision(outcome, alive, Duration::ZERO, 0, 0, true),
                    SuperviseAction::MarkUnhealthy,
                    "outcome={outcome:?} alive={alive} must not be retried",
                );
            }
        }
    }

    /// The override must not hijack a WORKING engine: a healthy probe still wins,
    /// so a stale failure flag can never cull a workspace that came back up.
    #[test]
    fn terminal_boot_failure_never_overrides_a_healthy_probe() {
        assert_eq!(
            respawn_decision(ProbeOutcome::Healthy, true, established(), 0, 0, true),
            SuperviseAction::Healthy
        );
    }

    #[test]
    fn healthy_probe_keeps_engine() {
        assert_eq!(
            decide(ProbeOutcome::Healthy, true, established(), 0, 0),
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
                decide(outcome, true, Duration::from_secs(1), 999, 0),
                SuperviseAction::Booting,
                "outcome={outcome:?}"
            );
        }
    }

    #[test]
    fn within_backoff_waits() {
        // Just (re)spawned, not yet healthy → don't respawn again immediately.
        assert_eq!(
            decide(
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
        // An established, alive engine is never respawned: not on one slow
        // probe, not on a long run of misses, whatever the outcome. Respawning
        // a live engine interrupts its threads and feeds the cross-workspace
        // respawn cascade (ADR 0014).
        for outcome in [
            ProbeOutcome::Slow,
            ProbeOutcome::Unreachable,
            ProbeOutcome::Other,
        ] {
            for misses in [1, 5, 50, 9999] {
                assert_eq!(
                    decide(outcome, true, established(), misses, 0),
                    SuperviseAction::Wait,
                    "outcome={outcome:?} misses={misses}"
                );
            }
        }
    }

    #[test]
    fn alive_engine_at_restart_cap_still_waits_never_unhealthy() {
        // Liveness gates everything: an alive engine is left alone even with
        // the restart cap's worth of attempts accumulated.
        assert_eq!(
            decide(
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
        // A refused connection is a strong "down" signal, so a small threshold.
        assert_eq!(
            decide(
                ProbeOutcome::Unreachable,
                false,
                established(),
                DEAD_MISS_THRESHOLD - 1,
                0
            ),
            SuperviseAction::Wait
        );
        assert_eq!(
            decide(
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
        // Probe timed out but the process has EXITED, so treat it as dead
        // rather than busy and let the crash recover promptly.
        assert_eq!(
            decide(
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
            decide(
                ProbeOutcome::Unreachable,
                false,
                established(),
                DEAD_MISS_THRESHOLD,
                RESTART_CAP
            ),
            SuperviseAction::MarkUnhealthy
        );
    }

    // ── Provisioning-failure retry policy ──────────────────────────────────
    // Pinned in both directions: a transient failure is retried, a terminal one
    // is not, and the retry is bounded and backed off. Without the first, a
    // gateway that autostarts before Docker Desktop has its daemon socket
    // latches every workspace dead for the gateway's lifetime.

    #[test]
    fn a_transient_provisioning_failure_is_retried() {
        assert_eq!(
            provision_failure_action(ProvisionErrorKind::Transient, 1),
            ProvisionFailureAction::Retry,
            "one Docker hiccup must not be a permanent verdict"
        );
    }

    #[test]
    fn a_terminal_provisioning_failure_latches_on_the_first_attempt() {
        // Same reasoning as a reported terminal boot failure (ADR 0014): the
        // retry re-runs the identical failure, so spending the budget first
        // only delays the message the user needs.
        assert_eq!(
            provision_failure_action(ProvisionErrorKind::Terminal, 1),
            ProvisionFailureAction::Latch
        );
    }

    #[test]
    fn provisioning_retries_are_bounded_by_the_restart_cap() {
        // Walk the loop the supervisor actually walks: attempt 1 is `bring_up`,
        // each later one a `respawn_stack`. It must stop, and stop at the budget.
        let mut attempts = 1;
        while provision_failure_action(ProvisionErrorKind::Transient, attempts)
            == ProvisionFailureAction::Retry
        {
            attempts += 1;
            assert!(
                attempts <= RESTART_CAP,
                "retry loop ran past the budget at attempt {attempts}"
            );
        }
        assert_eq!(
            attempts, RESTART_CAP,
            "the budget must be spent in full before latching"
        );
    }

    #[test]
    fn the_stack_a_transient_failure_leaves_behind_is_one_the_supervisor_respawns() {
        // `health: Unhealthy` makes `supervise_once` skip the stack before it
        // ever probes. `bring_up` leaves `Booting`, one attempt and no engine
        // process, so assert that shape reaches `Respawn` once the backoff and
        // the miss threshold pass.
        let attempts = 1;
        assert_eq!(
            decide(
                ProbeOutcome::Unreachable,
                false, // no engine was spawned, so nothing is alive
                respawn_backoff(attempts) + Duration::from_secs(1),
                DEAD_MISS_THRESHOLD,
                attempts,
            ),
            SuperviseAction::Respawn
        );
    }

    #[test]
    fn a_retry_waits_out_its_backoff_before_running_docker_again() {
        // Provisioning shells out to `docker` several times per attempt, so the
        // gate has to hold even with misses piled up.
        let attempts = 3;
        assert_eq!(
            decide(
                ProbeOutcome::Unreachable,
                false,
                respawn_backoff(attempts) - Duration::from_secs(1),
                999,
                attempts,
            ),
            SuperviseAction::Wait
        );
    }

    #[test]
    fn respawn_backoff_grows_from_the_old_flat_gap_up_to_the_cap() {
        // The first gap is the flat one, so a one-off engine crash recovers
        // promptly.
        assert_eq!(respawn_backoff(0), RESPAWN_BACKOFF);
        // Then it doubles, and never shrinks or overflows however many attempts
        // are claimed.
        let mut previous = respawn_backoff(0);
        for attempts in 1..=64u32 {
            let gap = respawn_backoff(attempts);
            assert!(gap >= previous, "backoff shrank at attempt {attempts}");
            assert!(gap <= RESPAWN_BACKOFF_MAX, "backoff blew the cap");
            previous = gap;
        }
        assert_eq!(respawn_backoff(1), Duration::from_secs(10));
        assert_eq!(respawn_backoff(u32::MAX), RESPAWN_BACKOFF_MAX);
    }

    #[test]
    fn the_retry_budget_outlasts_a_cold_docker_desktop_start() {
        // Why the budget is spent over grown gaps: it has to still be trying
        // when Docker Desktop finishes starting. Sum the gaps the supervisor
        // will actually wait, from the first attempt onward.
        let window: Duration = (1..RESTART_CAP).map(respawn_backoff).sum();
        assert!(
            window >= Duration::from_secs(120),
            "retry window {window:?} is too short to outlast a Docker Desktop cold start"
        );
    }

    // ── Boot-failure disposition ───────────────────────────────────────────

    #[test]
    fn only_a_terminal_boot_failure_stops_the_supervisor() {
        // The retrying failure is the gateway narrating its own backoff.
        // Reading it as terminal would latch precisely the workspace it exists
        // to keep alive.
        let retrying = BootFailure::retrying("The Docker daemon is not running yet.", 2, 5);
        assert!(!retrying.is_terminal());
        assert_eq!(
            respawn_decision(
                ProbeOutcome::Unreachable,
                false,
                established(),
                DEAD_MISS_THRESHOLD,
                2,
                retrying.is_terminal(),
            ),
            SuperviseAction::Respawn
        );
        assert!(BootFailure::terminal("A newer Lucidos migrated this database.").is_terminal());
    }

    #[test]
    fn a_retrying_failure_is_what_the_splash_says() {
        // A failure arm that clears the phase and sets nothing leaves the
        // splash on "Workspace starting…". The actual reason is then buried in
        // the picker's tooltip.
        let retrying = BootFailure::retrying("The Docker daemon is not running yet.", 2, 5);
        assert_eq!(
            splash_label(Some(&retrying), Some(BootPhase::ProvisioningDatabase)),
            "The Docker daemon is not running yet. Retrying… (attempt 2 of 5)",
            "the reason must outrank the phase it failed during"
        );
        // No failure: unchanged phase narration.
        assert_eq!(
            splash_label(None, Some(BootPhase::Migrating)),
            BootPhase::Migrating.label()
        );
        assert_eq!(splash_label(None, None), boot_phase::DEFAULT_LABEL);
        // A terminal failure is rendered by `failed_page`, never as an
        // auto-refreshing label.
        let terminal = BootFailure::terminal("A newer Lucidos migrated this database.");
        assert_eq!(
            splash_label(Some(&terminal), None),
            boot_phase::DEFAULT_LABEL
        );
    }

    // ── Probe staleness guard (probe_result_is_stale) ──────────────────────
    // The supervisor drops the stack lock across the probe, so on re-lock the
    // stack may have changed. These pin which results must be DISCARDED.

    #[test]
    fn removed_stack_result_is_stale() {
        // A concurrent stop or delete removed the stack from the map, so never
        // apply, whatever the generation.
        let t0 = Instant::now();
        assert!(probe_result_is_stale(
            false,
            Some(t0),
            Some(t0),
            Health::Booting
        ));
        assert!(probe_result_is_stale(false, None, None, Health::Healthy));
    }

    #[test]
    fn respawned_stack_result_is_stale() {
        // `last_spawn` moved during the probe, so the probe described the OLD
        // engine and applying it would bounce a just-restarted workspace.
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(1);
        assert!(probe_result_is_stale(
            true,
            Some(t0),
            Some(t1),
            Health::Booting
        ));
        // A re-adopted engine (None) that respawned during the probe (Some) too.
        assert!(probe_result_is_stale(true, None, Some(t1), Health::Booting));
    }

    #[test]
    fn capped_unhealthy_result_is_stale() {
        // Capped Unhealthy while the probe was in flight, so left for a manual
        // retry.
        let t0 = Instant::now();
        assert!(probe_result_is_stale(
            true,
            Some(t0),
            Some(t0),
            Health::Unhealthy
        ));
    }

    #[test]
    fn unchanged_present_result_applies() {
        // Still present, same generation, not capped: apply the probe result.
        let t0 = Instant::now();
        assert!(!probe_result_is_stale(
            true,
            Some(t0),
            Some(t0),
            Health::Healthy
        ));
        assert!(!probe_result_is_stale(
            true,
            Some(t0),
            Some(t0),
            Health::Booting
        ));
    }

    #[test]
    fn readopted_engine_unchanged_applies() {
        // A re-adopted engine keeps `last_spawn == None`, which counts as
        // unchanged, so its healthy probes still apply. Otherwise it never
        // leaves Booting.
        assert!(!probe_result_is_stale(true, None, None, Health::Booting));
        assert!(!probe_result_is_stale(true, None, None, Health::Healthy));
    }
}
