import { API, API_BASE, json, mutatingFetch, throwIfNotOk } from './_core';
import { lucidos } from '@lucidos/sdk';
import type { App, PinnedAppEntry } from '../../store/types';
import type { ApiResult, ArtifactsResponse, UploadResponse } from '../types';

// --- Workspaces ---
export interface WorkspaceInfo {
  name: string;
  path: string;
  port: number | null;
  engine_running: boolean;
  engine_version: string;
}

export async function fetchWorkspaces(): Promise<{ workspaces: WorkspaceInfo[] }> {
  return json(`${API}/workspaces`);
}

// --- Artifacts (SDK delegation) ---
export function listArtifacts(): Promise<ArtifactsResponse> {
  return lucidos.data.list().then(files => ({
    artifacts: files,
  })) as Promise<ArtifactsResponse>;
}

export function uploadFile(file: File): Promise<UploadResponse> {
  return lucidos.data.upload(file) as Promise<UploadResponse>;
}

// --- Plugins ---
export interface PluginArchiveUploadResponse {
  path: string;
  filename: string;
  byte_size: number;
}

/** Upload a `.lucidos-plugin` archive to a per-request temp directory inside
 *  the workspace. Returns the absolute filesystem path the LLM tool
 *  `install_plugin` can consume. The chat layer then sends a message asking
 *  the LLM to install from that path. */
export async function uploadPluginArchive(file: File): Promise<PluginArchiveUploadResponse> {
  const fd = new FormData();
  fd.append('file', file, file.name);
  const res = await mutatingFetch(`${API}/plugins/upload-archive`, {
    method: 'POST',
    body: fd,
  });
  await throwIfNotOk(res);
  return res.json();
}

export interface PluginConfirmInstallResponse {
  summary: string;
  installed_files: string[];
  /** Set when the plugin shipped `setup` instructions: the Lucidos Agent thread
   *  spawned to walk the user through them. The frontend navigates here. */
  setup_thread_id?: string;
}

/** User accepted the staged install in the install panel. The engine writes
 *  files into `data/`, emits `PluginInstalled`, and (if any `auth-modules/`
 *  files were touched) auto-reloads the WASM signer map. */
export async function confirmPluginInstall(installId: string): Promise<PluginConfirmInstallResponse> {
  const res = await mutatingFetch(
    `${API}/plugins/install/${encodeURIComponent(installId)}/confirm`,
    { method: 'POST' },
  );
  await throwIfNotOk(res);
  return res.json();
}

/** User dismissed the staged install. The engine drops the staged temp dir
 *  and emits `PluginInstallCanceled` for audit. */
export async function cancelPluginInstall(installId: string): Promise<void> {
  const res = await mutatingFetch(
    `${API}/plugins/install/${encodeURIComponent(installId)}/cancel`,
    { method: 'POST' },
  );
  await throwIfNotOk(res);
}

export interface PluginConfirmUninstallResponse {
  summary: string;
  files_deleted: string[];
  files_missing: string[];
}

/** User accepted the staged uninstall in the panel. The engine deletes the
 *  recorded files from `data/`, prunes empty parent dirs, emits
 *  `PluginUninstalled` with the deleted/missing partition, and (if any
 *  `auth-modules/` files were removed) reloads the WASM signer map. */
export async function confirmPluginUninstall(uninstallId: string): Promise<PluginConfirmUninstallResponse> {
  const res = await mutatingFetch(
    `${API}/plugins/uninstall/${encodeURIComponent(uninstallId)}/confirm`,
    { method: 'POST' },
  );
  await throwIfNotOk(res);
  return res.json();
}

/** User dismissed the staged uninstall. No files touched; emits
 *  `PluginUninstallCanceled` for audit. */
export async function cancelPluginUninstall(uninstallId: string): Promise<void> {
  const res = await mutatingFetch(
    `${API}/plugins/uninstall/${encodeURIComponent(uninstallId)}/cancel`,
    { method: 'POST' },
  );
  await throwIfNotOk(res);
}

// --- Apps (SDK delegation) ---
export function listAppsApi(): Promise<App[]> {
  return lucidos.apps.list() as Promise<App[]>;
}

