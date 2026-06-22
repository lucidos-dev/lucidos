/**
 * Workspace gateway control plane client (ADR 0014).
 *
 * The gateway serves these under the reserved sigil namespace
 * `/~/api/v1/control/*` (ADR 0014 §2) — an absolute path that can never collide
 * with a workspace slug. This client uses that absolute path deliberately
 * (bypassing `API_BASE`) so the same calls work from the picker (served at `/`
 * with base `/~/`) AND from the in-app switcher inside a workspace (`/<slug>/`):
 * the browser hits `/~/…`, which the gateway owns. In legacy (non-gateway) mode
 * these routes don't exist, so callers treat a failure as "no gateway".
 */

const CONTROL = '/~/api/v1/control';

export type WorkspaceHealth = 'booting' | 'healthy' | 'unhealthy';

export interface WorkspaceStatus {
  id: string;
  name: string;
  port: number;
  health: WorkspaceHealth;
  /** Whether the gateway auto-starts this workspace's engine on boot. The picker
   *  renders a per-workspace toggle bound to it (ADR 0014). */
  autostart: boolean;
  last_error?: string;
}

async function controlJson<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${CONTROL}${path}`, init);
  if (!res.ok) {
    let reason = res.statusText;
    try {
      const body = await res.json();
      if (body?.error) reason = body.error;
    } catch { /* non-JSON body */ }
    throw new Error(reason || `HTTP ${res.status}`);
  }
  // 204 (rename / autostart / delete) and 202 (restart / stop — "accepted,
  // async") are bodyless; calling res.json() on an empty body throws a
  // SyntaxError, which would surface a spurious error and skip the refresh.
  return res.status === 204 || res.status === 202 ? (undefined as T) : res.json();
}

export async function listWorkspaces(): Promise<WorkspaceStatus[]> {
  const body = await controlJson<{ workspaces: WorkspaceStatus[] }>('/workspaces');
  return body.workspaces ?? [];
}

export async function createWorkspace(name: string): Promise<WorkspaceStatus> {
  const body = await controlJson<{ workspace: WorkspaceStatus }>('/workspaces', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name }),
  });
  return body.workspace;
}

export async function renameWorkspace(id: string, name: string): Promise<void> {
  await controlJson<void>(`/workspaces/${encodeURIComponent(id)}/rename`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name }),
  });
}

/** Start a stopped workspace, retry an unhealthy one, or respawn a running one
 *  (all route to the gateway's restart control). */
export async function restartWorkspace(id: string): Promise<void> {
  await controlJson<void>(`/workspaces/${encodeURIComponent(id)}/restart`, {
    method: 'POST',
  });
}

/** Stop a workspace's engine. It stays listed in the picker as stopped (the
 *  registry entry survives — membership is "all ever launched"). */
export async function stopWorkspace(id: string): Promise<void> {
  await controlJson<void>(`/workspaces/${encodeURIComponent(id)}/stop`, {
    method: 'POST',
  });
}

/** Toggle whether the gateway auto-starts this workspace on boot (registry-only;
 *  does not start or stop the engine). */
export async function setAutostart(id: string, enabled: boolean): Promise<void> {
  await controlJson<void>(`/workspaces/${encodeURIComponent(id)}/autostart`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ enabled }),
  });
}

/** Delete-to-trash. `confirm` must equal the workspace's current display name
 *  (the server re-checks the type-the-name confirmation). */
export async function deleteWorkspace(id: string, confirm: string): Promise<void> {
  await controlJson<void>(`/workspaces/${encodeURIComponent(id)}`, {
    method: 'DELETE',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ confirm }),
  });
}

/** Navigate the browser to a workspace (`/<slug>/`, ADR 0014). */
export function openWorkspace(id: string): void {
  window.location.href = `/${encodeURIComponent(id)}/`;
}

// ── Gateway self-update (picker reload control) ────────────────────────────

/** The running gateway's build id plus whether a rebuilt binary is waiting on
 *  disk. Mirrors the Rust `gateway_status` handler. */
export interface GatewayStatus {
  build_id: string;
  update_available: boolean;
}

/** Build id of the running gateway + whether a newer binary is on disk (the dev
 *  picker's "new gateway available" badge). */
export async function getGatewayStatus(): Promise<GatewayStatus> {
  return controlJson<GatewayStatus>('/gateway/status');
}

/** Adopt the rebuilt gateway binary: the gateway re-execs itself (same PID). The
 *  call resolves on the 202; the gateway then briefly drops while the new image
 *  binds and the picker's poll reconnects. */
export async function reloadGateway(): Promise<void> {
  await controlJson<void>('/gateway/reload', { method: 'POST' });
}

// ── Restore from backup (picker) ───────────────────────────────────────────

/** State of the gateway's restore-from-backup flow. Mirrors the Rust
 *  `RestoreStatus` enum; the picker polls `getRestoreStatus()` for it. */
export type GwRestoreStatus =
  | { status: 'idle' }
  | { status: 'running'; id: string; name: string; phase: string }
  | { status: 'completed'; id: string; name: string }
  | { status: 'failed'; name: string; error: string };

export interface RestoreStarted {
  id: string;
  name: string;
}

/** Restore a local encrypted `.enc` backup into a NEW workspace. `name` is sent
 *  only to resolve a collision (otherwise the gateway derives it from the
 *  archive filename). Resolves once the background restore has STARTED (the
 *  returned `{id, name}`); poll {@link getRestoreStatus} for progress. A 409 is
 *  surfaced as an Error whose message is the collision / in-progress reason. */
export async function restoreBackup(file: File, key: string, name?: string): Promise<RestoreStarted> {
  const form = new FormData();
  form.append('file', file);
  form.append('key', key);
  if (name && name.trim()) form.append('name', name.trim());
  // No explicit Content-Type — the browser sets the multipart boundary. Streams
  // the (possibly large) file rather than buffering a JSON-encoded copy.
  const res = await fetch(`${CONTROL}/workspaces/restore`, { method: 'POST', body: form });
  if (!res.ok) {
    let reason = res.statusText;
    try {
      const body = await res.json();
      if (body?.error) reason = body.error;
    } catch { /* non-JSON body */ }
    throw new Error(reason || `HTTP ${res.status}`);
  }
  return res.json();
}

/** Current restore-flow state, for the picker's poll. */
export async function getRestoreStatus(): Promise<GwRestoreStatus> {
  return controlJson<GwRestoreStatus>('/restore-status');
}

/** Dismiss a terminal (completed/failed) restore result. Refused while running. */
export async function clearRestoreStatus(): Promise<void> {
  await controlJson<void>('/restore-status', { method: 'DELETE' });
}

/** Slug a workspace name the way the gateway registry does (lowercase,
 *  non-alphanumerics → single `-`, trimmed), so the picker can predict a
 *  collision against existing workspace ids before uploading. Mirrors
 *  `registry::slugify`. */
export function slugifyWorkspaceName(name: string): string {
  let out = '';
  let prevDash = false;
  for (const ch of name) {
    if (/[a-z0-9]/i.test(ch)) {
      out += ch.toLowerCase();
      prevDash = false;
    } else if (!prevDash && out.length > 0) {
      out += '-';
      prevDash = true;
    }
  }
  out = out.replace(/-+$/, '');
  return out || 'workspace';
}

/** Derive the workspace name a backup archive was produced for, from its
 *  filename (`lucidos-backup-{name}-{YYYYMMDD-HHMMSS}.enc`). Returns null for a
 *  non-archive name. Mirrors `core::backup::parse_workspace_name_from_archive`. */
export function parseWorkspaceNameFromArchive(filename: string): string | null {
  const m = filename.match(/^lucidos-backup-(.+)-\d{8}-\d{6}\.enc$/);
  return m ? m[1] : null;
}
