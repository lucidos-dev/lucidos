/**
 * The picker's view of remote workspaces: each remote gateway's workspaces,
 * kept through a gateway's silence so its rows grey out instead of vanishing.
 *
 * Pure, so the merge is unit-testable apart from the picker's rendering.
 */

import type { ListedWorkspace, RemoteGatewayListing } from '../../api/client/control';
import { workspaceRouteSegment } from '../../utils/basePath';

/** One remote gateway as the picker draws it. */
export interface RemoteGatewayView {
  gateway: string;
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
      ? { gateway: listing.gateway, workspaces: listing.workspaces, unreachable: null }
      : {
          gateway: listing.gateway,
          workspaces: previous.find((p) => p.gateway === listing.gateway)?.workspaces ?? [],
          unreachable: listing.reason,
        },
  );
}

/** The route a remote workspace opens at on this gateway: `<slug>@<gateway>`. */
export function remoteWorkspaceRoute(view: RemoteGatewayView, workspace: ListedWorkspace): string {
  return workspaceRouteSegment({ slug: workspace.id, gateway: view.gateway });
}
