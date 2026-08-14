/**
 * The one reading of a workspace's health, shared by every surface that draws a
 * status dot for one.
 *
 * The gateway reports `health` plus a `last_error`, and those two do not map
 * onto four distinct things by themselves: a workspace that was never started
 * answers `unhealthy` with "not started", which is a calm idle rather than a
 * failure. Collapsing that here is what keeps the *workspace picker* and the
 * *in-app workspace switcher* from disagreeing about what a red dot means.
 *
 * A leaf module on purpose. The picker is lazily loaded into its own chunk
 * (`main.tsx`), so importing this out of `WorkspacePicker.tsx` would pull that
 * whole chunk into the app bundle for the sake of one predicate.
 */

import type { WorkspaceStatus } from '../api/client/control';

export type WorkspaceState = 'healthy' | 'booting' | 'stopped' | 'unhealthy';

export function workspaceState(w: WorkspaceStatus): WorkspaceState {
  if (w.health === 'healthy') return 'healthy';
  if (w.health === 'booting') return 'booting';
  return w.last_error === 'not started' ? 'stopped' : 'unhealthy';
}

export const WORKSPACE_STATE_LABEL: Record<WorkspaceState, string> = {
  healthy: 'Ready',
  booting: 'Starting…',
  stopped: 'Stopped',
  unhealthy: 'Unhealthy',
};

/** What the dot says out loud: the engine's own error when there is one, the
 *  state word otherwise. Both surfaces put this on `data-tooltip` AND on
 *  `aria-label`, since a tooltip is hover-only and the error would otherwise be
 *  unreachable. */
export function workspaceStateLabel(w: WorkspaceStatus): string {
  return w.last_error || WORKSPACE_STATE_LABEL[workspaceState(w)];
}

/** The same words again, but only when the row owes the user an EXPLANATION
 *  rather than a label: the state that is a fault, and nothing else.
 *
 *  `unhealthy` is the one, and it is the one the picker draws in red. The other
 *  three are already told by the row itself: a healthy workspace is the ordinary
 *  case, `booting` pulses and offers Stop, and `stopped` is a calm idle the play
 *  button explains (it is also the state a never-started workspace lands in, see
 *  `workspaceState`, which is exactly why "unhealthy with an error" and "not
 *  started" must not share a sentence).
 *
 *  Pure and separate from the rendering so the four states are unit-testable,
 *  and so no surface has to re-derive which one is the fault. */
export function workspaceFaultNote(w: WorkspaceStatus): string | null {
  return workspaceState(w) === 'unhealthy' ? workspaceStateLabel(w) : null;
}
