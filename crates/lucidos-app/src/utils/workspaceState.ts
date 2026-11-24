
import type { ListedWorkspace } from '../api/client/control';

export type WorkspaceState = 'healthy' | 'booting' | 'stopped' | 'unhealthy';

export function workspaceState(w: ListedWorkspace): WorkspaceState {
  if (w.health === 'healthy') return 'healthy';
  if (w.health === 'booting' || w.health === 'restoring') return 'booting';
  return w.last_error === 'not started' ? 'stopped' : 'unhealthy';
}

export const WORKSPACE_STATE_LABEL: Record<WorkspaceState, string> = {
  healthy: 'Ready',
  booting: 'Starting…',
  stopped: 'Stopped',
  unhealthy: 'Unhealthy',
};

export function workspaceStateLabel(w: ListedWorkspace): string {
  if (w.health === 'restoring') return w.last_error || 'Restoring…';
  return w.last_error || WORKSPACE_STATE_LABEL[workspaceState(w)];
}

export function workspaceFaultNote(w: ListedWorkspace): string | null {
  return workspaceState(w) === 'unhealthy' ? workspaceStateLabel(w) : null;
}
