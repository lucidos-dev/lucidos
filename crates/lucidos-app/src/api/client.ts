import type {
  ApiResult,
  ArtifactsResponse,
  ChatRequestBody,
  CredentialsListResponse,
  DeviceInfo,
  MemoryEntriesResponse,
  MemorySourceResponse,
  MemoryStatsResponse,
  NotificationsResponse,
  TriggersListResponse,
  UploadResponse,
} from './types';
import type { AuthType, Notification, OAuthAccountInfo, PinnedAppEntry, App, TriggerRun } from '../store/types';
import type { RepoDiff } from '../store/store';
import { lucidos } from '@lucidos/sdk';

export const API_BASE = '';
const API = `${API_BASE}/api`;

export class ApiError extends Error {
  constructor(
    public readonly httpCode: number,
    public readonly reason: string,
  ) {
    super(`${httpCode} ${reason}`);
    this.name = 'ApiError';
  }
}

// Inlined to avoid a circular import with devices.ts (which imports json()).
function deviceIdHeader(): Record<string, string> {
  if (typeof localStorage === 'undefined') return {};
  const id = localStorage.getItem('lucidos-device-id');
  return id ? { 'x-lucidos-device-id': id } : {};
}

/** `fetch` wrapped to always send `x-lucidos-device-id` so the engine can
 *  attribute the call to this device. Use for non-JSON-response mutating
 *  endpoints (apply-now, cancel, discard, etc.); JSON endpoints should use
 *  `json()` which already adds the header. */
async function mutatingFetch(url: string, init?: RequestInit): Promise<Response> {
  const headers = { ...deviceIdHeader(), ...(init?.headers as Record<string, string> | undefined) };
  return fetch(url, { ...init, headers });
}

/** Match the `TypeError` messages browsers throw for transport-layer fetch
 *  failures: Safari ("Load failed"), Chrome ("Failed to fetch"), Firefox
 *  ("NetworkError when attempting to fetch resource"). Anything else is a
 *  real bug and must surface, not be silently retried. */
function isTransportError(err: unknown): boolean {
  return err instanceof TypeError
    && /Load failed|Failed to fetch|NetworkError/i.test(err.message);
}

/** Same as `mutatingFetch` but retries once on a transport-layer error
 *  (iOS Safari surfaces stale-connection failures as `TypeError("Load failed")`
 *  after the PWA backgrounds). Use only for endpoints whose backend handler is
 *  idempotent — a retry must be safe to observe a side-effect twice. The
 *  service worker has the equivalent retry for GETs (`fetchWithRetry` in
 *  sw.js); POSTs bypass the SW because iOS WebKit can't reliably clone
 *  request bodies, so the retry has to live here. */
async function mutatingFetchIdempotent(url: string, init?: RequestInit): Promise<Response> {
  try {
    return await mutatingFetch(url, init);
  } catch (err) {
    if (isTransportError(err)) return mutatingFetch(url, init);
    throw err;
  }
}

/** Throw `ApiError` with the body's `{error}` field as the reason if present,
 *  otherwise fall back to `res.statusText`. */
async function throwIfNotOk(res: Response): Promise<void> {
  if (res.ok) return;
  let reason = res.statusText;
  try {
    const body = await res.json();
    if (body?.error) reason = body.error;
  } catch { /* body not JSON, use statusText */ }
  throw new ApiError(res.status, reason);
}

export async function json<T>(url: string, init?: RequestInit, timeoutMs = 10000): Promise<T> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const headers = { ...deviceIdHeader(), ...(init?.headers as Record<string, string> | undefined) };
    const res = await fetch(url, { ...init, headers, signal: controller.signal });
    await throwIfNotOk(res);
    return res.json();
  } finally {
    clearTimeout(timeout);
  }
}

// --- Health ---
export interface HealthInfo {
  status: string;
  workspace: string;
  workspace_path: string;
  started_at: string;
  release?: string;
  release_dirty?: boolean;
  engine_version?: string;
  latest_engine_version?: string;
  latest_tauri_app_version?: string;
}

