
import { workspaceRouteUrlSegment } from '../../utils/basePath';
import { deviceIdHeader } from '../../utils/deviceIdHeader';
import { replaceDocument } from '../../utils/documentNavigation';
import { landingHash, type WorkspaceLanding } from '../../utils/workspaceLanding';
import { AWAITING_OWNER, isAwaitingOwner, type AwaitingOwner } from './awaitingOwner';
import { gatewayErrorBody, gatewayErrorReason } from './gatewayError';
import { fetchWithDeadline, throwIfNotOk } from './_core';
import { discardResumableUpload, forgetResumableUpload, uploadResumable } from './resumableUpload';
import { UPLOAD_RESPONSE_MS, type UploadProgress } from './uploadProgress';
import type { AppUpdateProgress } from '../../utils/tauri';

const CONTROL = '/~/api/v1/control';

export type WorkspaceHealth = 'booting' | 'healthy' | 'unhealthy' | 'restoring';

export interface WorkspaceStatus {
  id: string;
  name: string;
  port: number;
  health: WorkspaceHealth;
  autostart: boolean;
  last_error?: string;
  unread_count?: number;
  last_successful_backup?: LastSuccessfulBackup;
  delete_interrupted?: boolean;
}

export interface LastSuccessfulBackup {
  at: string | null;
  stale: boolean;
  configured: boolean;
}

