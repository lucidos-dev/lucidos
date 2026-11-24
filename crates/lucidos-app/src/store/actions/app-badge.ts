
import { computed, signal } from '@preact/signals';
import { unreadCount } from '../store';
import { IS_PICKER, WORKSPACE_ID } from '../../utils/basePath';
import { isTauri } from '../../utils/platform';
import { listRemoteWorkspaces, listWorkspaces, type WorkspaceStatus } from '../../api/client/control';
import { notifyRows } from '../../utils/notificationRows';
import { mergeRemoteListings, type RemoteGatewayView } from '../../utils/remoteWorkspaces';

type BadgingNavigator = Navigator & {
  setAppBadge?: (count?: number) => Promise<void>;
  clearAppBadge?: () => Promise<void>;
};

export function applyAppBadge(count: number): void {
  const nav = navigator as BadgingNavigator;
  if (typeof nav.setAppBadge !== 'function') return;
  if (count > 0) {
    nav.setAppBadge(count).catch(() => {});
  } else {
    nav.clearAppBadge?.().catch(() => {});
  }
}

export const peerWorkspaces = signal<WorkspaceStatus[]>([]);

export const remotePeerWorkspaces = signal<RemoteGatewayView[]>([]);

let refreshSeq = 0;

let remoteRefreshSeq = 0;

function badgesEveryWorkspace(): boolean {
  return WORKSPACE_ID !== null;
}

const otherWorkspacesUnread = computed(() =>
  peerWorkspaces.value
    .filter((w) => w.id !== WORKSPACE_ID)
    .reduce((sum, w) => sum + (w.unread_count ?? 0), 0),
);

export const crossWorkspaceUnreadTotal = computed(
  () => unreadCount.value + (badgesEveryWorkspace() ? otherWorkspacesUnread.value : 0),
);

export const notificationsMenuUnreadTotal = computed(() =>
  notifyRows({
    peers: peerWorkspaces.value,
    remotePeers: remotePeerWorkspaces.value,
    ownUnread: unreadCount.value,
    ownId: WORKSPACE_ID,
    ownName: null,
  }).reduce((sum, row) => sum + row.count, 0),
);

export async function refreshOtherWorkspacesUnread(): Promise<void> {
  if (IS_PICKER || !badgesEveryWorkspace()) return;
  await Promise.all([refreshLocalPeers(), refreshRemotePeers()]);
}

async function refreshLocalPeers(): Promise<void> {
  const seq = ++refreshSeq;
  let rows: WorkspaceStatus[];
  try {
    rows = await listWorkspaces();
  } catch (e) {
    console.warn('[app-badge] cross-workspace unread refresh failed; keeping the last total', e);
    return;
  }
  if (seq !== refreshSeq) return;
  peerWorkspaces.value = rows;
  syncWorkspaceAppBadge();
}

async function refreshRemotePeers(): Promise<void> {
  const seq = ++remoteRefreshSeq;
  try {
    const listings = await listRemoteWorkspaces();
    if (seq !== remoteRefreshSeq) return;
    remotePeerWorkspaces.value = mergeRemoteListings(remotePeerWorkspaces.value, listings);
  } catch (e) {
    console.warn('[app-badge] remote workspace unread refresh failed; keeping the last rows', e);
  }
}

export function syncWorkspaceAppBadge(): void {
  if (IS_PICKER || isTauri()) return;
  applyAppBadge(crossWorkspaceUnreadTotal.value);
}