export function deleteAppApi(
  id: string
): Promise<{ commit: string }> {
  return json(`${API}/app?id=${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
}

export function updateAppApi(
  id: string,
  data: { name: string; description: string; instructions?: string }
): Promise<{ commit: string }> {
  return json(`${API}/app?id=${encodeURIComponent(id)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
  });
}

export interface UiSourceFile {
  name: string;
  content: string;
}

export function readAppSourceApi(
  appId: string,
): Promise<{ files: UiSourceFile[] }> {
  return json(
    `${API}/app/${encodeURIComponent(appId)}/source`
  );
}

export function writeAppSourceApi(
  appId: string,
  files: UiSourceFile[]
): Promise<{ commit: string }> {
  return json(
    `${API}/app/${encodeURIComponent(appId)}/source`,
    {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ files }),
    }
  );
}

export function appUrl(
  appId: string,
  commit?: string,
  threadId?: string,
): string {
  const base = `${API_BASE}/app/${encodeURIComponent(appId)}/`;
  const params = new URLSearchParams();
  if (commit) params.set('commit', commit);
  // `thread_id` triggers the engine's WIP-preview branch (see
  // `api/apps.rs::serve_app_ui`) — content comes from the open
  // coding-agent thread's worktree instead of the live workspace data.
  if (threadId) params.set('thread_id', threadId);
  const qs = params.toString();
  return qs ? `${base}?${qs}` : base;
}

export interface AppVersion {
  commit: string;
  message: string;
  timestamp: number;
  author: string;
}

export interface AppVersionsPage {
  versions: AppVersion[];
  has_more: boolean;
}

export async function getAppVersions(appId: string, limit = 10, skip = 0): Promise<AppVersionsPage> {
  return json<AppVersionsPage>(`${API}/app/versions?id=${encodeURIComponent(appId)}&limit=${limit}&skip=${skip}`);
}

// --- App Capture ---
export async function postAppCapture(
  requestId: string,
  screenshot: string,
  dom: string,
): Promise<void> {
  const resp = await mutatingFetch(`${API}/app-capture`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ request_id: requestId, screenshot, dom }),
  });
  await throwIfNotOk(resp);
}

// --- Pinned Apps ---
export function getPinnedAppUis(deviceId: string): Promise<{ entries: PinnedAppEntry[] }> {
  return json(`${API}/pinned-apps?device_id=${encodeURIComponent(deviceId)}`);
}

export function pinAppApi(appId: string, deviceId: string): Promise<ApiResult> {
  return json(`${API}/pinned-apps`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ app_id: appId, device_id: deviceId }),
  });
}

export function unpinAppApi(appId: string, deviceId: string): Promise<ApiResult> {
  return json(`${API}/pinned-apps`, {
    method: 'DELETE',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ app_id: appId, device_id: deviceId }),
  });
}

// --- Events ---
export function fetchEventTypes(): Promise<string[]> {
  return json(`${API}/events/types`);
}

// --- Knowhow ---
export interface KnowhowEntry {
  id: string;
  name: string;
  description: string;
}

/** Process-wide cache for the knowhow list — multiple consumers (trigger form,
 *  file-preview 404 hint) hit the same endpoint and don't need their own copy.
 *  Holds the in-flight promise so concurrent first-callers share the request. */
let knowhowEntriesCache: Promise<KnowhowEntry[]> | null = null;

export function fetchKnowhowEntries(): Promise<KnowhowEntry[]> {
  if (!knowhowEntriesCache) {
    knowhowEntriesCache = json<{ knowhow: KnowhowEntry[] }>(`${API}/knowhow`)
      .then(r => r.knowhow)
      .catch(e => { knowhowEntriesCache = null; throw e; });
  }
  return knowhowEntriesCache;
}

/** `system-knowhow/` ids are already rooted (engine-shipped); bare ids root
 *  under `data/knowhow/`. */
export function knowhowPreviewPath(id: string): string {
  return id.startsWith('system-knowhow/') ? `${id}.md` : `knowhow/${id}.md`;
}
