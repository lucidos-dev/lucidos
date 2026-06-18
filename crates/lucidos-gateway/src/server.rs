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

use crate::error::ApiError;
use crate::postgres::{self, PgBackend, PgHandle};
use crate::proxy;
use crate::registry::{self, Registry, Workspace, SIGIL};
use crate::stack::{self, Health, StackRuntime, WorkspaceStatus};
use crate::BoxError;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
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
    /// Single-slot state of the picker's restore-from-backup flow (see
    /// [`RestoreStatus`]). Polled via the control API; never persisted.
    restore: RwLock<RestoreStatus>,
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
                });
            }
        }
        out
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
                    stack::probe_health(&me.inner.health_client, me.engine_scheme(), ws.port).await;
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
                last_spawn: Some(Instant::now()),
                last_error: None,
            },
            Err(e) => {
                crate::log!("[Gateway] workspace '{}' failed to start: {}", ws.id, e);
                StackRuntime {
                    ws: ws.clone(),
                    resolved_dir,
                    pg: PgHandle::External,
                    engine: None,
                    health: Health::Unhealthy,
                    restart_attempts: RESTART_CAP,
                    last_spawn: None,
                    last_error: Some(e.to_string()),
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
        let prov = self.ensure_postgres(ws).await?;

        if stack::probe_health(&self.inner.health_client, self.engine_scheme(), ws.port).await {
            crate::log!("[Gateway] re-adopting healthy engine for '{}'", ws.id);
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
        Ok((prov.handle, Some(child), Health::Booting))
    }

    /// Create a new workspace: reserve a registry entry (id + free port), persist
    /// it, then provision + start the stack. The entry is committed before
    /// provisioning so a provisioning failure leaves a recoverable `Unhealthy`
    /// workspace (retry / delete from the picker) rather than vanishing.
    ///
    /// `autostart` seeds the new entry's flag: the picker's "+ New" passes `false`
    /// (the user opens it now; whether it auto-starts on a future gateway boot is
    /// their per-workspace toggle), while the first-run bootstrap passes `true`
    /// so a fresh install opens straight into a running `default`.
    pub async fn create_workspace(
        &self,
        name: &str,
        autostart: bool,
    ) -> Result<WorkspaceStatus, BoxError> {
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
                autostart,
            };
            reg.add(ws.clone())?;
            reg.save(&self.inner.registry_path)?;
            ws
        };

        self.bring_up(ws.clone()).await;
        let stacks = self.inner.stacks.lock().await;
        let status = match stacks.get(&ws.id) {
            Some(s) => s.lock().await.status(),
            None => WorkspaceStatus {
                id: ws.id.clone(),
                name: ws.name.clone(),
                port: ws.port,
                health: Health::Unhealthy,
                autostart: ws.autostart,
                last_error: Some("stack missing after create".to_string()),
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
                    last_spawn: Some(Instant::now()),
                    last_error: None,
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
        if let Some(stack) = self.inner.stacks.lock().await.get(id) {
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
        if let Some(stack) = self.inner.stacks.lock().await.get(id) {
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
        for stack in stacks {
            let mut s = stack.lock().await;
            // A concurrent delete may have removed this stack from the map (and
            // trashed its dir) while we held only the snapshot Arc. Don't
            // resurrect a deleted workspace's engine + Postgres — skip it.
            if !self.inner.stacks.lock().await.contains_key(&s.ws.id) {
                continue;
            }
            // A stack capped as Unhealthy is left for manual retry/delete.
            if s.health == Health::Unhealthy {
                continue;
            }
            if stack::probe_health(&self.inner.health_client, self.engine_scheme(), s.ws.port).await
            {
                s.health = Health::Healthy;
                s.restart_attempts = 0;
                s.last_error = None;
                continue;
            }

            let since_spawn = s.last_spawn.map(|t| t.elapsed()).unwrap_or(Duration::MAX);
            let alive = engine_process_alive(&mut s);

            // Process alive but not yet healthy, still within its boot window →
            // it's booting. Don't respawn it out from under itself.
            if alive && since_spawn < BOOT_GRACE {
                s.health = Health::Booting;
                continue;
            }

            // Crash backoff: don't respawn more often than RESPAWN_BACKOFF.
            if since_spawn < RESPAWN_BACKOFF {
                continue;
            }
            if s.restart_attempts >= RESTART_CAP {
                s.health = Health::Unhealthy;
                if s.last_error.is_none() {
                    s.last_error =
                        Some("engine failed to become healthy after repeated restarts".to_string());
                }
                crate::log!(
                    "[Gateway] '{}' marked unhealthy after {} restarts",
                    s.ws.id,
                    s.restart_attempts
                );
                continue;
            }
            // Either crashed (dead process) or wedged past the boot grace.
            self.respawn_stack(&mut s).await;
        }
    }
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
            pg_lock: AsyncMutex::new(()),
            proxy_client: proxy::build_client(),
            health_client: stack::build_health_client(),
            registry: Mutex::new(registry),
            stacks: AsyncMutex::new(HashMap::new()),
            starting: AsyncMutex::new(HashSet::new()),
            routes: RwLock::new(HashMap::new()),
            restore: RwLock::new(RestoreStatus::default()),
        }),
    };

    // First run: an empty registry auto-creates `default` (auto-start, so a fresh
    // install opens straight into a running workspace, ADR 0014 §10) and drops
    // the user in.
    let is_empty = state.inner.registry.lock().unwrap().workspaces.is_empty();
    if is_empty {
        crate::log!("[Gateway] empty registry — auto-creating 'default'");
        if let Err(e) = state.create_workspace("Default", true).await {
            crate::log!("[Gateway] failed to auto-create default workspace: {}", e);
        }
    } else {
        state.boot_all().await;
    }

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

    serve(state, gateway_port).await
}

/// Build the gateway router and serve it (TLS when certs are configured, like
/// the engine). `/~/api/v1/health` + `/~/api/v1/control/*` are the gateway's
/// own; every other path falls through to [`fallback`] (smart root, picker
/// static under `/~/`, else proxy `/<slug>/*`).
async fn serve(state: GatewayState, port: u16) -> Result<(), BoxError> {
    let router = Router::new()
        .route("/~/api/v1/health", get(gateway_health))
        .nest("/~/api/v1/control", crate::control::router())
        .fallback(fallback)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port));
    let handle = axum_server::Handle::new();
    install_shutdown(handle.clone());

    let tls_cert = std::env::var("LUCIDOS_TLS_CERT").ok();
    let tls_key = std::env::var("LUCIDOS_TLS_KEY").ok();
    if let (Some(cert), Some(key)) = (tls_cert, tls_key) {
        let cfg = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key).await?;
        crate::log!("[Gateway] listening on https://[::]:{} (TLS)", port);
        axum_server::bind_rustls(addr, cfg)
            .handle(handle)
            .serve(router.into_make_service())
            .await?;
    } else {
        crate::log!("[Gateway] listening on http://[::]:{}", port);
        axum_server::bind(addr)
            .handle(handle)
            .serve(router.into_make_service())
            .await?;
    }
    Ok(())
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
            let target = format!("{}://127.0.0.1:{port}", state.engine_scheme());
            proxy::proxy(&state.inner.proxy_client, &target, &slug, req).await
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
                    return proxy::starting_page();
                }
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!("workspace '{slug}' is stopped"),
                )
                    .into_response();
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
}