async function controlJson<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${CONTROL}${path}`, init);
  if (!res.ok) throw new Error(await gatewayErrorReason(res));
  return res.status === 204 || res.status === 202 ? (undefined as T) : res.json();
}

export interface WorkspaceListing {
  workspaces: WorkspaceStatus[];
  order: string[];
}

export async function listWorkspacesWithOrder(): Promise<WorkspaceListing> {
  const body = await controlJson<Partial<WorkspaceListing>>('/workspaces');
  return { workspaces: body.workspaces ?? [], order: body.order ?? [] };
}

export async function listWorkspaces(): Promise<WorkspaceStatus[]> {
  return (await listWorkspacesWithOrder()).workspaces;
}

export async function setWorkspaceOrder(order: readonly string[]): Promise<void> {
  await controlJson<void>('/workspace-order', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ order }),
  });
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

export async function restartWorkspace(id: string): Promise<void> {
  await controlJson<void>(`/workspaces/${encodeURIComponent(id)}/restart`, {
    method: 'POST',
    headers: deviceIdHeader(),
  });
}

export async function startWorkspace(id: string): Promise<void> {
  await controlJson<void>(`/workspaces/${encodeURIComponent(id)}/start`, { method: 'POST' });
}

export async function stopWorkspace(id: string): Promise<void> {
  await controlJson<void>(`/workspaces/${encodeURIComponent(id)}/stop`, {
    method: 'POST',
    headers: deviceIdHeader(),
  });
}

export async function setAutostart(id: string, enabled: boolean): Promise<void> {
  await controlJson<void>(`/workspaces/${encodeURIComponent(id)}/autostart`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ enabled }),
  });
}

export interface CleanupResult { id: string; status: string; detail: string | null }
export interface WorkspaceDeletionMember {
  id: string;
  name: string;
  database_backup: boolean;
}
export interface WorkspaceDeletionBatch {
  id: string;
  phrase: string;
  members: WorkspaceDeletionMember[];
}
export interface TrashEntry {
  id: string;
  name: string;
  status: string;
  restore_available: boolean;
  finish_available: boolean;
  database_backup: boolean;
  bytes: number | null;
  warning: string | null;
}
export interface EmptyTrashBatch {
  id: string;
  phrase: string;
  members: { id: string; bytes: number }[];
  results: CleanupResult[];
}

function cleanupPost<T>(path: string, body?: unknown): Promise<T> {
  return controlJson<T>(path, cleanupInit(body));
}

function cleanupInit(body?: unknown): RequestInit {
  return {
    method: 'POST',
    headers: { ...deviceIdHeader(), 'Content-Type': 'application/json' },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  };
}

async function cleanupJob<T>(path: string, body?: unknown): Promise<T> {
  for (;;) {
    const res = await fetch(`${CONTROL}${path}`, cleanupInit(body));
    if (res.status === 202) continue;
    if (!res.ok) throw new Error(await gatewayErrorReason(res));
    return res.status === 204 ? (undefined as T) : res.json();
  }
}

export function freezeWorkspaceDeletion(workspaces: string[]): Promise<WorkspaceDeletionBatch> {
  return cleanupPost('/workspace-delete-batches', { workspaces });
}
export function confirmCleanup(id: string, confirm: string): Promise<{ results: CleanupResult[] }> {
  return cleanupJob(`/cleanup/batches/${encodeURIComponent(id)}/confirm`, { confirm });
}
export function listTrash(): Promise<TrashEntry[]> {
  return controlJson<TrashEntry[]>('/trash');
}
export function restoreTrash(id: string): Promise<void> {
  return cleanupJob<void>(`/trash/${encodeURIComponent(id)}/restore`);
}
export function finishTrashDelete(id: string): Promise<void> {
  return cleanupJob<void>(`/trash/${encodeURIComponent(id)}/finish-delete`);
}
export function abandonTrashStaging(id: string): Promise<void> {
  return cleanupPost<void>(`/trash/${encodeURIComponent(id)}/abandon-staging`);
}
export function freezeEmptyTrash(): Promise<EmptyTrashBatch> {
  return cleanupPost<EmptyTrashBatch>('/trash/empty-batches');
}
export function confirmEmptyTrash(id: string, confirm: string): Promise<EmptyTrashBatch> {
  return cleanupJob<EmptyTrashBatch>(`/trash/empty-batches/${encodeURIComponent(id)}/confirm`, { confirm });
}

export function openWorkspace(id: string, landing?: WorkspaceLanding): void {
  replaceDocument(`/${workspaceRouteUrlSegment(id)}/${landingHash(landing)}`);
}


export interface GatewayStatus {
  build_id: string;
  update_available: boolean;
  packaged?: boolean;
  release_check?: ReleaseCheck;
  update_relay?: UpdateRelay;
}

export interface UpdateRelay {
  client: DesktopClient | null;
  request: UpdateRelayRequest | null;
}

export interface DesktopClient {
  version: string;
  blocker: string | null;
}

export interface UpdateRelayRequest {
  id: string;
  version: string;
  state: 'requested' | 'running' | 'ended';
  progress: AppUpdateProgress | null;
}

export interface UpdateRelayTicket {
  id: string;
  version: string;
}

export async function requestUpdateRelay(): Promise<UpdateRelayTicket> {
  return controlJson<UpdateRelayTicket>('/update-relay', { method: 'POST' });
}

export type InstallShape = 'desktop-app' | 'installer-rerun';

export interface ReleaseOffer {
  version: string;
  notes: string | null;
  install: InstallShape | null;
  command: string | null;
}

export interface ReleaseCheck {
  enabled: boolean;
  supported: boolean;
  current_version: string;
  checked_at: string | null;
  last_error: string | null;
  latest: ReleaseOffer | null;
  update_address: string | null;
  update_address_error: string | null;
  channel?: string;
  channels?: { channel: string; available: boolean; reason: string | null }[];
  custom?: { name: string; url: string }[];
  channel_version?: string | null;
  ahead_of_channel?: boolean;
  channel_error?: string | null;
  local_channel_dir?: string | null;
}

export async function getGatewayStatus(): Promise<GatewayStatus> {
  return controlJson<GatewayStatus>('/gateway/status');
}

export type UserKind = 'lucidos' | 'app' | 'process';

export interface MemoryUser {
  name: string;
  bytes: number;
  kind: UserKind;
}

export interface ProcessorUser {
  name: string;
  percent: number;
  kind: UserKind;
}

export type DatabaseProblem = 'not_answering' | 'pool_exhausted';

export type SlownessReason =
  | { reason: 'disk'; free_bytes: number }
  | { reason: 'database'; problem: DatabaseProblem; slow_workspaces: string[] }
  | { reason: 'memory'; top_users: MemoryUser[] }
  | { reason: 'unclear'; busiest_apps: ProcessorUser[]; slow_workspaces: string[] };

export type SlownessStatus =
  | { state: 'normal' }
  | ({ state: 'slow'; episode_id: string } & SlownessReason);

export async function getSlownessStatus(): Promise<SlownessStatus> {
  return controlJson<SlownessStatus>('/slowness');
}

export async function requestUpdateCheck(force = false): Promise<ReleaseCheck> {
  return controlJson<ReleaseCheck>('/gateway/check-updates', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ force }),
  });
}

export interface ReleaseCheckConfig {
  enabled?: boolean;
  channel?: string;
  custom_upsert?: { name: string; url: string };
  custom_remove?: string;
}

export async function setReleaseCheckConfig(body: ReleaseCheckConfig): Promise<ReleaseCheck> {
  return controlJson<ReleaseCheck>('/release-check', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

export async function reloadGateway(): Promise<void> {
  await controlJson<void>('/gateway/reload', { method: 'POST' });
}


export type InstallKind = 'desktop-app' | 'headless-installer' | 'source-checkout';

export interface InstallLaunchAgent {
  label: string;
  path: string;
  manager: 'launchd' | 'systemd';
}

export interface InstallRecord {
  kind: InstallKind;
  name: string;
  root: string | null;
  version: string | null;
  data_dir: string | null;
  port: number | null;
  agents: InstallLaunchAgent[];
  running_here: boolean;
  removal: string;
}

export interface InstallPortConflict {
  port: number;
  installs: string[];
}

export interface InstallInventory {
  installs: InstallRecord[];
  conflicts: InstallPortConflict[];
}

export async function fetchInstallInventory(): Promise<InstallInventory> {
  return controlJson<InstallInventory>('/installs');
}


export type WorkspaceLocation =
  | { status: 'reachable'; install: string; gateway_port: number; slug: string }
  | { status: 'install-not-running'; install: string; gateway_port: number; slug: string }
  | { status: 'ambiguous'; installs: string[] };

export async function locateWorkspace(name: string): Promise<WorkspaceLocation | null> {
  const res = await fetch(`${CONTROL}/workspace-location?name=${encodeURIComponent(name)}`);
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(await gatewayErrorReason(res));
  const body = (await res.json().catch(() => null)) as WorkspaceLocation | null;
  return body && typeof body.status === 'string' ? body : null;
}


export interface GatewayNetworkConfig {
  gateway_bind: string;
  inherit: boolean;
  detected_tailscale_ip: string | null;
}

export async function getGatewayNetworkConfig(): Promise<GatewayNetworkConfig> {
  return controlJson<GatewayNetworkConfig>('/network-config');
}

export async function setGatewayNetworkConfig(body: {
  gateway_bind: string;
  inherit: boolean;
}): Promise<void> {
  await controlJson<void>('/network-config', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}


export interface ExternalAddressSuggestion {
  interface: string;
  origin: string;
}

export type ExternalAddressProbe =
  | { verdict: 'verified' }
  | { verdict: 'unreachable'; detail: string }
  | { verdict: 'unknown'; detail: string };

export interface ExternalAddressInfo {
  external_address: string | null;
  source: 'unset' | 'file' | 'env';
  locked: boolean;
  env_var: string;
  suggestions: ExternalAddressSuggestion[];
  probe: ExternalAddressProbe | null;
  public_port: number | null;
}

export async function getExternalAddress(): Promise<ExternalAddressInfo> {
  return controlJson<ExternalAddressInfo>('/external-address');
}

export async function setExternalAddress(externalAddress: string): Promise<ExternalAddressInfo> {
  return controlJson<ExternalAddressInfo>('/external-address', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ external_address: externalAddress }),
  });
}

export interface GatewayTls {
  source: 'operator' | 'tailnet' | 'none';
  name: string | null;
  expires_at: string | null;
  error: string | null;
  operator_serves_other_names: boolean;
  https_origin: string | null;
}

export async function getGatewayTls(): Promise<GatewayTls> {
  return controlJson<GatewayTls>('/tls');
}


export type GwRestoreStatus =
  | { status: 'idle' }
  | { status: 'running'; id: string; name: string; phase: string }
  | { status: 'completed'; result_id?: string; id: string; name: string; warnings: string[] }
  | { status: 'failed'; result_id?: string; name: string; error: string };

export interface RestoreStarted {
  id: string;
  name: string;
}

export interface RestoreRequest {
  file: File;
  key: string;
  name: string;
}

export interface RestoreObserver {
  signal?: AbortSignal;
  onProgress?: (progress: UploadProgress) => void;
  onRetry?: (attempt: number, reason: unknown) => void;
  onUploaded?: () => void;
}

export async function restoreBackup(
  request: RestoreRequest,
  observer: RestoreObserver = {},
  controlBase = CONTROL,
): Promise<RestoreStarted> {
  const uploads = restoreUploadsBase(controlBase);
  const upload = await uploadResumable({
    base: uploads,
    file: request.file,
    signal: observer.signal,
    onProgress: observer.onProgress,
    onRetry: observer.onRetry,
  });
  observer.onUploaded?.();
  const res = await fetchWithDeadline(`${controlBase}/workspaces/restore`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ upload, key: request.key, name: request.name.trim() || undefined }),
    signal: observer.signal,
  }, UPLOAD_RESPONSE_MS);
  await throwIfNotOk(res);
  forgetResumableUpload(uploads, request.file);
  return res.json();
}

export function discardRestoreUpload(file: File, controlBase = CONTROL): Promise<void> {
  return discardResumableUpload(restoreUploadsBase(controlBase), file);
}

function restoreUploadsBase(controlBase: string): string {
  return `${controlBase}/restore-uploads`;
}

export function getRestoreStatus(controlBase = CONTROL): Promise<GwRestoreStatus> {
  return gatewayJson<GwRestoreStatus>(`${controlBase}/restore-status`);
}

export async function clearRestoreStatus(
  resultId: string | undefined,
  controlBase = CONTROL,
): Promise<void> {
  const query = resultId === undefined ? '' : `?${new URLSearchParams({ result_id: resultId })}`;
  await gatewayFetch(`${controlBase}/restore-status${query}`, { method: 'DELETE' });
}


const AUTH = '/~/api/v1/auth';
const REMOTE_GATEWAYS = '/~/api/v1/remote-gateways';

function authBase(gateway: string | null): string {
  return gateway === null ? AUTH : `${REMOTE_GATEWAYS}/${encodeURIComponent(gateway)}/manage`;
}

export interface PairedDevice {
  id: string;
  label: string;
  user_id?: string;
  paired_at: string;
  last_seen_at?: string;
}

export class GatewayError extends Error {
  constructor(message: string, readonly status: number, readonly reasonKind: string | null = null) {
    super(message);
    this.name = 'GatewayError';
  }

  get isAbsent(): boolean {
    return this.status === 404 && this.reasonKind === null;
  }
}

function authJson<T>(gateway: string | null, path: string, init?: RequestInit): Promise<T> {
  return gatewayJson<T>(`${authBase(gateway)}${path}`, init);
}

async function gatewayFetch(url: string, init?: RequestInit): Promise<Response> {
  let res: Response;
  try {
    res = await fetch(url, init);
  } catch (e) {
    throw new GatewayError(e instanceof Error ? e.message : 'could not reach the gateway', 0);
  }
  if (!res.ok) {
    const { reason, reasonKind } = await gatewayErrorBody(res);
    throw new GatewayError(reason, res.status, reasonKind);
  }
  return res;
}

async function gatewayJson<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await gatewayFetch(url, init);
  return res.status === 204 ? (undefined as T) : res.json();
}

// See "User rename wire aliases" in docs/temporary-measures.md.
export async function listPairedDevices(gateway: string | null = null): Promise<PairedDevice[]> {
  const devices = await authJson<(PairedDevice & { operator_id?: string })[]>(gateway, '/devices');
  return devices.map(({ operator_id, ...device }) => ({ ...device, user_id: device.user_id ?? operator_id }));
}

export async function revokePairedDevice(id: string, gateway: string | null = null): Promise<void> {
  await authJson<void>(gateway, `/devices/${encodeURIComponent(id)}`, { method: 'DELETE' });
}


export interface GatewayUser {
  id: string;
  name: string;
  emails: string[];
  device_ids: string[];
}

export interface RelayedUser {
  device_id: string;
  user_id: string;
  name: string;
  via: string;
  emails: string[];
}

export interface GatewayUsersResponse {
  users: GatewayUser[];
  relayed: RelayedUser[];
}

async function gatewayUsersCall(
  gateway: string | null,
  path: string,
  method: string,
  body?: unknown,
): Promise<GatewayUsersResponse> {
  const { requestGatewayUsers } = await import('./gatewayUsersCompat');
  return requestGatewayUsers(gateway, path, method, body, authJson);
}

const gatewayUserPath = (id: string) => `/users/${encodeURIComponent(id)}`;

export function listGatewayUsers(gateway: string | null = null): Promise<GatewayUsersResponse> {
  return gatewayUsersCall(gateway, '/users', 'GET');
}

export function addGatewayUser(gateway: string | null, name: string): Promise<GatewayUsersResponse> {
  return gatewayUsersCall(gateway, '/users', 'POST', { name });
}

export function renameGatewayUser(gateway: string | null, id: string, name: string): Promise<GatewayUsersResponse> {
  return gatewayUsersCall(gateway, gatewayUserPath(id), 'PUT', { name });
}

export function removeGatewayUser(gateway: string | null, id: string): Promise<GatewayUsersResponse> {
  return gatewayUsersCall(gateway, gatewayUserPath(id), 'DELETE');
}

export function addGatewayUserEmail(gateway: string | null, id: string, email: string): Promise<GatewayUsersResponse> {
  return gatewayUsersCall(gateway, `${gatewayUserPath(id)}/emails`, 'POST', { email });
}

export function removeGatewayUserEmail(
  gateway: string | null,
  id: string,
  email: string,
): Promise<GatewayUsersResponse> {
  return gatewayUsersCall(gateway, `${gatewayUserPath(id)}/emails/${encodeURIComponent(email)}`, 'DELETE');
}

export function moveDeviceToGatewayUser(
  gateway: string | null,
  deviceId: string,
  userId: string,
): Promise<GatewayUsersResponse> {
  return gatewayUsersCall(gateway, `/devices/${encodeURIComponent(deviceId)}/user`, 'PUT', {
    user_id: userId,
  });
}


export interface GatewayPairingCode {
  code: string;
  expires_in_secs: number;
}

export const REMOTE_START_SETTINGS = ['allow', 'ask', 'deny'] as const;
export type RemoteStartSetting = typeof REMOTE_START_SETTINGS[number];

export interface PairedGateway {
  id: string;
  label: string;
  paired_at: string;
  last_used_at?: string | null;
  may_manage: boolean;
  may_control: boolean;
  remote_starts: RemoteStartSetting;
}

export interface RemoteGateway {
  header_names: string[];
  name: string;
  address: string;
  pairing_id: string;
  gateway_protocol: number;
  added_at: string;
  last_rotated_at?: string | null;
  rotation_error?: string | null;
  return_access?: boolean;
  return_access_warning?: string;
  trusted_pairing?: boolean;
}

export interface RemovedRemoteGateway {
  remote_forgot: boolean;
  detail?: string;
}

export function mintGatewayPairingCode(): Promise<GatewayPairingCode> {
  return authJson<GatewayPairingCode>(null, '/gateway-pairing-code', { method: 'POST' });
}

export function listPairedGateways(): Promise<PairedGateway[]> {
  return authJson<PairedGateway[]>(null, '/gateways');
}

export async function unpairGateway(id: string): Promise<void> {
  await authJson<void>(null, `/gateways/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

