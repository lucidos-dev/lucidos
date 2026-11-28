import { API, ApiError, json, text, throwIfRefused } from './_core';
import { repoFileUrl } from './fileUrls';
import type { FormRequestEvent } from '../../store/thread-events/thread-event-types';
import { lucidos } from '@lucidos/sdk';
import { HEADER_DEVICE_ID } from '@lucidos/engine-constants';
import type {
  AuthType,
  EmailAccountInfo,
  KnownOAuthProviders,
  Notification,
  OAuthAccountInfo,
} from '../../store/types';
import type { AgentBinariesResponse, ApiResult, CodingAgent, CodingAgentLadder, IntelligenceLadder, IntelligenceRung, LadderSurface, BackgroundModels, CredentialsListResponse, DeviceInfo, EmbeddingModelStatus, EnvVarsListResponse, RecallDateResponse, RecallZoomResponse, SummaryTreeThreadsResponse, SummaryTreeTop, TreeBackfill, TreeBackfillEstimate, MemoryEntriesResponse, MemorySourceResponse, MemoryStatsResponse, NotificationsResponse } from '../types';

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

export function listCredentials(): Promise<CredentialsListResponse> {
  return json(`${API}/credentials`);
}

export function createCredential(body: {
  service_name: string;
  base_urls: string[];
  auth_type: AuthType;
  auth_value: string;
  env_var_name?: string;
  form_request_id?: string;
}): Promise<ApiResult> {
  return json(`${API}/credentials`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}


export interface PendingFormRequest {
  thread_id: string;
  request_id: string;
  event: FormRequestEvent;
}

export function listPendingFormRequests(): Promise<PendingFormRequest[]> {
  return json(`${API}/form-requests/pending`);
}

export async function listResolvedFormRequests(ids: string[]): Promise<string[]> {
  const resolved: string[] = [];
  for (let offset = 0; offset < ids.length; offset += 100) {
    const query = new URLSearchParams({ request_ids: ids.slice(offset, offset + 100).join(',') });
    resolved.push(...await json<string[]>(`${API}/form-requests/resolved?${query}`));
  }
  return resolved;
}

export function cancelFormRequest(requestId: string): Promise<{ resolved: boolean }> {
  return json(`${API}/form-requests/${encodeURIComponent(requestId)}/cancel`, {
    method: 'POST',
  });
}

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

export function getCodingAgentBinaries(): Promise<AgentBinariesResponse> {
  return json(`${API}/coding-agents/binaries`);
}

export function getCodingAgentLadder(agent: CodingAgent): Promise<CodingAgentLadder> {
  return json(`${API}/intelligence-ladder?surface=${agent}`);
}

export function getChatLadder(): Promise<IntelligenceLadder> {
  return json(`${API}/intelligence-ladder?surface=chat`);
}

export function copyLadderFromChat(surface: CodingAgent): Promise<{ rungs: IntelligenceRung[] }> {
  return json(`${API}/intelligence-ladder/copy-from-chat`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ surface } satisfies { surface: LadderSurface }),
  });
}

export function sortLadder(surface: LadderSurface): Promise<{ rungs: IntelligenceRung[] }> {
  return json(`${API}/intelligence-ladder/sort`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ surface } satisfies { surface: LadderSurface }),
  });
}

export function setLadderDefault(surface: LadderSurface, { model, effort }: IntelligenceRung): Promise<ApiResult> {
  return json(`${API}/intelligence-ladder/default`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ surface, model, effort }),
  });
}

export function getModelPrices() {
  return lucidos.priceTable.get();
}

export function deletePreference(key: string): Promise<ApiResult> {
  return json(`${API}/preferences?key=${encodeURIComponent(key)}`, { method: 'DELETE' });
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
  base_urls: string[];
  auth_type: AuthType;
  auth_header: string;
  auth_value?: string;
  email?: EmailAccountSettings;
  env_var_name?: string;
  form_request_id?: string;
}


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

const CREDENTIAL_FORM_READ_TIMEOUT_MS = 30_000;

async function revealCredentialOnce(
  id: string
): Promise<{ auth_type: string; auth_value: string }> {
  const { token } = await json<{ token: string; expires_in_secs: number }>(
    `${API}/credential-reveal-token?id=${encodeURIComponent(id)}`,
    { method: 'POST' },
    CREDENTIAL_FORM_READ_TIMEOUT_MS,
  );
  return json(
    `${API}/credential-value?id=${encodeURIComponent(id)}&token=${encodeURIComponent(token)}`,
    undefined,
    CREDENTIAL_FORM_READ_TIMEOUT_MS,
  );
}

export function getEmailAccount(name: string): Promise<EmailAccountInfo> {
  return json(
    `${API}/email-account?name=${encodeURIComponent(name)}`,
    undefined,
    CREDENTIAL_FORM_READ_TIMEOUT_MS,
  );
}

