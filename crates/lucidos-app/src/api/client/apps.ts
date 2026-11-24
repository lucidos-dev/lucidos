import { API, API_BASE, json, mutatingFetch, retryTransient, throwIfNotOk, throwIfRefused } from './_core';
import { readDeviceId } from '../../utils/deviceIdHeader';
import { lucidos, WIDGET_PARAMS_QUERY } from '@lucidos/sdk';
import { canonicalWidgetParams, hasWidgetParams, type WidgetParams } from '../../utils/widgetParams';
import type { AffectedTrigger, App, PinnedAppEntry } from '../../store/types';
import type { ApiResult, ArtifactsResponse, UploadResponse } from '../types';

export interface WorkspaceInfo {
  id: string;
  name: string;
  path: string;
  port: number;
  health: 'restoring' | 'booting' | 'healthy' | 'unhealthy';
}

export async function fetchWorkspaces(): Promise<{ workspaces: WorkspaceInfo[] }> {
  return json(`${API}/workspaces`);
}

export function listArtifacts(): Promise<ArtifactsResponse> {
  return lucidos.data.list().then(files => ({
    artifacts: files,
  })) as Promise<ArtifactsResponse>;
}

export function uploadFile(file: File): Promise<UploadResponse> {
  return lucidos.data.upload(file) as Promise<UploadResponse>;
}

export interface PluginArchiveUploadResponse {
  path: string;
  filename: string;
  byte_size: number;
}

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

export interface PluginLocalChangesResult {
  merged: string[];
  conflicted: string[];
  replaced: string[];
  restored: string[];
  removed?: string[];
  saved_paths: string[];
}

export interface PluginConfirmInstallResponse {
  summary: string;
  installed_files: string[];
  removed_files?: string[];
  setup_thread_id?: string;
  local_changes?: PluginLocalChangesResult;
  affected_triggers?: AffectedTrigger[];
}

export async function confirmPluginInstall(
  installId: string,
  keepLocalChanges = true,
): Promise<PluginConfirmInstallResponse> {
  const res = await mutatingFetch(
    `${API}/plugins/install/${encodeURIComponent(installId)}/confirm`
      + `?keep_local_changes=${keepLocalChanges}`,
    { method: 'POST' },
  );
  await throwIfNotOk(res);
  return res.json();
}

export interface PluginProposeUpstreamResponse {
  patch_path: string;
  thread_id: string;
}

export async function proposePluginUpstream(
  id: string,
): Promise<PluginProposeUpstreamResponse> {
  const res = await mutatingFetch(`${API}/plugins/propose-upstream`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ id }),
  });
  await throwIfNotOk(res);
  return res.json();
}

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
  affected_triggers?: AffectedTrigger[];
}

export async function confirmPluginUninstall(uninstallId: string): Promise<PluginConfirmUninstallResponse> {
  const res = await mutatingFetch(
    `${API}/plugins/uninstall/${encodeURIComponent(uninstallId)}/confirm`,
    { method: 'POST' },
  );
  await throwIfNotOk(res);
  return res.json();
}

export async function cancelPluginUninstall(uninstallId: string): Promise<void> {
  const res = await mutatingFetch(
    `${API}/plugins/uninstall/${encodeURIComponent(uninstallId)}/cancel`,
    { method: 'POST' },
  );
  await throwIfNotOk(res);
}

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

export async function recordAppOpenedApi(id: string): Promise<void> {
  const res = await mutatingFetch(`${API}/app/opened?id=${encodeURIComponent(id)}`, {
    method: 'POST',
  });
  await throwIfNotOk(res);
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


export function appUrl(
  appId: string,
  threadId?: string,
  fragment?: string,
  widgetParams?: WidgetParams,
  device: string | null = readDeviceId(),
): string {
  const base = `${API_BASE}/app/${encodeURIComponent(appId)}/`;
  const params = new URLSearchParams();
  if (threadId) params.set('thread_id', threadId);
  if (device) params.set('device', device);
  if (hasWidgetParams(widgetParams)) params.set(WIDGET_PARAMS_QUERY, canonicalWidgetParams(widgetParams));
  const query = params.toString();
  const withQuery = query ? `${base}?${query}` : base;
  return fragment ? `${withQuery}#${fragment}` : withQuery;
}

export function appFileUrl(appId: string, path: string): string {
  const segments = path.split('/').map(encodeURIComponent).join('/');
  return `${API_BASE}/app/${encodeURIComponent(appId)}/${segments}`;
}

export async function postAppCapture(
  requestId: string,
  screenshot: string,
  dom: string,
): Promise<void> {
  const resp = await mutatingFetch(`${API}/app-capture`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ request_id: requestId, device_id: readDeviceId(), screenshot, dom }),
  });
  await throwIfNotOk(resp);
}

export function getPinnedAppUis(deviceId: string): Promise<{ entries: PinnedAppEntry[] }> {
  return json(`${API}/pinned-apps?device_id=${encodeURIComponent(deviceId)}`);
}

export async function pinAppApi(appId: string, deviceId: string): Promise<ApiResult> {
  const result = await json<ApiResult>(`${API}/pinned-apps`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ app_id: appId, device_id: deviceId }),
  });
  return throwIfRefused(result, `the engine refused to pin "${appId}"`);
}

export async function unpinAppApi(appId: string, deviceId: string): Promise<ApiResult> {
  const result = await json<ApiResult>(`${API}/pinned-apps`, {
    method: 'DELETE',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ app_id: appId, device_id: deviceId }),
  });
  return throwIfRefused(result, `the engine refused to unpin "${appId}"`);
}


export function fetchEventTypes(): Promise<string[]> {
  return retryTransient(() => json(`${API}/events/types`));
}

export interface KnowhowEntry {
  id: string;
  name: string;
  description: string;
}

let knowhowEntriesCache: Promise<KnowhowEntry[]> | null = null;

export function fetchKnowhowEntries(): Promise<KnowhowEntry[]> {
  if (!knowhowEntriesCache) {
    knowhowEntriesCache = json<{ knowhow: KnowhowEntry[] }>(`${API}/knowhow`)
      .then(r => r.knowhow)
      .catch(e => { knowhowEntriesCache = null; throw e; });
  }
  return knowhowEntriesCache;
}

export function knowhowPreviewPath(id: string): string {
  return id.startsWith('system-knowhow/') ? `${id}.md` : `knowhow/${id}.md`;
}