export async function checkHealth(): Promise<HealthInfo | null> {
  try {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 3000);
    const res = await fetch(`${API}/health`, { signal: controller.signal });
    clearTimeout(timeout);
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}

// --- Chat ---
export async function submitChat(body: ChatRequestBody): Promise<{ event_id: string }> {
  const res = await mutatingFetch(`${API}/chat/stream`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new ApiError(res.status, text || res.statusText);
  }
  return res.json();
}

export async function cancelChat(threadId?: string): Promise<void> {
  const params = threadId ? `?thread_id=${encodeURIComponent(threadId)}` : '';
  const res = await mutatingFetchIdempotent(`${API}/chat/cancel${params}`, { method: 'POST' });
  if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
}

export async function interruptClaudeCode(threadId?: string): Promise<void> {
  const params = new URLSearchParams();
  if (threadId) params.set('thread_id', threadId);
  const qs = params.toString();
  const url = qs ? `${API}/claude-code/interrupt?${qs}` : `${API}/claude-code/interrupt`;
  const res = await mutatingFetchIdempotent(url, { method: 'POST' });
  if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
}

export async function applyNow(threadId: string): Promise<void> {
  const res = await mutatingFetch(`${API}/claude-code/apply-now?thread_id=${encodeURIComponent(threadId)}`, { method: 'POST' });
  await throwIfNotOk(res);
}

export async function sendControlRequest(threadId: string, request: Record<string, string>): Promise<void> {
  await json(`${API}/claude-code/control`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ thread_id: threadId, request }),
  });
}

export interface CCCommandOption {
  value: string;
  label: string;
  description: string;
}

export interface CCCommandParam {
  key: string;
  label?: string;
  placeholder?: string;
  options?: CCCommandOption[];
}

export interface CCCommandDef {
  subtype: string;
  label: string;
  params: CCCommandParam[];
}

export interface CCCommandsResponse {
  control_commands: CCCommandDef[];
  builtin_commands: string[];
  skill_commands: string[];
  current_model: string | null;
  current_reasoning_effort: string | null;
  has_active_session: boolean;
}

/** Model aliases from CC_MODEL_OPTIONS in claude_code.rs */
export type CCModelValue = 'default' | 'sonnet' | 'sonnet[1m]' | 'claude-opus-4-7' | 'claude-opus-4-1' | 'opus' | 'opus[1m]' | 'haiku';

/** Reasoning effort levels from CC_REASONING_EFFORT_OPTIONS in claude_code.rs */
export type CCReasoningEffort = 'low' | 'medium' | 'high' | 'xhigh' | 'max';

export async function fetchCCCommands(threadId?: string): Promise<CCCommandsResponse> {
  const qs = threadId ? `?thread_id=${threadId}` : '';
  return json<CCCommandsResponse>(`${API}/claude-code/commands${qs}`);
}

export async function cancelClaudeCode(apply?: boolean, threadId?: string, discard?: boolean): Promise<void> {
  const params = new URLSearchParams();
  if (apply) params.set('apply', 'true');
  if (discard) params.set('discard', 'true');
  if (threadId) params.set('thread_id', threadId);
  const qs = params.toString();
  const url = qs ? `${API}/claude-code/cancel?${qs}` : `${API}/claude-code/cancel`;
  const res = await mutatingFetch(url, { method: 'POST' });
  await throwIfNotOk(res);
}

export async function discardCCChanges(threadId: string): Promise<void> {
  const res = await mutatingFetch(`${API}/claude-code/discard`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ thread_id: threadId }),
  });
  if (!res.ok) throw new ApiError(res.status, await res.text());
}

import type { AnswerKind } from '../store/thread-events';

/** Answer a CC AskUserQuestion. Backend emits UserQuestionAnswered then
 *  spawns CC --resume with a matching tool_result. Returns true on success;
 *  false for 409 (stale/duplicate) so the UI can re-sync from events. */
