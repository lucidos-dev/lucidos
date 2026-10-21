import { signal } from '@preact/signals';

/** Set to the filter text to open the coding-agent menu from a typed "/". The
 *  menu consumes it. */
export const codingAgentMenuOpenRequest = signal<string | null>(null);

/** The ⋯ box a folded rung control opens its list against. The ⋯ menu row sets
 *  it, and the control, which stays mounted while folded, takes it. */
export const rungFoldOpenRequest = signal<HTMLElement | null>(null);

export type ControlMenuKind = 'coding-agent' | 'rung';

/** A press on a control menu's button before the menu's code arrived. The menu
 *  takes it as it mounts and opens as that press would have (ADR 0436). */
export const controlMenuPressed = signal<ControlMenuKind | null>(null);

let stopWatching: (() => void) | null = null;

function dropControlMenuPress(): void {
  stopWatching?.();
  stopWatching = null;
  controlMenuPressed.value = null;
}

/** Hold a press on `kind`'s placeholder `button`. Like `whenLoaded`, a second
 *  press on it takes the press back, and a press anywhere else drops it. */
export function pressControlMenu(kind: ControlMenuKind, button: Element): void {
  const takeBack = controlMenuPressed.peek() === kind;
  dropControlMenuPress();
  if (takeBack) return;
  controlMenuPressed.value = kind;
  const onPress = (e: PointerEvent) => {
    if (!button.contains(e.target as Node)) dropControlMenuPress();
  };
  document.addEventListener('pointerdown', onPress, true);
  stopWatching = () => document.removeEventListener('pointerdown', onPress, true);
}

/** Take the press held for `kind`'s menu. True when there was one. */
export function takeControlMenuPress(kind: ControlMenuKind): boolean {
  if (controlMenuPressed.peek() !== kind) return false;
  dropControlMenuPress();
  return true;
}
