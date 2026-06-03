import { signal } from '@preact/signals';
import { showToast, workspaceName } from '../store';
import { fetchWorkspaces } from '../../api/client';
import { isTauri } from '../../utils/platform';
import { openUrl } from './artifacts';
import { focusThreadOrBootstrap } from './threads';
import { errorDetail } from '../../utils/errorDetail';

/** Hash-channel deep link to a thread. Hash (not `?thread=`) is deliberate:
 *  `useStartup` strips `?thread=` unconditionally to defuse stale SW deep-
 *  links, so the query channel can't be repurposed for user-initiated
 *  cross-workspace navigation. */
export const THREAD_HASH_RE = /^#thread=([0-9a-f-]+)$/;

/** Open a thread that lives in a different Lucidos workspace. Each workspace
 *  runs its own engine on its own port, so we discover the target via
 *  `/api/v1/workspaces` and navigate to `https://localhost:<port>/#thread=<uuid>`.
 *  We use a named browser target so the user's existing tab for that workspace
 *  (if any) is reused instead of accumulating duplicate tabs — `noopener`
 *  would force `_blank` per the HTML spec, so it's omitted intentionally. */
export async function openThreadInWorkspace(workspace: string, threadId: string): Promise<void> {
  let entry: Awaited<ReturnType<typeof fetchWorkspaces>>['workspaces'][number] | undefined;
  try {
    const { workspaces } = await fetchWorkspaces();
    entry = workspaces.find(w => w.name === workspace);
  } catch (e) {
    showToast(`Failed to open thread in workspace '${workspace}': ${errorDetail(e)}`, 'error');
    return;
  }

  if (!entry) {
    showToast(`Workspace '${workspace}' not found`, 'error');
    return;
  }
  if (!entry.engine_running || entry.port == null) {
    showToast(`Workspace '${workspace}' is not running`, 'error');
    return;
  }

  const url = `https://localhost:${entry.port}/#thread=${threadId}`;
  if (isTauri()) {
    openUrl(url);
    return;
  }
  window.open(url, `lucidos-ws-${workspace}`);
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
 *  and cache it. The engine serves CORS-permissive and a cross-workspace link
 *  already requires the source workspace to be running, so resolving the title
 *  live adds no new requirement and stays fresh across renames. Deduped by key;
 *  only successes are cached, so a transient failure retries on the next call. */
export async function ensureCrossWorkspaceThreadTitle(
  workspace: string,
  threadId: string,
): Promise<void> {
  const key = crossWsKey(workspace, threadId);
  if (crossWsTitles.value.has(key) || crossWsInFlight.has(key)) return;
  crossWsInFlight.add(key);
  try {
    const { workspaces } = await fetchWorkspaces();
    const entry = workspaces.find(w => w.name === workspace);
    if (!entry?.engine_running || entry.port == null) return;
    const res = await fetch(`https://localhost:${entry.port}/api/v1/threads/${threadId}`);
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