export async function answerCCQuestion(
  threadId: string,
  toolUseId: string,
  answer: AnswerKind,
): Promise<boolean> {
  const res = await mutatingFetch(`${API}/claude-code/answer-question`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ thread_id: threadId, tool_use_id: toolUseId, answer }),
  });
  if (res.ok) return true;
  if (res.status === 409) return false; // already answered or no pending question
  throw new ApiError(res.status, await res.text());
}

// --- Changes ---

export interface Change {
  id: string;
  request_id: string;
  thread_id: string | null;
  thread_title: string | null;
  branch_name: string;
  repo_root: string;
  description: string;
  file_count: number;
  files: string[];
  requires_restart: boolean;
  hardened: boolean;
  status: string;
  created_at: string;
  resolved_at: string | null;
  pre_merge_sha: string | null;
  post_merge_sha: string | null;
  commits: string[];
}

/** One thread's contribution to the current restart-required toast: derived
 *  server-side from applied-but-not-yet-restarted changes since engine start. */
export interface ApiRestartGroup {
  thread_id: string | null;
  thread_title: string | null;
  commits: string[];
}

export interface ChangesState {
  pending: Change[];
  applied: Change[];
  total_pending: number;
  restart_required: boolean;
  restart_groups: ApiRestartGroup[];
  client_update_available: boolean;
  has_more_applied: boolean;
}

export async function fetchChanges(params?: {
  limit?: number;
  before?: number;
}): Promise<ChangesState> {
  const qs = new URLSearchParams();
  if (params?.limit != null) qs.set('limit', String(params.limit));
  if (params?.before != null) qs.set('before', String(params.before));
  const q = qs.toString();
  return json(`${API}/changes${q ? `?${q}` : ''}`);
}

export type ApplyStatus = 'applied' | 'noop' | 'hardening' | 'conflict';

/** Response body for POST /api/changes/:id/apply. Mirrors the Rust
 *  `ApplyResult` struct in `crates/lucidos-engine/src/engine/types.rs`. */
export interface ApplyChangeResult {
  status: ApplyStatus;
  change_id: string;
  thread_id: string | null;
  message: string;
  restart_required: boolean;
  /** SHA of main HEAD after the merge. Only set when a real merge
   *  happened (absent for the external-repo handoff and non-`applied`). */
  applied_commit?: string;
  previous_commit?: string;
  commits_applied: number;
  files_changed: number;
  /** Set when `status === 'conflict'` — focus this thread to resolve. */
  conflict_thread_id?: string;
  /** Set when `status === 'hardening'` — track as "applying" until done. */
  review_thread_id?: string;
}

export async function applyChange(id: string): Promise<ApplyChangeResult> {
  return json(`${API}/changes/${id}/apply`, { method: 'POST' });
}

export async function discardChange(id: string): Promise<{ message: string }> {
  return json(`${API}/changes/${id}/discard`, { method: 'POST' });
}

export async function applyAllChanges(): Promise<{ message: string; restart_required: boolean }> {
  return json(`${API}/changes/apply-all`, { method: 'POST' });
}

export async function discardAllChanges(): Promise<{ discarded: number; failed: number; errors: string[] }> {
  return json(`${API}/changes/discard-all`, { method: 'POST' });
}


export async function revertChange(id: string): Promise<{ message: string }> {
  return json(`${API}/changes/${id}/revert`, { method: 'POST' });
}

export interface RepoChangesState {
  pending: Change[];
  applied: Change[];
  has_more: boolean;
}

export async function getChangeById(changeId: string): Promise<Change> {
  return json(`${API}/changes/${changeId}`);
}

export async function getChangeDiff(changeId: string): Promise<RepoDiff> {
  return json(`${API}/changes/${changeId}/diff`);
}

export async function getChangeFileContent(changeId: string, path: string): Promise<string> {
  const params = new URLSearchParams({ path });
  const res = await fetch(`${API}/changes/${encodeURIComponent(changeId)}/file?${params}`);
  if (!res.ok) throw new ApiError(res.status, await res.text());
  return res.text();
}

export async function getRepoChanges(repoId: string, limit?: number, before?: number): Promise<RepoChangesState> {
  const params = new URLSearchParams();
  if (limit != null) params.set('limit', String(limit));
  if (before != null) params.set('before', String(before));
  const qs = params.toString();
  return json(`${API}/changes/for-repo/${encodeURIComponent(repoId)}${qs ? `?${qs}` : ''}`);
}

