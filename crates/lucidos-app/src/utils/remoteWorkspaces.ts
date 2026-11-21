/**
 * The picker's view of remote workspaces: each remote gateway's workspaces,
 * kept through a gateway's silence so its rows grey out instead of vanishing.
 *
 * Shared merging, routing and device cache for the picker and switcher.
 */

import type { ListedWorkspace, RemoteGatewayListing } from '../api/client/control';
import { workspaceState } from './workspaceState';
import { parseWorkspaceRoute, isWorkspaceSlug, workspaceRouteSegment } from './basePath';

/** One remote gateway as the picker draws it. */
export interface RemoteGatewayView {
  gateway: string;
  host?: string;
  pairingId?: string;
  lastSeenAt?: string | null;
  reasonKind?: string | null;
  /** The newest workspaces the gateway listed. Empty until it first answers. */
  workspaces: ListedWorkspace[];
  /** Why the gateway did not answer the newest ask, or null when it did. */
  unreachable: string | null;
}

/** Fold a fresh listing onto the previous view. A gateway that answered
 *  replaces its rows. One that did not keeps its last rows, marked unreachable.
 *  A gateway missing from `next` was removed, so its rows go. */
export function mergeRemoteListings(
  previous: readonly RemoteGatewayView[],
  next: readonly RemoteGatewayListing[],
): RemoteGatewayView[] {
  return next.map((listing) =>
    listing.state === 'reachable'
      ? { gateway: listing.gateway, workspaces: listing.workspaces, unreachable: null, ...metadata(listing) }
      : {
          gateway: listing.gateway,
          workspaces: previous.find((p) => p.gateway === listing.gateway && p.pairingId === listing.pairing_id && p.host === listing.host)?.workspaces ?? [],
          unreachable: listing.reason,
          ...metadata(listing),
          lastSeenAt: listing.last_seen_at ?? previous.find((p) => p.gateway === listing.gateway && p.pairingId === listing.pairing_id && p.host === listing.host)?.lastSeenAt,
        },
  );
}

/** The route a remote workspace opens at on this gateway: `<slug>@<gateway>`. */
export function remoteWorkspaceRoute(view: RemoteGatewayView, workspace: ListedWorkspace): string {
  return workspaceRouteSegment({ slug: workspace.id, gateway: view.gateway });
}

function metadata(listing: RemoteGatewayListing): Partial<RemoteGatewayView> {
  return {
    ...(listing.pairing_id !== undefined ? { pairingId: listing.pairing_id } : {}),
    ...(listing.host !== undefined ? { host: listing.host } : {}),
    ...(listing.last_seen_at !== undefined ? { lastSeenAt: listing.last_seen_at } : {}),
    ...('reason_kind' in listing ? { reasonKind: listing.reason_kind } : {}),
  };
}

export function remoteWorkspaceOpenable(view: RemoteGatewayView, workspace: ListedWorkspace): boolean {
  const state = workspaceState(workspace);
  return view.unreachable === null && (state === 'healthy' || state === 'booting');
}

export const REMOTE_WORKSPACES_CACHE_KEY = 'lucidos-remote-workspaces';

export function recallRemoteListings(): RemoteGatewayView[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(REMOTE_WORKSPACES_CACHE_KEY) ?? '[]');
    if (!Array.isArray(value)) return [];
    return value.filter((v): v is RemoteGatewayView =>
      typeof v?.gateway === 'string' && parseWorkspaceRoute(`cache@${v.gateway}`) !== null &&
      (v.host === undefined || typeof v.host === 'string') &&
      (v.lastSeenAt == null || (typeof v.lastSeenAt === 'string' && Number.isFinite(Date.parse(v.lastSeenAt)))) &&
      Array.isArray(v.workspaces) &&
      v.workspaces.every((w: ListedWorkspace) => typeof w?.id === 'string' && isWorkspaceSlug(w.id) && typeof w.name === 'string' &&
        ['healthy', 'booting', 'restoring', 'unhealthy'].includes(w.health) &&
        (w.last_error == null || typeof w.last_error === 'string')))
      .map((v) => ({ ...v, unreachable: 'Checking gateway' }));
  } catch { return []; }
}

export function rememberRemoteListings(views: readonly RemoteGatewayView[]): void {
  try { localStorage.setItem(REMOTE_WORKSPACES_CACHE_KEY, JSON.stringify(views)); } catch { /* Storage can be disabled. */ }
}
