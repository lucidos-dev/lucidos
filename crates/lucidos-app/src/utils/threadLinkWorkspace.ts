
import { REMOTE_ROUTE_SEPARATOR } from '../generated/gateway-constants';
import { workspaceRouteSegment } from './basePath';
import { slugifyWorkspaceName } from './slug';

export const THREAD_LINK_SOURCE =
  `thread:(?:([a-zA-Z0-9_-]+(?:${REMOTE_ROUTE_SEPARATOR}[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)?)\\/)?([0-9a-f-]+)`;

export const THREAD_LINK_RE = new RegExp(`^${THREAD_LINK_SOURCE}$`);

export interface LinkedWorkspace {
  name: string;
  gateway: string | null;
}

export function resolveLinkedWorkspace(raw: string, pageGateway: string | null): LinkedWorkspace {
  const at = raw.indexOf(REMOTE_ROUTE_SEPARATOR);
  return at < 0
    ? { name: raw, gateway: pageGateway }
    : { name: raw.slice(0, at), gateway: raw.slice(at + 1) };
}

export function isThisWorkspace(
  linked: LinkedWorkspace,
  pageGateway: string | null,
  pageName: string,
): boolean {
  return linked.gateway === pageGateway && linked.name === pageName;
}

export function linkedWorkspaceRef(name: string, pageGateway: string | null): string {
  return pageGateway === null ? name : `${name}${REMOTE_ROUTE_SEPARATOR}${pageGateway}`;
}

export function linkedWorkspaceRoute(linked: LinkedWorkspace): string {
  return workspaceRouteSegment({ slug: slugifyWorkspaceName(linked.name), gateway: linked.gateway });
}