export async function restartEngine(): Promise<void> {
  const res = await mutatingFetch(`${API}/restart`, { method: 'POST' });
  await throwIfNotOk(res);
}

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

// --- Notifications (SDK delegation) ---
export function getNotifications(params?: {
  limit?: number;
  before?: number;
  filter?: string;
}): Promise<NotificationsResponse> {
  return lucidos.notifications.list(params) as Promise<NotificationsResponse>;
}

export function getNotification(id: string): Promise<Notification | null> {
  return json(`${API}/notification?id=${encodeURIComponent(id)}`);
}

export function markNotificationRead(id: string): Promise<void> {
  return lucidos.notifications.markRead(id);
}

export function markAllNotificationsRead(): Promise<void> {
  return lucidos.notifications.markAllRead();
}

// --- Credentials ---
export function listCredentials(): Promise<CredentialsListResponse> {
  return json(`${API}/credentials`);
}

export function createCredential(body: {
  service_name: string;
  base_url: string;
  auth_type: AuthType;
  auth_value: string;
}): Promise<ApiResult> {
  return json(`${API}/credentials`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

export function updateCredential(
  service: string,
  authValue: string
): Promise<ApiResult> {
  return json(`${API}/credentials?service=${encodeURIComponent(service)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ auth_value: authValue }),
  });
}

export function deleteCredentialApi(
  service: string
): Promise<ApiResult> {
  return json(`${API}/credentials?service=${encodeURIComponent(service)}`, {
    method: 'DELETE',
  });
}

export function getCredentialValue(
  service: string
): Promise<{ auth_type: string; auth_value: string }> {
  return json(`${API}/credential-value?service=${encodeURIComponent(service)}`);
}

// --- OAuth Accounts ---
export function listOAuthAccounts(): Promise<{ accounts: OAuthAccountInfo[] }> {
  return json(`${API}/oauth/accounts`);
}

export function deleteOAuthAccountApi(id: string): Promise<ApiResult> {
  return json(`${API}/oauth/accounts?id=${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
}

export function reauthorizeOAuth(provider: string, scopes: string): Promise<ApiResult> {
  return json(`${API}/oauth/reauthorize`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ provider, scopes }),
  });
}

export function completeOAuth(provider: string): Promise<ApiResult> {
  return json(`${API}/oauth/complete?provider=${encodeURIComponent(provider)}`, {
    method: 'POST',
  }, 130000);
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
): string {
  const base = `${API}/app/${encodeURIComponent(appId)}/`;
  return commit ? `${base}?commit=${encodeURIComponent(commit)}` : base;
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
  const resp = await fetch(`${API}/app/versions?id=${encodeURIComponent(appId)}&limit=${limit}&skip=${skip}`);
  if (!resp.ok) throw new ApiError(resp.status, `Failed to fetch app versions`);
  return resp.json();
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
  if (!resp.ok) {
    throw new ApiError(resp.status, `App capture POST failed for request ${requestId}`);
  }
}

// --- MCP Consent ---
export async function postMcpConsent(requestId: string, allowed: boolean): Promise<void> {
  const resp = await mutatingFetch(`${API}/mcp/consent`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ request_id: requestId, allowed }),
  });
  if (!resp.ok) {
    throw new ApiError(resp.status, `MCP consent POST failed for request ${requestId}`);
  }
}

// --- Preferences (SDK delegation) ---
export async function getPreferences(deviceId?: string): Promise<{ preferences: Record<string, string> }> {
  const preferences = await lucidos.preferences.get(deviceId);
  return { preferences };
}

