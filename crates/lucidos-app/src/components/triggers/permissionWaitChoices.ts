import type { PermissionWait } from '../../store/types';

/** The Dropdown value of the Workspace default option: the trigger stores no
 *  permission wait and inherits `unattended_permission_wait`. */
export const WORKSPACE_DEFAULT_WAIT = '';

const PRESET_MINUTES = [5, 15, 30, 60];

function minutesLabel(minutes: number): string {
  return minutes === 1 ? 'Wait 1 minute' : `Wait ${minutes} minutes`;
}

/** The Dropdown value for a stored wait. */
export function permissionWaitValue(wait: PermissionWait | undefined): string {
  return wait === undefined ? WORKSPACE_DEFAULT_WAIT : String(wait);
}

/** The stored wait for a Dropdown value; `null` is the workspace default. */
export function permissionWaitFromValue(value: string): PermissionWait | null {
  if (value === WORKSPACE_DEFAULT_WAIT) return null;
  if (value === 'deny' || value === 'indefinite') return value;
  return Number(value);
}

/** The options the form offers. A stored wait no preset matches, such as one
 *  set through the CLI, joins the list so the form never drops it on save. */
export function permissionWaitOptions(current: string): { value: string; label: string }[] {
  const minutes = new Set(PRESET_MINUTES);
  const stored = permissionWaitFromValue(current);
  if (typeof stored === 'number') minutes.add(stored);
  return [
    { value: WORKSPACE_DEFAULT_WAIT, label: 'Workspace default' },
    { value: 'deny', label: 'Deny at once' },
    ...[...minutes].sort((a, b) => a - b).map(m => ({ value: String(m), label: minutesLabel(m) })),
    { value: 'indefinite', label: 'Wait until answered' },
  ];
}
