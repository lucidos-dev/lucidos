import { signal } from '@preact/signals';
import { showToast, workspaceName } from '../store';
import { fetchWorkspaces } from '../../api/client';
import { listWorkspaces, locateWorkspace, slugifyWorkspaceName } from '../../api/client/control';
import { GATEWAY_PORT, WORKSPACE_ID } from '../../utils/basePath';
import { isTauri } from '../../utils/platform';
import { openUrl } from './artifacts';
import { focusThreadOrBootstrap } from './threads';
import { errorDetail } from '../../utils/errorDetail';
import { openNewTab } from '../../utils/newTab';
import { workspaceTabName } from '../../utils/workspaceWindow';

/** Hash-channel deep link to a thread. Hash (not `?thread=`) is deliberate:
 *  `useStartup` strips `?thread=` unconditionally to defuse stale SW deep-
 *  links, so the query channel can't be repurposed for user-initiated
 *  cross-workspace navigation. */
export const THREAD_HASH_RE = /^#thread=([0-9a-f-]+)$/;

/** Where a peer workspace is reachable, and what to name the tab that shows it.
 *
 *  `tabKey` is the SLUG behind the gateway, which is what a workspace row keys
 *  its own tab on. So a thread link and a row reach one tab between them. On a
 *  dedicated port there is no slug, so the workspace name is the key. */
interface WorkspaceReach {
  base: string;
  tabKey: string;
}

/** Base URL (no trailing slash) for reaching a target workspace's engine from the
 *  page we're on — **preserving the access context we're already in** (ADR 0014):
 *
 *   • Served behind the gateway (`/<slug>/`, `WORKSPACE_ID` set) → reach peers
 *     through the *same gateway origin* by their slug path
 *     (`https://<gateway>/<slug>`). The slug is the authoritative registry id from
 *     the control listing (matched on name, then on a slugified-name id so a
 *     collision-suffixed slug still resolves).
 *   • Served directly on an engine port (`WORKSPACE_ID` null) → reach peers on
 *     *their own dedicated ports* (`https://<host>:<port>`), discovered via
 *     `/api/v1/workspaces`.
 *
 *  Either way it keeps the user's host (localhost or Tailscale) and never mixes
 *  the two topologies.
 *
 *  `lazyStart` is the navigation-vs-passive split: a user opening the thread
 *  (`true`) tolerates a stopped peer — the gateway boots it on the proxy hit, and
 *  if the control plane is unreachable we still hand back a slugified-name URL so
 *  the gateway can resolve it. A passive caller like the title fetch (`false`)
 *  must NOT boot a stopped peer just to read a title, so it resolves only a
 *  gateway-`healthy` peer and gives up when the control plane is unreachable.
 *
 *  Returns null when the workspace is absent / not reachable in the requested
 *  mode; throws only on an unexpected failure (the dedicated-port list request
 *  erroring) so callers can surface the cause.
 *
 *  This is THIS gateway's view, which is the whole of the passive caller's
 *  answer. A navigating caller takes a null on to `reachPeerInstall`, which
 *  looks across the machine's other installs. */
async function resolveWorkspaceBaseUrl(
  workspace: string,
  opts: { lazyStart: boolean },
): Promise<WorkspaceReach | null> {
  if (WORKSPACE_ID !== null) {
    // This page is served under a gateway slug prefix, so its origin IS the
    // gateway — route cross-workspace traffic back through it.
    let list: Awaited<ReturnType<typeof listWorkspaces>>;
    try {
      list = await listWorkspaces();
    } catch {
      // Control plane unreachable: navigation can still let the gateway resolve
      // (and lazy-start) the peer from the slugified name (exact when name ===
      // slug, the common case); a passive fetch gives up rather than guess.
      if (!opts.lazyStart) return null;
      const guess = slugifyWorkspaceName(workspace);
      return { base: `${location.origin}/${encodeURIComponent(guess)}`, tabKey: guess };
    }
    const entry =
      list.find(w => w.name === workspace) ??
      list.find(w => w.id === slugifyWorkspaceName(workspace));
    if (!entry) return null; // not registered with the gateway
    if (!opts.lazyStart && entry.health !== 'healthy') return null; // don't boot a stopped peer for a title
    return {
      base: `${location.origin}/${encodeURIComponent(entry.id)}`,
      tabKey: entry.id,
    };
  }
  const { workspaces } = await fetchWorkspaces();
  const entry = workspaces.find(w => w.name === workspace);
  if (!entry || !entry.engine_running || entry.port == null) return null;
  return {
    base: `${location.protocol}//${location.hostname}:${entry.port}`,
    tabKey: workspace,
  };
}