export async function setPreference(key: string, value: string, deviceId?: string): Promise<ApiResult> {
  await lucidos.preferences.set(key, value, deviceId);
  return { success: true };
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

// --- Triggers (SDK delegation) ---
export function listTriggers(): Promise<TriggersListResponse> {
  return lucidos.triggers.list().then(triggers => ({ triggers })) as Promise<TriggersListResponse>;
}

export function createTrigger(body: {
  name: string;
  run: TriggerRun;
  cron_expressions: string[];
  on_event?: string;
  condition?: Record<string, unknown>;
}): Promise<ApiResult> {
  return lucidos.triggers.create(body) as Promise<ApiResult>;
}

export function updateTrigger(
  id: string,
  body: {
    name?: string;
    run?: TriggerRun;
    cron_expressions?: string[];
    paused?: boolean;
    on_event?: string | null;
    condition?: Record<string, unknown> | null;
  }
): Promise<ApiResult> {
  return lucidos.triggers.update(id, body) as Promise<ApiResult>;
}

export function deleteTriggerApi(
  id: string
): Promise<ApiResult> {
  return lucidos.triggers.delete(id) as Promise<ApiResult>;
}

// --- Search Everywhere ---
export type SearchCategory = 'all' | 'threads' | 'files' | 'apps' | 'triggers' | 'settings' | 'changes';

export interface SearchResultItem {
  id: string;
  title: string;
  subtitle: string;
  category: string;
  score: number;
  last_activity?: string;
}

export interface SearchResults {
  results: Record<string, SearchResultItem[]>;
}

export async function searchEverywhere(query: string, category: SearchCategory, signal?: AbortSignal): Promise<SearchResults> {
  const params = new URLSearchParams({ category });
  if (query) params.set('q', query);
  const res = await fetch(`${API}/search?${params}`, { signal });
  if (!res.ok) throw new ApiError(res.status, 'Search failed');
  return res.json();
}

// --- Devices ---
export function registerDevice(deviceId: string, userAgent: string): Promise<ApiResult> {
  return json(`${API}/devices/register`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ device_id: deviceId, user_agent: userAgent }),
  });
}

export function listDevices(): Promise<{ devices: DeviceInfo[] }> {
  return json(`${API}/devices`);
}

export function renameDevice(deviceId: string, name: string | null): Promise<ApiResult> {
  return json(`${API}/devices/${deviceId}/name`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name }),
  });
}

export function setDevicePush(deviceId: string, pushEnabled: boolean): Promise<ApiResult> {
  return json(`${API}/devices/${deviceId}/push`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ push_enabled: pushEnabled }),
  });
}

export function deleteDevice(deviceId: string): Promise<ApiResult> {
  return json(`${API}/devices/${deviceId}`, { method: 'DELETE' });
}

// --- Memory Inspector ---
export function getMemoryStats(): Promise<MemoryStatsResponse> {
  return json(`${API}/memory/stats`);
}

export function getMemoryEntries(params?: {
  limit?: number;
  offset?: number;
  source_type?: string;
  sort?: string;
  importance?: string;
}): Promise<MemoryEntriesResponse> {
  const qs = new URLSearchParams();
  if (params?.limit != null) qs.set('limit', String(params.limit));
  if (params?.offset != null) qs.set('offset', String(params.offset));
  if (params?.source_type) qs.set('source_type', params.source_type);
  if (params?.sort) qs.set('sort', params.sort);
  if (params?.importance) qs.set('importance', params.importance);
  const q = qs.toString();
  return json(`${API}/memory/entries${q ? `?${q}` : ''}`);
}

export function getMemorySource(params: {
  source_type: string;
  source_id?: string;
  path?: string;
  commit?: string;
}): Promise<MemorySourceResponse> {
  const qs = new URLSearchParams();
  qs.set('source_type', params.source_type);
  if (params.source_id) qs.set('source_id', params.source_id);
  if (params.path) qs.set('path', params.path);
  if (params.commit) qs.set('commit', params.commit);
  return json(`${API}/memory/source?${qs}`);
}

export async function rebuildMemory(force = false): Promise<void> {
  const url = force ? `${API}/memory/rebuild?force=true` : `${API}/memory/rebuild`;
  await json(url, { method: 'POST' });
}

export async function cancelMemoryRebuild(): Promise<void> {
  await json(`${API}/memory/rebuild`, { method: 'DELETE' });
}

