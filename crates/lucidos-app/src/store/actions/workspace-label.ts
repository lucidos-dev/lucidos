/**
 * Resolving this workspace's DISPLAY LABEL.
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
 *
 * TWO WAYS TO THE SAME REGISTRY, decided by how this page is served:
 *
 *   • Behind the gateway (`<base href="/<slug>/">`, so `WORKSPACE_ID` is set):
 *     read the listing directly and match on the slug. This page has a SECOND
 *     adopter, the in-app workspace switcher, which fetches its own listing
 *     every time its row is unfolded and hands it to
 *     {@link adoptWorkspaceDisplayName}, so a rename made in the picker lands
 *     here without a reload.
 *   • On the engine's OWN port (no `<base href>` at all, so `WORKSPACE_ID` is
 *     null): ask our own engine, which asks the gateway over loopback. This
 *     page cannot read the listing itself, and not for want of a URL: the
 *     listing is on the gateway ORIGIN, a different port, and `control_authz`
 *     (`lucidos-gateway/src/control.rs`) refuses any browser request that is not
 *     same-origin. That is a CSRF boundary, not an oversight, so the engine
 *     answers instead (`api/workspace_label.rs`). Before that route existed a
 *     direct-port page returned early here and showed the engine's directory
 *     name forever, which is what an installed iOS PWA on the engine port was
 *     doing after a rename.
 *
 * The switcher is not a third way to the registry, it is the first way used
 * twice: it gates on `WORKSPACE_ID !== null` (`WorkspaceSwitcher.tsx`'s
 * `canList`, the stricter of that component's two gates), so on a direct-port
 * page it never lists and this module is the only adopter.
 *
 * WHICH IS WHY BOTH ROUTES ARE ALSO RE-RUN ON RESUME (`useStartup.ts`'s
 * `onResume`). "The next load picks it up" is a promise the reporting device
 * cannot keep: an installed iOS PWA does not reload when it returns from
 * background, which is the premise of that whole handler. Behind the gateway
 * the switcher would eventually correct the name on its next unfold; on a
 * direct-port page nothing else ever would.
 */

import { workspaceDisplayName } from '../store';
import { listWorkspaces, type WorkspaceStatus } from '../../api/client/control';
import { getWorkspaceLabel } from '../../api/client';
import { IS_PICKER, WORKSPACE_ID } from '../../utils/basePath';

/** Adopt the label for THIS workspace out of a control listing someone else
 *  already fetched (the in-app workspace switcher fetches one every time its row
 *  is unfolded, which is how a rename made in the picker shows up without a
 *  reload). A listing that doesn't mention us changes nothing: our slug is
 *  frozen, so not being in it means the registry no longer carries this
 *  workspace, and the engine's own name is then the only name there is. */
export function adoptWorkspaceDisplayName(list: readonly WorkspaceStatus[]): void {
  if (WORKSPACE_ID === null) return;
  const entry = list.find((w) => w.id === WORKSPACE_ID);
  if (entry) workspaceDisplayName.value = entry.name;
}

/** Startup: find out what the user calls this workspace, by whichever of the two
 *  routes this page has (see the module note). No-op on the picker, which is not
 *  inside a workspace and has no single label to resolve. */
export async function loadWorkspaceDisplayName(): Promise<void> {
  if (IS_PICKER) return;
  try {
    if (WORKSPACE_ID !== null) {
      adoptWorkspaceDisplayName(await listWorkspaces());
      return;
    }
    // Direct engine port. The engine answers `null` when it has no gateway to
    // ask (a legacy no-gateway engine, the e2e harness, a coding agent's
    // frontend preview), and then the engine name is the only name there is.
    const label = await getWorkspaceLabel();
    if (label) workspaceDisplayName.value = label;
  } catch (e) {
    // Best-effort startup probe (frontend.md carve-out), deliberately not a
    // toast: the user did not ask for this, and the display falls back to the
    // engine's own name so nothing is blank or wrong-looking. It is the probe
    // that runs unasked; the switcher's own listing is the other adopter, and it
    // surfaces its failures in the list the user opened.
    //
    // Either route can land here, for its own reason. The gateway route only
    // runs when a gateway put us behind a slug, so reaching here means one that
    // WAS there has stopped answering (down mid-session, a 403, a dropped
    // connection). The engine route reaches here on an engine older than
    // `/api/v1/workspace-label`, which 404s it. Neither is a failure worth
    // showing: a workspace whose gateway is down has louder problems, and an
    // old engine is simply one that predates the label.
    console.warn('[workspace] display label unavailable; using the engine name', e);
  }
}
