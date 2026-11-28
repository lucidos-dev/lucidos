
import { workspaceDisplayName } from '../store';
import { listRemoteWorkspaces, listWorkspaces, type WorkspaceStatus } from '../../api/client/control';
import { getWorkspaceLabel } from '../../api/client';
import { IS_PICKER, WORKSPACE_ID, parseWorkspaceRoute } from '../../utils/basePath';

export function adoptWorkspaceDisplayName(list: readonly WorkspaceStatus[]): void {
  if (WORKSPACE_ID === null) return;
  const entry = list.find((w) => w.id === WORKSPACE_ID);
  if (entry) workspaceDisplayName.value = entry.name;
}

export async function loadWorkspaceDisplayName(): Promise<void> {
  if (IS_PICKER) return;
  try {
    if (WORKSPACE_ID !== null) {
      const route = parseWorkspaceRoute(WORKSPACE_ID);
      if (route?.gateway) {
        const listing = (await listRemoteWorkspaces()).find((g) => g.gateway === route.gateway);
        if (listing?.state === 'reachable') {
          const entry = listing.workspaces.find((w) => w.id === route.slug);
          if (entry) workspaceDisplayName.value = entry.name;
        }
      } else adoptWorkspaceDisplayName(await listWorkspaces());
      return;
    }
    const label = await getWorkspaceLabel();
    if (label) workspaceDisplayName.value = label;
  } catch (e) {
    console.warn('[workspace] display label unavailable; using the engine name', e);
  }
}
