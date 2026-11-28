import { retryTransportRead } from '../../../../../packages/lucidos-sdk/src/_transport';
import { signal } from '@preact/signals';
import { showToast, workspaceName } from '../store';
import { fetchWorkspaces } from '../../api/client';
import { listRemoteWorkspaces, listWorkspaces, locateWorkspace } from '../../api/client/control';
import {
  GATEWAY_PORT,
  REMOTE_GATEWAY,
  WORKSPACE_ID,
  parseWorkspaceRoute,
  workspaceRouteSegment,
  workspaceRouteUrlSegment,
} from '../../utils/basePath';
import { slugifyWorkspaceName } from '../../utils/slug';
import { isTauri } from '../../utils/platform';
import { showWorkspaceInNativeWindow } from '../../utils/tauri';
import { openUrl } from './artifacts';
import { focusThreadOrBootstrap } from './threads';
import { errorDetail } from '../../utils/errorDetail';
import { openNewTab } from '../../utils/newTab';
import { replaceDocument } from '../../utils/documentNavigation';
import { isCanonicalThreadId, landingHash } from '../../utils/workspaceLanding';
import { separateViewIsAvailable, workspaceTabName } from '../../utils/workspaceWindow';
import {
  isThisWorkspace,
  linkedWorkspaceRoute,
  resolveLinkedWorkspace,
  type LinkedWorkspace,
} from '../../utils/threadLinkWorkspace';

import { THREAD_HASH_RE } from './threadHash';
export { THREAD_HASH_RE } from './threadHash';


interface WorkspaceReach {
  base: string;
  tabKey: string;
  gatewayRoute: string | null;
}

function reachOnThisGateway(route: string): WorkspaceReach {
  return { base: `${location.origin}/${workspaceRouteUrlSegment(route)}`, tabKey: route, gatewayRoute: route };
}

async function reachRemoteWorkspace(
  linked: LinkedWorkspace,
  gateway: string,
  opts: { lazyStart: boolean },
): Promise<WorkspaceReach | null> {
  const guess = () => (opts.lazyStart ? reachOnThisGateway(linkedWorkspaceRoute(linked)) : null);
  let listings: Awaited<ReturnType<typeof listRemoteWorkspaces>>;
  try {
    listings = await listRemoteWorkspaces();
  } catch {
    return guess();
  }
  const listing = listings.find((l) => l.gateway === gateway);
  if (!listing) return null;
  if (listing.state === 'unreachable') return guess();
  const entry =
    listing.workspaces.find((w) => w.name === linked.name) ??
    listing.workspaces.find((w) => w.id === slugifyWorkspaceName(linked.name));
  if (!entry) return null;
  if (!opts.lazyStart && entry.health !== 'healthy') return null;
  return reachOnThisGateway(workspaceRouteSegment({ slug: entry.id, gateway }));
}

async function resolveWorkspaceBaseUrl(
  linked: LinkedWorkspace,
  opts: { lazyStart: boolean },
): Promise<WorkspaceReach | null> {
  const workspace = linked.name;
  if (linked.gateway !== null) {
    return WORKSPACE_ID === null ? null : reachRemoteWorkspace(linked, linked.gateway, opts);
  }
  if (WORKSPACE_ID !== null) {
    let list: Awaited<ReturnType<typeof listWorkspaces>>;
    try {
      list = await listWorkspaces();
    } catch {
      if (!opts.lazyStart) return null;
      return reachOnThisGateway(slugifyWorkspaceName(workspace));
    }
    const entry =
      list.find(w => w.name === workspace) ??
      list.find(w => w.id === slugifyWorkspaceName(workspace));
    if (!entry) return null;
    if (!opts.lazyStart && entry.health !== 'healthy') return null;
    return reachOnThisGateway(entry.id);
  }
  const { workspaces } = await fetchWorkspaces();
  const entry =
    workspaces.find(w => w.name === workspace) ??
    workspaces.find(w => w.id === slugifyWorkspaceName(workspace));
  if (!entry || entry.health !== 'healthy') return null;
  return {
    base: `${location.protocol}//${location.hostname}:${entry.port}`,
    tabKey: workspace,
    gatewayRoute: null,
  };
}

function currentPort(): string {
  if (location.port) return location.port;
  return location.protocol === 'https:' ? '443' : '80';
}

function reachedOwnGatewayPort(): boolean {
  return GATEWAY_PORT !== null && currentPort() === String(GATEWAY_PORT);
}

async function reachPeerInstall(workspace: string): Promise<WorkspaceReach | null> {
  const unavailable = () => showToast(`Workspace '${workspace}' is not available`, 'error');
  if (WORKSPACE_ID === null) {
    unavailable();
    return null;
  }
  const found = await locateWorkspace(workspace);
  if (found === null) {
    unavailable();
    return null;
  }
  if (found.status === 'ambiguous') {
    showToast(
      `Workspace '${workspace}' exists on ${found.installs.join(' and ')}. ` +
        `Open it from the one you mean.`,
      'error',
    );
    return null;
  }
  if (found.status === 'install-not-running') {
    showToast(`Workspace '${workspace}' lives on ${found.install}, which is not running`, 'error');
    return null;
  }
  if (!reachedOwnGatewayPort()) {
    showToast(
      `Workspace '${workspace}' lives on ${found.install}, on port ${found.gateway_port}. ` +
        `This address cannot reach it.`,
      'error',
    );
    return null;
  }
  return {
    base: `${location.protocol}//${location.hostname}:${found.gateway_port}/${encodeURIComponent(found.slug)}`,
    tabKey: `${found.gateway_port}-${found.slug}`,
    gatewayRoute: null,
  };
}

