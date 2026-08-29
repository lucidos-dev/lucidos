import { API, ApiError, json, text } from './_core';
import { lucidos } from '@lucidos/sdk';
import type {
  AuthType,
  EmailAccountInfo,
  KnownOAuthProviders,
  Notification,
  OAuthAccountInfo,
} from '../../store/types';
import type { AgentBinariesResponse, ApiResult, CredentialsListResponse, DeviceInfo, EmbeddingModelStatus, EnvVarsListResponse, MemoryEntriesResponse, MemorySourceResponse, MemoryStatsResponse, NetworkConfigResponse, NotificationsResponse, TailnetStatusResponse } from '../types';

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
  /** The *credential scope*: every base URL the credential may be sent to. */
  base_urls: string[];
  auth_type: AuthType;
  auth_value: string;
  /** Override the default `CRED_<NAME>` env var name. */
  env_var_name?: string;
}): Promise<ApiResult> {
  return json(`${API}/credentials`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

// --- Environment variables (Settings → Environment Variables) ---
export function listEnvVars(): Promise<EnvVarsListResponse> {
  return json(`${API}/env-vars`);
}

export function createEnvVar(body: { name: string; value: string }): Promise<ApiResult> {
  return json(`${API}/env-vars`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

export function updateEnvVar(name: string, body: { value: string }): Promise<ApiResult> {
  return json(`${API}/env-vars?name=${encodeURIComponent(name)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

export function deleteEnvVarApi(name: string): Promise<ApiResult> {
  return json(`${API}/env-vars?name=${encodeURIComponent(name)}`, {
    method: 'DELETE',
  });
}

// --- Coding agent binaries (Settings → Coding Agents) ---
export function getCodingAgentBinaries(): Promise<AgentBinariesResponse> {
  return json(`${API}/coding-agents/binaries`);
}

export function deletePreference(key: string): Promise<ApiResult> {
  return json(`${API}/preferences?key=${encodeURIComponent(key)}`, { method: 'DELETE' });
}

// --- Network access (per-workspace engine bind; Settings → Access) ---
export function getNetworkConfig(): Promise<NetworkConfigResponse> {
  return json(`${API}/network-config`);
}

export function setNetworkConfig(body: { engine_bind: string }): Promise<ApiResult> {
  return json(`${API}/network-config`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

/** The MagicDNS name and this workspace's verified HTTPS URL (Settings →
 *  Access → Connect URLs). Separate from `getNetworkConfig` because it probes:
 *  a reverse lookup and a round trip through the tailnet, which the bind editor
 *  must not pay for. */
export function getTailnetStatus(): Promise<TailnetStatusResponse> {
  return json(`${API}/tailnet-status`);
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
  /** The *credential scope*: every base URL the credential may be sent to. */
  base_urls: string[];
  auth_type: AuthType;
  auth_header: string;
  /** Omitted / empty keeps the currently-stored secret. */
  auth_value?: string;
  /** Present only when editing an `email_password` credential. */
  email?: EmailAccountSettings;
  /** Override the default `CRED_<NAME>` env var name. */
  env_var_name?: string;
}

// The three verbs that act on an EXISTING credential take its `id`, not its
// service name. A name stopped being a unique handle when `auth_type` became
// the discriminator: an OAuth app registration may share a name with an API key
// for the same provider. Editing needs the id for a sharper reason still, since
// the form can change `auth_type`, so even name-plus-type cannot identify the
// row being edited. `createCredential` stays name-keyed because it is an upsert
// and the row has no id yet.

export function updateCredential(
  id: string,
  body: UpdateCredentialBody
): Promise<ApiResult> {
  return json(`${API}/credentials?id=${encodeURIComponent(id)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

export function deleteCredentialApi(
  id: string
): Promise<ApiResult> {
  return json(`${API}/credentials?id=${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
}

/** A credential's plaintext, for the Copy buttons and the edit form's prefill.
 *
 *  Two steps: mint a one-shot reveal token for this row, then spend it. The
 *  token lives 30 seconds and dies on use, so the plaintext is never one bare
 *  GET away. See ADR 0117 for what that does and does not close.
 *
 *  Retried once on a 403, because a spent token is a state only this function
 *  can recover from. The service worker re-issues a `GET` whose response was
 *  lost (`fetchWithRetry` in `public/sw.js`), and the server already redeemed
 *  the token on the attempt that vanished. So the retry that exists to rescue a
 *  flaky connection would otherwise turn one into a hard failure. Re-minting is
 *  exactly what a second click would do. */
export async function getCredentialValue(
  id: string
): Promise<{ auth_type: string; auth_value: string }> {
  try {
    return await revealCredentialOnce(id);
  } catch (err) {
    if (err instanceof ApiError && err.httpCode === 403) return revealCredentialOnce(id);
    throw err;
  }
}

/** One mint-then-spend round trip. */
async function revealCredentialOnce(
  id: string
): Promise<{ auth_type: string; auth_value: string }> {
  const { token } = await json<{ token: string; expires_in_secs: number }>(
    `${API}/credential-reveal-token?id=${encodeURIComponent(id)}`,
    { method: 'POST' }
  );
  return json(
    `${API}/credential-value?id=${encodeURIComponent(id)}&token=${encodeURIComponent(token)}`
  );
}

export function getEmailAccount(name: string): Promise<EmailAccountInfo> {
  return json(`${API}/email-account?name=${encodeURIComponent(name)}`);
}

// --- OAuth Accounts ---
export function listOAuthAccounts(): Promise<{ accounts: OAuthAccountInfo[] }> {
  return json(`${API}/oauth/accounts`);
}

/** The *OAuth provider registry* plus the redirect URI a flow will send.
 *
 *  Non-secret provider metadata, so it needs no gating. An engine with no
 *  staged system-knowhow answers an empty `providers` list rather than failing,
 *  and the Accounts page falls back to its typed-name path. */
export function listKnownOAuthProviders(): Promise<KnownOAuthProviders> {
  return json(`${API}/oauth/known-providers`);
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

/** What the engine did with a hand-over claim. Every value is an ordinary
 *  answer: `already-done` and `no-such-device` both mean "stop asking". */
export type HandOverOutcome = 'moved' | 'already-done' | 'no-such-device';

/**
 * Move this device's row from the id it used to report to the one it reports
 * now, keeping its push subscription and its device-scoped preferences.
 *
 * Called once, early in boot, and only when the two differ. See
 * `docs/plans/2026-08-22-one-device-identity-minted-at-the-gateway.md`.
 *
 * **It asserts the id being adopted, overriding the default stamp.** This is
 * the one request whose subject is a change of identity, and `localStorage`
 * still holds the OLD id here by design: nothing is written until the row has
 * moved. `fetchWithDefaults` would therefore claim the identity being left.
 * The engine refuses that (`foreign_hand_over` in `api/settings.rs`) whenever
 * the gateway is not the one asserting the header. Behind an up-to-date
 * gateway this value is replaced by the authenticated id, which is the same
 * id. `docs/plans/2026-08-22-device-hand-over-must-not-need-a-fresh-gateway.md`
 */
export function handOverDevice(
  oldDeviceId: string,
  deviceId: string,
): Promise<ApiResult & { outcome?: HandOverOutcome }> {
  return json(`${API}/devices/hand-over`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'x-lucidos-device-id': deviceId },
    body: JSON.stringify({ old_device_id: oldDeviceId, device_id: deviceId }),
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

/** Snapshot of the background embedding-model load. The live signal is the
 *  `EmbeddingModelStatusChanged` SSE frame; this is how a client that connected
 *  mid-download catches up, and it returns the identical shape. */
export function getEmbeddingModelStatus(): Promise<EmbeddingModelStatus> {
  return json(`${API}/memory/embedding-model-status`);
}

// --- Email ---

/** The engine's worst-case send path is the 30s-bounded OAuth token refresh
 *  (`bounded_http_client` in `core/oauth.rs`) followed by the 80s-bounded SMTP
 *  send (`SMTP_SEND_TIMEOUT` in `core/email_client.rs`) = 110s. This request
 *  timeout must stay ABOVE that sum so the engine's specific failure ("SMTP
 *  send via host:port timed out", auth errors, no-route) reaches the toast:
 *  `json()`'s 10s default aborted first, surfaced only a misleading generic
 *  "request timed out" while the backend kept sending, and users retrying the
 *  still-open modal double-sent.
 *  Exported for the contract pin in email-send-pending.test.ts. */
export const EMAIL_SEND_TIMEOUT_MS = 120_000;

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
  }, EMAIL_SEND_TIMEOUT_MS);
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
  /** Which required scopes a CONNECTED account is missing, in the provider's
   *  declared order. Empty for a ready provider and for one with no account.
   *  Named on the page: "access not granted" alone cannot tell a grant that
   *  never happened from one that came back a scope short. */
  missing_scopes: string[];
  /** Web URL to this provider's backups folder ("View backups folder" link), or null. */
  folder_url: string | null;
}

export async function getBackupProviders(): Promise<BackupProviderInfo[]> {
  return json(`${API}/backup/providers`);
}

/** Mint a one-shot token for the two key routes below. Step one of two.
 *
 *  The backup key takes the same shape as a credential reveal (ADR 0117): the
 *  token lives 30 seconds, dies on use, and is bound to this one secret. It
 *  takes no id, because a workspace has exactly one backup key. */
async function mintBackupKeyRevealToken(): Promise<string> {
  const { token } = await json<{ token: string; expires_in_secs: number }>(
    `${API}/backup/key/reveal-token`,
    { method: 'POST' }
  );
  return token;
}

/** Run one mint-then-spend round trip, retrying the pair once on a 403.
 *
 *  A spent token is a state only this function can recover from. The service
 *  worker re-issues a request whose response was lost (`fetchWithRetry` in
 *  `public/sw.js`), and the server already redeemed the token on the attempt
 *  that vanished. So the retry that exists to rescue a flaky connection would
 *  otherwise turn one into a hard failure. Re-minting is exactly what a second
 *  click would do, and one retry means a real refusal still surfaces. */
async function withBackupKeyToken(
  spend: (token: string) => Promise<BackupKeyResponse>
): Promise<BackupKeyResponse> {
  const once = async () => spend(await mintBackupKeyRevealToken());
  try {
    return await once();
  } catch (err) {
    if (err instanceof ApiError && err.httpCode === 403) return once();
    throw err;
  }
}

/** Reveal the EXISTING backup key. Read-only — throws `ApiError` 404 when no key
 *  has been generated yet (call `generateBackupKey` for that). Never mints a key. */
export async function getBackupKey(): Promise<BackupKeyResponse> {
  return withBackupKeyToken((token) =>
    json(`${API}/backup/key?token=${encodeURIComponent(token)}`));
}

/** Generate the backup key if none exists yet, then return it. Idempotent on the
 *  engine: if a key already exists it's returned unchanged (`is_new: false`), so
 *  this can never overwrite the key that protects existing backups.
 *
 *  That idempotence is why it spends a token too. On a workspace that already
 *  has a key, this route hands back the same plaintext the reveal does. */
export async function generateBackupKey(): Promise<BackupKeyResponse> {
  return withBackupKeyToken((token) =>
    json(`${API}/backup/key?token=${encodeURIComponent(token)}`, { method: 'POST' }));
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

/** Persisted outcome of the last backup run — survives engine restarts. */
export interface BackupLastRun {
  status: 'success' | 'failure';
  at: string;
  /** When the run started (RFC 3339); null for records predating duration
   *  tracking. Duration = `at - started_at`. */
  started_at: string | null;
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

// Restore lives in the workspace picker (gateway control plane), not here — see
// `api/client/control.ts` (`restoreBackup` / `getRestoreStatus`). Settings keeps
// only backup *creation* + scheduling.

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

/** URL of a repo file's raw bytes at `gitRef` (default HEAD). The engine serves
 *  it with a content-type inferred from the extension, so this is safe to point
 *  an <img>/<video>/<audio>/<iframe> `src` at for binary-media previews. */
export function repoFileUrl(repoId: string, path: string, gitRef?: string): string {
  const params = new URLSearchParams({ path });
  if (gitRef) params.set('ref', gitRef);
  return `${API}/repositories/${encodeURIComponent(repoId)}/file?${params}`;
}

export async function getRepoFileContent(repoId: string, path: string, gitRef?: string): Promise<string> {
  return text(repoFileUrl(repoId, path, gitRef));
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