export function listOAuthAccounts(): Promise<{ accounts: OAuthAccountInfo[] }> {
  return json(`${API}/oauth/accounts`);
}

export function listKnownOAuthProviders(): Promise<KnownOAuthProviders> {
  return json(`${API}/oauth/known-providers`);
}

export function deleteOAuthAccountApi(id: string): Promise<ApiResult> {
  return json(`${API}/oauth/accounts?id=${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
}

export function reauthorizeOAuth(
  provider: string,
  scopes: string,
  signIn = false,
): Promise<ApiResult> {
  return json(`${API}/oauth/reauthorize`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ provider, scopes, sign_in: signIn }),
  });
}

export function completeOAuth(provider: string): Promise<ApiResult> {
  return json(`${API}/oauth/complete?provider=${encodeURIComponent(provider)}`, {
    method: 'POST',
  }, 130000);
}

export async function getPreferences(deviceId?: string): Promise<{ preferences: Record<string, string> }> {
  const preferences = await lucidos.preferences.get(deviceId);
  return { preferences };
}

export async function setPreference(
  key: string,
  value: string,
  deviceId?: string,
): Promise<ApiResult> {
  const result: ApiResult = await json(`${API}/preferences?key=${encodeURIComponent(key)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ value, device_id: deviceId }),
  });
  return throwIfRefused(result, `the engine refused the '${key}' preference`);
}

export function registerDevice(deviceId: string, userAgent: string): Promise<ApiResult> {
  return json(`${API}/devices/register`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ device_id: deviceId, user_agent: userAgent }),
  });
}

export type HandOverOutcome = 'moved' | 'already-done' | 'no-such-device';

export function handOverDevice(
  oldDeviceId: string,
  deviceId: string,
): Promise<ApiResult & { outcome?: HandOverOutcome }> {
  return json(`${API}/devices/hand-over`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', [HEADER_DEVICE_ID]: deviceId },
    body: JSON.stringify({ old_device_id: oldDeviceId, device_id: deviceId }),
  });
}

export function getCallerDevice(): Promise<{ device_id: string | null }> {
  return json(`${API}/devices/caller`);
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

export function getEmbeddingModelStatus(): Promise<EmbeddingModelStatus> {
  return json(`${API}/memory/embedding-model-status`);
}

export function getTreeBackfill(): Promise<TreeBackfill> {
  return json(`${API}/memory/tree-backfill`);
}

export function raiseTreeBackfillCap(fromCap: number): Promise<{ cap_usd: number }> {
  return json(`${API}/memory/tree-backfill/cap`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ from_cap_usd: fromCap }),
  });
}

const TREE_BACKFILL_ESTIMATE_TIMEOUT_MS = 60_000;

export function getTreeBackfillEstimate(): Promise<TreeBackfillEstimate> {
  return json(`${API}/memory/tree-backfill/estimate`, undefined, TREE_BACKFILL_ESTIMATE_TIMEOUT_MS);
}

export function getSummaryTree(threadId?: string): Promise<SummaryTreeTop> {
  const q = threadId ? `?thread=${encodeURIComponent(threadId)}` : '';
  return json(`${API}/memory/tree${q}`);
}

export function getSummaryTreeThreads(params: { limit: number; offset: number }): Promise<SummaryTreeThreadsResponse> {
  return json(`${API}/memory/tree/threads?limit=${params.limit}&offset=${params.offset}`);
}

export function getRecallZoom(id: string): Promise<RecallZoomResponse> {
  return json(`${API}/recall/zoom?id=${encodeURIComponent(id)}&n=1`);
}

export function getRecallDate(id: string): Promise<RecallDateResponse> {
  return json(`${API}/recall/date?id=${encodeURIComponent(id)}`);
}

export function getBackgroundModels(): Promise<BackgroundModels> {
  return json(`${API}/models/background`);
}


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
  form_request_id?: string;
}): Promise<ApiResult> {
  return json(`${API}/email/send`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(draft),
  }, EMAIL_SEND_TIMEOUT_MS);
}

export interface BackupEntry {
  id: string;
  filename: string;
  size_bytes: number;
  created_at: string;
}

export interface BackupKeyResponse {
  key: string;
}

export interface BackupKeys {
  recipient: string | null;
  pending: boolean;
  legacy_key: boolean;
  secs_since_verified: number | null;
}

export interface BackupProviderInfo {
  id: string;
  name: string;
  connected: boolean;
  ready: boolean;
  missing_scopes: string[];
  folder_url: string | null;
}

export async function getBackupProviders(): Promise<BackupProviderInfo[]> {
  return json(`${API}/backup/providers`);
}

async function mintBackupKeyRevealToken(): Promise<string> {
  const { token } = await json<{ token: string; expires_in_secs: number }>(
    `${API}/backup/key/reveal-token`,
    { method: 'POST' }
  );
  return token;
}

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

export async function getBackupKey(): Promise<BackupKeyResponse> {
  return withBackupKeyToken((token) =>
    json(`${API}/backup/key?token=${encodeURIComponent(token)}`));
}

