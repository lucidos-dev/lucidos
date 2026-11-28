import { ApiError, fetchWithDeadline, throwIfNotOk } from '../api/client/_core';
import { REMOTE_PREFLIGHT_QUERY, workspaceRouteSegment, workspaceRouteUrlSegment } from './basePath';
import { deviceIdHeader } from './deviceIdHeader';
import { replaceDocument } from './documentNavigation';
import { remoteWorkspaceOpenable, remoteWorkspaceRoute, type RemoteGatewayView } from './remoteWorkspaces';
import { readWorkspaceClientMismatch, type WorkspaceClientMismatch } from './workspaceClientMismatch';

const PREFLIGHT_LIMIT = 6;
const PREFLIGHT_TIMEOUT_MS = 25_000;
const pending = new Map<string, Promise<void>>();

function preflight(route: string): Promise<void> {
  const existing = pending.get(route);
  if (existing) return existing;
  const result = fetchWithDeadline(`/${workspaceRouteUrlSegment(route)}/api/v1/health`, {
    cache: 'no-store',
    redirect: 'error',
    headers: { ...deviceIdHeader(), Accept: 'application/json' },
  }, PREFLIGHT_TIMEOUT_MS)
    .then((response) => throwIfNotOk(response, { routeMismatch: false }))
    .finally(() => { pending.delete(route); });
  pending.set(route, result);
  return result;
}

export function warmRemoteWorkspaces(views: readonly RemoteGatewayView[], currentId: string | null = null): void {
  const routes = views.flatMap((view) => view.workspaces.flatMap((workspace) =>
    remoteWorkspaceOpenable(view, workspace) ? [remoteWorkspaceRoute(view, workspace)] : []));
  for (const route of routes.filter((route) => route !== currentId).slice(0, PREFLIGHT_LIMIT)) {
    void preflight(route).catch(() => {});
  }
}

export async function openRemoteInPlace(route: string, active: () => boolean): Promise<WorkspaceClientMismatch | null> {
  try {
    await preflight(route);
  } catch (error) {
    const mismatch = error instanceof ApiError && error.httpCode === 426 ? readWorkspaceClientMismatch(error.body) : null;
    if (mismatch && workspaceRouteSegment(mismatch.route) === route) return mismatch;
    throw error;
  }
  if (active()) replaceDocument(`/${workspaceRouteUrlSegment(route)}/?${REMOTE_PREFLIGHT_QUERY}`);
  return null;
}
