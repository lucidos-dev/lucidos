import type { WorkspaceStatus } from '../api/client/control';
import { remoteWorkspaceRoute, type RemoteGatewayView } from './remoteWorkspaces';
import { workspaceState, type WorkspaceState } from './workspaceState';

/** One row's worth of the group, resolved from the two count sources. */
export interface NotifyRow {
  /** Registry slug, or `null` for this workspace on a non-gateway page. */
  id: string | null;
  name: string;
  count: number;
  /** True for the workspace this page serves, which routes in-app. */
  isSelf: boolean;
  /** How the gateway reports that workspace's engine. Carried because opening
   *  one is `alternateOpenMode`'s and `middleClickActivates`' decision, and
   *  both ask the state. This workspace's own row is `healthy` by
   *  construction: this page is running inside it. */
  state: WorkspaceState;
}

export interface NotifyRowsInput {
  /** Every workspace the gateway serves, from the shared cache in
   *  `store/actions/app-badge.ts`. Empty outside a gateway-served page. */
  peers: WorkspaceStatus[];
  /** Every paired remote gateway's workspaces, from the same cache. A gateway
   *  that did not answer the newest ask contributes no row. */
  remotePeers: readonly RemoteGatewayView[];
  /** This workspace's LIVE unread count, never its polled row. See
   *  {@link notifyRows}. */
  ownUnread: number;
  /** This workspace's slug, or `null` on a legacy no-gateway page. */
  ownId: string | null;
  /** What to call this workspace. Falls back when `/health` has not answered. */
  ownName: string | null;
}

/** What this workspace's row is called before its name has resolved. */
const OWN_FALLBACK_NAME = 'This workspace';

/** Select unread rows using live state for the current route and listings for peers. */
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

