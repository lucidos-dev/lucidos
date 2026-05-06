import { showToast } from '../store';
import { fetchWorkspaces } from '../../api/client';
import { isTauri } from '../../utils/platform';
import { openUrl } from './artifacts';
import { errorDetail } from '../../utils/errorDetail';

/** Hash-channel deep link to a thread. Hash (not `?thread=`) is deliberate:
 *  `useStartup` strips `?thread=` unconditionally to defuse stale SW deep-
 *  links, so the query channel can't be repurposed for user-initiated
 *  cross-workspace navigation. */
export const THREAD_HASH_RE = /^#thread=([0-9a-f-]+)$/;

/** Open a thread that lives in a different Lucidos workspace. Each workspace
 *  runs its own engine on its own port, so we discover the target via
 *  `/api/workspaces` and navigate to `https://localhost:<port>/#thread=<uuid>`.
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
