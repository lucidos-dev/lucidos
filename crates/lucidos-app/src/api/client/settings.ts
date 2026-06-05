import { API, json, text } from './_core';
import { lucidos } from '@lucidos/sdk';
import type { AuthType, EmailAccountInfo, Notification, OAuthAccountInfo } from '../../store/types';
import type { ApiResult, CredentialsListResponse, DeviceInfo, MemoryEntriesResponse, MemorySourceResponse, MemoryStatsResponse, NotificationsResponse } from '../types';

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

export interface EmailAccountSettings {
  email_address: string;
  imap_host: string;
  imap_port: number;
  smtp_host: string;
  smtp_port: number;
  username: string;
  use_tls: boolean;
  require_send_confirmation: boolean;
}

export interface UpdateCredentialBody {
  base_url: string;
  auth_type: AuthType;
  auth_header: string;
  /** Omitted / empty keeps the currently-stored secret. */
  auth_value?: string;
  /** Present only when editing an `email_password` credential. */
  email?: EmailAccountSettings;
}

export function updateCredential(
  service: string,
  body: UpdateCredentialBody
): Promise<ApiResult> {
  return json(`${API}/credentials?service=${encodeURIComponent(service)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
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

export function getEmailAccount(name: string): Promise<EmailAccountInfo> {
  return json(`${API}/email-account?name=${encodeURIComponent(name)}`);
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

// --- Preferences (SDK delegation) ---
export async function getPreferences(deviceId?: string): Promise<{ preferences: Record<string, string> }> {
  const preferences = await lucidos.preferences.get(deviceId);
  return { preferences };
}

export async function setPreference(key: string, value: string, deviceId?: string): Promise<ApiResult> {
  await lucidos.preferences.set(key, value, deviceId);
  return { success: true };
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

/** Reveal the EXISTING backup key. Read-only — throws `ApiError` 404 when no key
 *  has been generated yet (call `generateBackupKey` for that). Never mints a key. */
export async function getBackupKey(): Promise<BackupKeyResponse> {
  return json(`${API}/backup/key`);
}

/** Generate the backup key if none exists yet, then return it. Idempotent on the
 *  engine: if a key already exists it's returned unchanged (`is_new: false`), so
 *  this can never overwrite the key that protects existing backups. */
export async function generateBackupKey(): Promise<BackupKeyResponse> {
  return json(`${API}/backup/key`, { method: 'POST' });
}

export interface BackupKeyExists {
  exists: boolean;
}

/** Whether a backup key already exists, without revealing it. Used on page load
 *  to label the key button ("Show backup key" vs "Generate new backup key"). */
export async function getBackupKeyExists(): Promise<BackupKeyExists> {
  return json(`${API}/backup/key/exists`);
}

/** Returns when the backup is queued — completion arrives via SSE
 *  (`BackupCompleted` / `BackupFailed`), not by awaiting this promise. */
export async function createBackup(provider: string): Promise<void> {
  await json(`${API}/backup`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ provider }),
  });
}

export async function listBackups(provider: string): Promise<BackupEntry[]> {
  return json(`${API}/backup/list?provider=${encodeURIComponent(provider)}`);
}

/** Persisted outcome of the last backup run — survives engine restarts. */
export interface BackupLastRun {
  status: 'success' | 'failure';
  at: string;
  filename: string | null;
  size_bytes: number | null;
  error: string | null;
}

/** Aggregated backup health for the Settings → Backup page. */
export interface BackupStatus {
  /** A backup is in progress right now. */
  running: boolean;
  /** Outcome of the last run, or null if never recorded. */
  last_run: BackupLastRun | null;
  /** Newest cloud backup, or null if none / provider unreachable. */
  latest_backup: BackupEntry | null;
  /** Age of `latest_backup` in seconds, or null if none. */
  age_seconds: number | null;
  /** No recent good backup (none, or older than 24h). */
  stale: boolean;
  /** Set when the provider couldn't be listed (cloud unreachable). */
  list_error: string | null;
}

export async function getBackupStatus(provider: string): Promise<BackupStatus> {
  return json(`${API}/backup/status?provider=${encodeURIComponent(provider)}`);
}

export interface RestoredWorkspace {
  workspace_path: string;
  workspace_name: string;
}

/** Authoritative restore state — mirrors the Rust `RestoreState` enum. The SAME
 *  shape arrives via the `Restore*` SSE events and from `getRestoreStatus()`, so
 *  a live stream and a page-reload refetch render identically. */
export type RestoreState =
  | { status: 'idle' }
  | { status: 'running'; workspace_name: string; phase: string; progress: number; total: number }
  | { status: 'completed'; workspace_name: string; workspace_path: string }
  | { status: 'failed'; workspace_name: string; error: string };

/** Kick off a restore. Returns 202 immediately — the restore runs detached on
 *  the engine and reports through `RestoreProgress`/`RestoreCompleted`/
 *  `RestoreFailed` SSE plus the refetchable `getRestoreStatus()`. Do NOT await
 *  this for the result; watch `restoreState` instead. */
export async function restoreBackup(
  provider: string,
  backupId: string,
  key: string,
  workspaceName: string,
): Promise<{ status: string }> {
  return json(`${API}/backup/restore`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ provider, backup_id: backupId, key, workspace_name: workspaceName }),
  });
}

/** The engine's current restore state, for reload re-attach. */
export async function getRestoreStatus(): Promise<RestoreState> {
  return json(`${API}/backup/restore-status`);
}

/** Drop a terminal (completed/failed) restore result so the banner clears and a
 *  reload agrees. Refused by the engine while a restore is still running. */
export async function clearRestoreStatus(): Promise<void> {
  await json(`${API}/backup/restore-status`, { method: 'DELETE' });
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
  /** True only once the started workspace answered /health. The caller opens the
   *  tab only when true, so it never opens a blank page against a booting engine. */
  ready: boolean;
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
  return text(`${API}/repositories/${encodeURIComponent(repoId)}/file?${params}`);
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