export async function openThreadInWorkspace(workspace: string, threadId: string): Promise<void> {
  const linked = resolveLinkedWorkspace(workspace, REMOTE_GATEWAY);
  let reach: WorkspaceReach | null;
  try {
    reach =
      (await resolveWorkspaceBaseUrl(linked, { lazyStart: true })) ??
      (linked.gateway === null ? await reachPeerInstall(workspace) : remoteUnavailable(linked));
  } catch (e) {
    toastOpenThreadFailed(workspace, e);
    return;
  }
  if (reach === null) return;
  await openThreadAt(reach, threadId, workspace);
}

function remoteUnavailable(linked: LinkedWorkspace): null {
  showToast(`Workspace '${linked.name}' on remote gateway '${linked.gateway}' is not available`, 'error');
  return null;
}

function toastOpenThreadFailed(workspace: string, err: unknown): void {
  showToast(`Failed to open thread in workspace '${workspace}': ${errorDetail(err)}`, 'error');
}

async function openThreadAt(reach: WorkspaceReach, threadId: string, workspace: string): Promise<void> {
  const landing = { thread: threadId };
  if (isTauri() && reach.gatewayRoute !== null) {
    try {
      await showWorkspaceInNativeWindow(reach.gatewayRoute, landing);
    } catch (e) {
      toastOpenThreadFailed(workspace, e);
    }
    return;
  }
  const url = `${reach.base}/${landingHash(landing)}`;
  if (isTauri()) {
    openUrl(url);
    return;
  }
  if (!separateViewIsAvailable() && new URL(url).origin === location.origin) {
    replaceDocument(url);
    return;
  }
  if (!openNewTab(url, workspaceTabName(reach.tabKey))) {
    showToast(`Your browser blocked the tab for '${workspace}'. Allow pop-ups for this site.`, 'error');
  }
}

export interface WorkspaceThreadUrl {
  route: string;
  threadId: string;
}

export function parseWorkspaceThreadUrl(href: string): WorkspaceThreadUrl | null {
  let url: URL;
  try {
    url = new URL(href);
  } catch {
    return null;
  }
  if (url.origin !== location.origin || url.search !== '') return null;
  const route = /^\/([^/]+)\/?$/.exec(url.pathname)?.[1];
  const threadId = THREAD_HASH_RE.exec(url.hash)?.[1];
  if (!route || !threadId || !parseWorkspaceRoute(route) || !isCanonicalThreadId(threadId)) return null;
  return { route, threadId };
}

export function openWorkspaceThreadUrl({ route, threadId }: WorkspaceThreadUrl): void {
  if (route === WORKSPACE_ID) {
    focusThreadOrBootstrap(threadId);
    return;
  }
  openThreadAt(reachOnThisGateway(route), threadId, route).catch((e) => toastOpenThreadFailed(route, e));
}

export function openThreadAcrossWorkspaces(workspace: string | undefined, threadId: string): void {
  if (
    workspace &&
    workspaceName.value &&
    !isThisWorkspace(resolveLinkedWorkspace(workspace, REMOTE_GATEWAY), REMOTE_GATEWAY, workspaceName.value)
  ) {
    void openThreadInWorkspace(workspace, threadId);
    return;
  }
  focusThreadOrBootstrap(threadId);
}

const crossWsTitles = signal<Map<string, string>>(new Map());
const crossWsInFlight = new Set<string>();
const crossWsKey = (workspace: string, threadId: string) =>
  `${encodeURIComponent(workspace)}/${threadId}`;

export function crossWorkspaceThreadTitle(workspace: string, threadId: string): string | undefined {
  return crossWsTitles.value.get(crossWsKey(workspace, threadId));
}

export async function ensureCrossWorkspaceThreadTitle(
  workspace: string,
  threadId: string,
): Promise<void> {
  const key = crossWsKey(workspace, threadId);
  if (crossWsTitles.value.has(key) || crossWsInFlight.has(key)) return;
  crossWsInFlight.add(key);
  try {
    const reach = await resolveWorkspaceBaseUrl(resolveLinkedWorkspace(workspace, REMOTE_GATEWAY), {
      lazyStart: false,
    });
    if (!reach) return;
    const res = await retryTransportRead(() => fetch(`${reach.base}/api/v1/threads/${threadId}`));
    if (!res.ok) return;
    const summary = (await res.json()) as { title?: string };
    const title = summary.title?.trim();
    if (title) {
      const next = new Map(crossWsTitles.value);
      next.set(key, title);
      crossWsTitles.value = next;
    }
  } catch (e) {
    console.warn(`cross-workspace title fetch failed for ${workspace}/${threadId}:`, errorDetail(e));
  } finally {
    crossWsInFlight.delete(key);
  }
}