/** The port this page was actually reached on, with the scheme's default spelled
 *  out (`location.port` is empty on 443 and 80). */
function currentPort(): string {
  if (location.port) return location.port;
  return location.protocol === 'https:' ? '443' : '80';
}

/** Did this page reach its own gateway directly, on the gateway's own port?
 *
 *  Load-bearing for a peer hop, which works by swapping in the peer's port.
 *  Behind `tailscale serve` or an ssh forward, the page arrives on a port
 *  mapped to one gateway. Swapping that port guesses at an address nothing
 *  answers. */
function reachedOwnGatewayPort(): boolean {
  return GATEWAY_PORT !== null && currentPort() === String(GATEWAY_PORT);
}

/** Reach a workspace served by ANOTHER Lucidos install on this machine.
 *
 *  A machine can run several installs, each with its own gateway, port and
 *  registry (ADR 0189). Our own gateway has already said it does not serve this
 *  one, so this is the last place a cross-gateway thread link can resolve.
 *
 *  Nothing is proxied: the URL points at the peer gateway's own origin, which
 *  authenticates the browser itself. Widening a pairing minted here into
 *  authority over another install's workspaces is what ADR 0132 refused. An
 *  unpaired browser lands on that gateway's pairing screen, which is honest.
 *
 *  Owns its own failure message. Every `null` it returns has already told the
 *  user why, and the four reasons differ. */
async function reachPeerInstall(workspace: string): Promise<WorkspaceReach | null> {
  const unavailable = () => showToast(`Workspace '${workspace}' is not available`, 'error');
  // The control plane lives on the gateway. Served straight off an engine port
  // there is none to ask, so the answer is the one this always gave.
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
    // The peer's scheme, because the two gateways disagree on it. Our host,
    // because that is how the user reaches this machine at all.
    base: `${found.scheme}://${location.hostname}:${found.gateway_port}/${encodeURIComponent(found.slug)}`,
    // Keyed on the peer's port too: two installs may both serve a slug, and
    // one tab name between them would re-point the wrong workspace's tab.
    tabKey: `${found.gateway_port}-${found.slug}`,
  };
}

/** Open a thread that lives in a different Lucidos workspace. Navigates in the
 *  same access context the user is already in (see `resolveWorkspaceBaseUrl`):
 *  through the gateway when behind it (`https://<gateway>/<slug>/#thread=<uuid>`),
 *  or to the target engine's own port when served directly. A workspace our own
 *  gateway does not serve falls to `reachPeerInstall`, which looks across the
 *  machine's other installs.
 *
 *  The tab is NAMED, so the user's existing tab for that workspace is reused
 *  rather than duplicated. `openNewTab` owns the naming and the `noopener`
 *  reasoning. Going through it also reports a blocked pop-up, which a raw
 *  `window.open` here left as a dead click. */