export async function getBackupKeys(): Promise<BackupKeys> {
  return json(`${API}/backup/recipient`);
}

export async function startBackupKeySetup(): Promise<string> {
  const { backup_key } = await json<{ backup_key: string }>(
    `${API}/backup/recipient/setup`, { method: 'POST' });
  return backup_key;
}

export async function confirmBackupKey(backupKey: string): Promise<BackupKeys> {
  return json(`${API}/backup/recipient/confirm`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ backup_key: backupKey }),
  });
}

export async function setOwnBackupRecipient(recipient: string): Promise<BackupKeys> {
  return json(`${API}/backup/recipient`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ recipient }),
  });
}

export async function verifyBackupKey(backupKey: string): Promise<boolean> {
  const { matches } = await json<{ matches: boolean }>(`${API}/backup/recipient/verify`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ backup_key: backupKey }),
  });
  return matches;
}

export interface OnepasswordStatus {
  saved: boolean;
  cli_path: string | null;
}

export interface OnepasswordVaultCheck {
  cli: string;
  vaults: string[];
}

export async function getOnepasswordStatus(): Promise<OnepasswordStatus> {
  return json(`${API}/onepassword`);
}

export async function saveOnepassword(token: string, cliPath: string): Promise<void> {
  await text(`${API}/onepassword`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ token, cli_path: cliPath }),
  });
}

export async function deleteOnepassword(): Promise<void> {
  await text(`${API}/onepassword`, { method: 'DELETE' });
}

const ONEPASSWORD_TEST_TIMEOUT_MS = 30_000;

export async function testOnepassword(): Promise<OnepasswordVaultCheck> {
  return json(`${API}/onepassword/test`, { method: 'POST' }, ONEPASSWORD_TEST_TIMEOUT_MS);
}

export async function createBackup(provider: string): Promise<void> {
  await json(`${API}/backup`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ provider }),
  });
}

export interface BackupLastRun {
  status: 'success' | 'failure';
  at: string;
  started_at: string | null;
  filename: string | null;
  size_bytes: number | null;
  error: string | null;
}

export interface BackupStatus {
  running: boolean;
  last_run: BackupLastRun | null;
  latest_backup: BackupEntry | null;
  age_seconds: number | null;
  stale: boolean;
  list_error: string | null;
}

export async function getBackupStatus(provider: string): Promise<BackupStatus> {
  return json(`${API}/backup/status?provider=${encodeURIComponent(provider)}`);
}

export interface LastSuccessfulBackupResponse {
  at: string | null;
  stale: boolean;
  configured: boolean;
  overdue: boolean;
  overdue_at: string | null;
}

export async function getLastSuccessfulBackup(): Promise<LastSuccessfulBackupResponse> {
  return json(`${API}/backup/last-successful`);
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


export async function listRepoFiles(repoId: string, gitRef?: string): Promise<string[]> {
  const params = gitRef ? `?ref=${encodeURIComponent(gitRef)}` : '';
  return json(`${API}/repositories/${encodeURIComponent(repoId)}/files${params}`);
}

export async function getRepoFileContent(repoId: string, path: string, gitRef?: string): Promise<string> {
  return text(repoFileUrl(repoId, path, gitRef));
}

export interface BrowseResult {
  path: string;
  directories: string[];
  is_git_repo: boolean;
}

export async function browseDirectories(path?: string): Promise<BrowseResult> {
  const params = path ? `?path=${encodeURIComponent(path)}` : '';
  return json(`${API}/browse-directories${params}`);
}


export type FootprintVerdict = 'used' | 'unused' | 'not-yet-judged' | 'not-loaded-by-name';

export interface FootprintItem {
  kind: 'app' | 'reusable-widget' | 'knowhow' | 'intent' | 'response-style' | 'user-profile'
    | 'email-account' | 'oauth-account' | 'credential' | 'mcp-server' | 'app-knowhow';
  id: string;
  name: string;
  chars: number;
  clipped_chars: number;
  usage?: { last_used_days_ago: number | null; verdict: FootprintVerdict };
  path?: string;
  mcp_threads?: { enabled: number; called: number };
}

export interface FootprintSection {
  id: string;
  title: string;
  when: 'every-turn' | 'gated' | 'open-app' | 'tools-dropped' | 'mcp-enabled';
  chars: number;
  over_ceiling: boolean;
  items: FootprintItem[];
}

export interface WorkspacePromptFootprint {
  sections: FootprintSection[];
  total_chars: number;
  over_total_ceiling: boolean;
  section_ceiling: number;
  total_ceiling: number;
  unused_days: number;
  system_prompt_chars: number;
  system_prompt_areas: { label: string; chars: number }[];
}

export function fetchWorkspacePromptFootprint(): Promise<WorkspacePromptFootprint> {
  return json(`${API}/workspace-prompt-footprint`);
}