// --- Email ---
export function sendEmailConfirmed(draft: {
  to: string[];
  subject: string;
  body: string;
  cc?: string[];
  bcc?: string[];
  reply_to_message_id?: string;
  account: string;
  attachments?: string[];
}): Promise<ApiResult> {
  return json(`${API}/email/send`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(draft),
  });
}

// --- Backup ---
export interface BackupEntry {
  id: string;
  filename: string;
  size_bytes: number;
  created_at: string;
}

export interface BackupKeyResponse {
  key: string;
  is_new: boolean;
}

export interface BackupProviderInfo {
  id: string;
  name: string;
  connected: boolean;
  ready: boolean;
  required_scope: string;
}

export async function getBackupProviders(): Promise<BackupProviderInfo[]> {
  return json(`${API}/backup/providers`);
}

export async function getBackupKey(): Promise<BackupKeyResponse> {
  return json(`${API}/backup/key`);
}

export async function createBackup(provider: string): Promise<BackupEntry> {
  return json(`${API}/backup`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ provider }),
  }, 300000);
}

export async function listBackups(provider: string): Promise<BackupEntry[]> {
  return json(`${API}/backup/list?provider=${encodeURIComponent(provider)}`);
}

export interface RestoredWorkspace {
  workspace_path: string;
  workspace_name: string;
}

export async function restoreBackup(
  provider: string,
  backupId: string,
  key: string,
  workspaceName: string,
): Promise<RestoredWorkspace> {
  return json(`${API}/backup/restore`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ provider, backup_id: backupId, key, workspace_name: workspaceName }),
  }, 600000);
}

export interface ValidateNameResult {
  valid: boolean;
  reason?: string;
}

export async function validateWorkspaceName(name: string): Promise<ValidateNameResult> {
  return json(`${API}/backup/validate-workspace-name?name=${encodeURIComponent(name)}`);
}

export interface StartWorkspaceResult {
  url: string;
}

export async function startWorkspace(workspacePath: string): Promise<StartWorkspaceResult> {
  return json(`${API}/backup/start-workspace`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ workspace_path: workspacePath }),
  }, 120000);
}

export interface BackupSchedule {
  schedule: string | null;
  provider: string | null;
}

export async function getBackupSchedule(): Promise<BackupSchedule> {
  return json(`${API}/backup/schedule`);
}

export async function setBackupSchedule(provider: string, schedule: string): Promise<void> {
  await json(`${API}/backup/schedule`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ provider, schedule }),
  });
}

export interface BackupRetention {
  keep: number;
}

export async function getBackupRetention(): Promise<BackupRetention> {
  return json(`${API}/backup/retention`);
}

export async function setBackupRetention(keep: number): Promise<void> {
  await json(`${API}/backup/retention`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ keep }),
  });
}

// --- Repo File Explorer ---

export async function listRepoFiles(repoId: string, gitRef?: string): Promise<string[]> {
  const params = gitRef ? `?ref=${encodeURIComponent(gitRef)}` : '';
  return json(`${API}/repositories/${encodeURIComponent(repoId)}/files${params}`);
}

export async function getRepoFileContent(repoId: string, path: string, gitRef?: string): Promise<string> {
  const params = new URLSearchParams({ path });
  if (gitRef) params.set('ref', gitRef);
  const res = await fetch(`${API}/repositories/${encodeURIComponent(repoId)}/file?${params}`);
  if (!res.ok) throw new ApiError(res.status, await res.text());
  return res.text();
}

// --- Browse Directories ---
export interface BrowseResult {
  path: string;
  directories: string[];
  is_git_repo: boolean;
}

export async function browseDirectories(path?: string): Promise<BrowseResult> {
  const params = path ? `?path=${encodeURIComponent(path)}` : '';
  return json(`${API}/browse-directories${params}`);
}

// --- Saved Contexts ---
export async function saveContext(label: string, model: string | undefined, sections: Array<{ name: string; content: string; char_count: number }>): Promise<{ id: string }> {
  return json(`${API}/saved-contexts`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ label, model, sections }),
  });
}
