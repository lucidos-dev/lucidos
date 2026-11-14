/**
 * The workspace a `thread:` link names, and which gateway it lives on.
 *
 * A link reads `thread:<uuid>`, `thread:<name>/<uuid>`, or
 * `thread:<name>@<gateway>/<uuid>` for a workspace on a remote gateway (ADR
 * 0591). An engine always writes the middle form with
 * its own workspace's name. So inside a remote workspace an unqualified name
 * means that remote gateway's workspace, never this gateway's workspace of the
 * same name.
 *
 * A leaf module: `renderMarkdown`, the preview link router and the
 * cross-workspace router all read links, and none may import another.
 */

import { REMOTE_ROUTE_SEPARATOR } from '../generated/gateway-constants';
import { workspaceRouteSegment } from './basePath';
import { slugifyWorkspaceName } from './slug';

/** A thread link, unanchored, capturing the optional workspace and the
 *  thread id. The qualifier takes the gateway-name shape, so a stray `@` reads
 *  as no link. */
export const THREAD_LINK_SOURCE =
  `thread:(?:([a-zA-Z0-9_-]+(?:${REMOTE_ROUTE_SEPARATOR}[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)?)\\/)?([0-9a-f-]+)`;

/** A whole href that is a thread link. */
export const THREAD_LINK_RE = new RegExp(`^${THREAD_LINK_SOURCE}$`);

/** A link's workspace, resolved against the page it sits on. */
export interface LinkedWorkspace {
  name: string;
  /** The remote gateway's short name, or null for this gateway's own. */
  gateway: string | null;
}

/** Resolve `raw`, a link's workspace part, on a page whose workspace lives on
 *  `pageGateway` (null for this gateway). A qualified name resolves as
 *  written. An unqualified one takes the page's gateway. */
export function resolveLinkedWorkspace(raw: string, pageGateway: string | null): LinkedWorkspace {
  const at = raw.indexOf(REMOTE_ROUTE_SEPARATOR);
  return at < 0
    ? { name: raw, gateway: pageGateway }
    : { name: raw.slice(0, at), gateway: raw.slice(at + 1) };
}

/** Does `linked` name the page's own workspace? */
export function isThisWorkspace(
  linked: LinkedWorkspace,
  pageGateway: string | null,
  pageName: string,
): boolean {
  return linked.gateway === pageGateway && linked.name === pageName;
}

/** The workspace part a copied ref carries, qualified inside a remote
 *  workspace so it keeps meaning when pasted on a local page. */
export function linkedWorkspaceRef(name: string, pageGateway: string | null): string {
  return pageGateway === null ? name : `${name}${REMOTE_ROUTE_SEPARATOR}${pageGateway}`;
}

/** The route segment a link's workspace most likely sits at, from its name
 *  alone. A caller that can list the workspaces matches on them first. */
export function linkedWorkspaceRoute(linked: LinkedWorkspace): string {
  return workspaceRouteSegment({ slug: slugifyWorkspaceName(linked.name), gateway: linked.gateway });
}