export type HopGrant = 'manage' | 'control';

export function setPairedGatewayGrant(id: string, grant: HopGrant, on: boolean): Promise<PairedGateway> {
  return authJson<PairedGateway>(null, `/gateways/${encodeURIComponent(id)}/${grant}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ [`may_${grant}`]: on }),
  });
}

export function setPairedGatewayRemoteStarts(
  id: string,
  remoteStarts: RemoteStartSetting,
): Promise<PairedGateway> {
  return authJson<PairedGateway>(null, `/gateways/${encodeURIComponent(id)}/remote-starts`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ remote_starts: remoteStarts }),
  });
}

export function listRemoteGateways(): Promise<RemoteGateway[]> {
  return gatewayJson<RemoteGateway[]>(REMOTE_GATEWAYS);
}

export function setRemoteGatewayReturnAccess(name: string, enabled: boolean): Promise<RemoteGateway> {
  return gatewayJson<RemoteGateway>(`${REMOTE_GATEWAYS}/${encodeURIComponent(name)}/return-access`, {
    method: 'PUT', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ enabled }),
  });
}

export function setRemoteGatewayTrustedPairing(name: string, enabled: boolean): Promise<RemoteGateway> {
  return gatewayJson<RemoteGateway>(`${REMOTE_GATEWAYS}/${encodeURIComponent(name)}/trusted-pairing`, {
    method: 'PUT', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ enabled }),
  });
}

export function addRemoteGateway(request: {
  headers?: Record<string, string>;
  address: string;
  code: string;
  name: string;
}): Promise<RemoteGateway | AwaitingOwner> {
  return postPairing(REMOTE_GATEWAYS, request);
}

export function repairRemoteGateway(name: string, code: string): Promise<RemoteGateway | AwaitingOwner> {
  return postPairing(`${REMOTE_GATEWAYS}/${encodeURIComponent(name)}/repair`, { code });
}

async function postPairing(url: string, body: object): Promise<RemoteGateway | AwaitingOwner> {
  let res: Response;
  try {
    res = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
  } catch (e) {
    throw new GatewayError(e instanceof Error ? e.message : 'could not reach the gateway', 0);
  }
  if (isAwaitingOwner(res)) return AWAITING_OWNER;
  if (!res.ok) throw new GatewayError(await gatewayErrorReason(res), res.status);
  return res.json() as Promise<RemoteGateway>;
}

export interface PairingRequest {
  id: string;
  kind: 'device' | 'gateway';
  label: string;
  expires_in_secs: number;
}

export function listPairingRequests(): Promise<PairingRequest[]> {
  return authJson<PairingRequest[]>(null, '/pairing-requests');
}

export async function answerPairingRequest(id: string, answer: 'allow' | 'decline'): Promise<void> {
  await authJson<void>(null, `/pairing-requests/${encodeURIComponent(id)}/${answer}`, { method: 'POST' });
}

export function rotateRemoteGateway(name: string): Promise<void> {
  return gatewayJson<void>(`${REMOTE_GATEWAYS}/${encodeURIComponent(name)}/rotate`, { method: 'POST' });
}

export function updateRemoteGatewayHeaders(name: string, headers: Record<string, string>): Promise<RemoteGateway> {
  return gatewayJson<RemoteGateway>(`${REMOTE_GATEWAYS}/${encodeURIComponent(name)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ headers }),
  });
}

