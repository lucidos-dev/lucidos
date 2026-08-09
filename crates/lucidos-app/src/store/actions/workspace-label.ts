/**
 * Resolving this workspace's DISPLAY LABEL from the workspace gateway.
 *
 * The engine only knows its own directory name (`/api/v1/health` `workspace`),
 * and a rename in the picker is a registry write the engine is never told about
 * (ADR 0014: rename edits the registry `name`, nothing moves). So after a rename
 * the app showed the pre-rename name in its header, System page and Files
 * dropdown, while the in-app switcher right beside them, which reads the gateway
 * listing, showed the new one.
 *
 * The label therefore comes from the same place the picker gets it: the control
 * listing, matched on this page's own slug. Identity keeps coming from the
 * engine (`workspaceName`), because thread-ref links embed it in durable text
 * and two workspaces may share a label.
 */

import { workspaceDisplayName } from '../store';
import { listWorkspaces, type WorkspaceStatus } from '../../api/client/control';
import { WORKSPACE_ID } from '../../utils/basePath';

/** Adopt the label for THIS workspace out of a control listing someone else
 *  already fetched (the switcher fetches it on every open, which is how a rename
 *  made elsewhere shows up without a reload). A listing that doesn't mention us
 *  changes nothing. */
export function adoptWorkspaceDisplayName(list: readonly WorkspaceStatus[]): void {
  if (WORKSPACE_ID === null) return;
  const entry = list.find((w) => w.id === WORKSPACE_ID);
  if (entry) workspaceDisplayName.value = entry.name;
}

/** Startup: ask the gateway what this workspace is called. No-op when we aren't
 *  served under a gateway slug (a direct engine has no registry to ask, and the
 *  engine name is then the only name there is). */
export async function loadWorkspaceDisplayName(): Promise<void> {
  if (WORKSPACE_ID === null) return;
  try {
    adoptWorkspaceDisplayName(await listWorkspaces());
  } catch (e) {
    // Best-effort startup probe (frontend.md carve-out), deliberately not a
    // toast: the user did not ask for this, and the display falls back to the
    // engine's own name so nothing is blank or wrong-looking. This is now the
    // ONLY caller: the workspace switcher used to re-resolve the label on every
    // open, and renaming lives on the gateway picker, which the app reloads
    // through, so the next boot picks the new name up. In legacy no-gateway mode
    // this route does not exist at all, which is a normal condition rather than
    // a failure.
    console.warn('[workspace] display label unavailable; using the engine name', e);
  }
}
