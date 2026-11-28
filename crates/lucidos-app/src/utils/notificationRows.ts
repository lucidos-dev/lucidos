import type { WorkspaceStatus } from '../api/client/control';
import { remoteWorkspaceRoute, type RemoteGatewayView } from './remoteWorkspaces';
import { workspaceState, type WorkspaceState } from './workspaceState';

export interface NotifyRow {
  id: string | null;
  name: string;
  count: number;
  isSelf: boolean;
  state: WorkspaceState;
}

export interface NotifyRowsInput {
  peers: WorkspaceStatus[];
  remotePeers: readonly RemoteGatewayView[];
  ownUnread: number;
  ownId: string | null;
  ownName: string | null;
}

const OWN_FALLBACK_NAME = 'This workspace';

export function notifyRows({ peers, remotePeers, ownUnread, ownId, ownName }: NotifyRowsInput): NotifyRow[] {
  const rows: NotifyRow[] = [];
  if (ownUnread > 0) {
    rows.push({
      id: ownId,
      name: ownName ?? OWN_FALLBACK_NAME,
      count: ownUnread,
      isSelf: true,
      state: 'healthy',
    });
  }
  for (const w of peers) {
    if (w.id === ownId) continue;
    const count = w.unread_count ?? 0;
    if (count > 0) {
      rows.push({ id: w.id, name: w.name, count, isSelf: false, state: workspaceState(w) });
    }
  }
  for (const view of remotePeers) {
    if (view.unreachable !== null) continue;
    for (const w of view.workspaces) {
      const route = remoteWorkspaceRoute(view, w);
      const count = w.unread_count ?? 0;
      if (route !== ownId && count > 0) {
        rows.push({ id: route, name: `${w.name} @${view.gateway}`, count, isSelf: false, state: workspaceState(w) });
      }
    }
  }
  return rows;
}