export async function openThreadInWorkspace(workspace: string, threadId: string): Promise<void> {
  let reach: WorkspaceReach | null;
  try {
    reach =
      (await resolveWorkspaceBaseUrl(workspace, { lazyStart: true })) ??
      (await reachPeerInstall(workspace));
  } catch (e) {
    showToast(`Failed to open thread in workspace '${workspace}': ${errorDetail(e)}`, 'error');
    return;
  }
  // `reachPeerInstall` has already said why, and said it more precisely than
  // a shared message here could.
  if (reach === null) return;

  const url = `${reach.base}/#thread=${threadId}`;
  if (isTauri()) {
    openUrl(url);
    return;
  }
  if (!openNewTab(url, workspaceTabName(reach.tabKey))) {
    showToast(`Your browser blocked the tab for '${workspace}'. Allow pop-ups for this site.`, 'error');
  }
}

/** Route a thread link to the engine that owns it: a same-workspace link
 *  focuses the thread in place; a cross-workspace link hops to the target
 *  workspace's UI (its thread isn't in our `threadMap`). `workspace` undefined
 *  (an untagged link) is always treated as same-workspace. Shared by the global
 *  `.thread-link` click handler (useStartup) and the message-route popover's
 *  Workspace-origin link so the two routing decisions can't drift. */
export function openThreadAcrossWorkspaces(workspace: string | undefined, threadId: string): void {
  if (workspace && workspaceName.value && workspace !== workspaceName.value) {
    void openThreadInWorkspace(workspace, threadId);
    return;
  }
  focusThreadOrBootstrap(threadId);
}

/** Cache of cross-workspace thread titles, keyed by `encodeURIComponent(workspace)`
 *  + `/` + thread id (the encode keeps the key unambiguous if a workspace name
 *  ever contains the separator). Populated lazily by
 *  `ensureCrossWorkspaceThreadTitle`; read synchronously by the message-route
 *  popover so a Workspace-origin link shows the real thread name instead of a
 *  UUID. A signal so the popover re-renders when a title arrives. */
const crossWsTitles = signal<Map<string, string>>(new Map());
const crossWsInFlight = new Set<string>();
const crossWsKey = (workspace: string, threadId: string) =>
  `${encodeURIComponent(workspace)}/${threadId}`;

/** Current cached title for a cross-workspace thread, or undefined if not yet
 *  resolved. Reading this inside a component render subscribes it to updates. */
export function crossWorkspaceThreadTitle(workspace: string, threadId: string): string | undefined {
  return crossWsTitles.value.get(crossWsKey(workspace, threadId));
}

/** Best-effort: fetch a thread's current title from another workspace's engine
 *  and cache it, over the same access context as navigation (see
 *  `resolveWorkspaceBaseUrl`) — same-origin via the gateway when behind it, or
 *  the target engine's port when served directly. Passive (`lazyStart: false`):
 *  it reads the title only from an already-running peer and never boots a stopped
 *  one just to render a popover, falling back to the short-id label otherwise.
 *  Stays fresh across renames. Deduped by key; only successes are cached, so a
 *  transient failure retries on the next call. */
export async function ensureCrossWorkspaceThreadTitle(
  workspace: string,
  threadId: string,
): Promise<void> {
  const key = crossWsKey(workspace, threadId);
  if (crossWsTitles.value.has(key) || crossWsInFlight.has(key)) return;
  crossWsInFlight.add(key);
  try {
    const reach = await resolveWorkspaceBaseUrl(workspace, { lazyStart: false });
    if (!reach) return;
    const res = await fetch(`${reach.base}/api/v1/threads/${threadId}`);
    if (!res.ok) return;
    const summary = (await res.json()) as { title?: string };
    const title = summary.title?.trim();
    if (title) {
      const next = new Map(crossWsTitles.value);
      next.set(key, title);
      crossWsTitles.value = next;
    }
  } catch (e) {
    // Best-effort telemetry carve-out (frontend.md): this fetch runs without
    // user intent (fired from the popover render), the link still works and the
    // short-id fallback renders, and the next popover open retries — so a toast
    // would be wrong. console.warn keeps the signal for debugging.
    console.warn(`cross-workspace title fetch failed for ${workspace}/${threadId}:`, errorDetail(e));
  } finally {
    crossWsInFlight.delete(key);
  }
}