export type ListedWorkspace = Pick<WorkspaceStatus, 'id' | 'name' | 'health' | 'last_error' | 'unread_count'>;

export type RemoteGatewayListing = { gateway: string; host?: string; pairing_id?: string; last_seen_at?: string | null } & (
  | { state: 'reachable'; workspaces: ListedWorkspace[] }
  | { state: 'unreachable'; reason: string; reason_kind?: 'timeout' | 'refused' | 'tls' | 'revoked' | 'incompatible' }
);

export function listRemoteWorkspaces(): Promise<RemoteGatewayListing[]> {
  return gatewayJson<RemoteGatewayListing[]>('/~/api/v1/remote-workspaces');
}

export function remoteControlBase(gateway: string): string {
  return `${REMOTE_GATEWAYS}/${encodeURIComponent(gateway)}/control`;
}

export function listRemoteControlWorkspaces(gateway: string): Promise<ListedWorkspace[]> {
  return gatewayJson<ListedWorkspace[]>(`${remoteControlBase(gateway)}/workspaces`);
}

export async function createRemoteWorkspace(gateway: string, name: string): Promise<ListedWorkspace> {
  const body = await gatewayJson<{ workspace: ListedWorkspace }>(`${remoteControlBase(gateway)}/workspaces`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name }),
  });
  return body.workspace;
}

export async function startRemoteWorkspace(gateway: string, id: string): Promise<void> {
  await gatewayFetch(`${remoteControlBase(gateway)}/workspaces/${encodeURIComponent(id)}/start`, { method: 'POST' });
}

export async function stopRemoteWorkspace(gateway: string, id: string): Promise<void> {
  await gatewayFetch(`${remoteControlBase(gateway)}/workspaces/${encodeURIComponent(id)}/stop`, {
    method: 'POST',
    headers: deviceIdHeader(),
  });
}

export function removeRemoteGateway(name: string): Promise<RemovedRemoteGateway> {
  return gatewayJson<RemovedRemoteGateway>(`${REMOTE_GATEWAYS}/${encodeURIComponent(name)}`, {
    method: 'DELETE',
  });
}

export const ARCHIVE_SUFFIXES = ['.age', '.enc'] as const;

const ARCHIVE_NAME = new RegExp(
  `^lucidos-backup-(.+)-\\d{8}-\\d{6}(?:-o[0-9a-f]{12})?(?:${ARCHIVE_SUFFIXES.map((suffix) => `\\${suffix}`).join('|')})$`,
);

export function parseWorkspaceNameFromArchive(filename: string): string | null {
  const m = filename.match(ARCHIVE_NAME);
  return m ? m[1] : null;
}
