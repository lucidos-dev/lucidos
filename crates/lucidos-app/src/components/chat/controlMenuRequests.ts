import { signal, useSignalEffect, type Signal } from '@preact/signals';
import { useRef } from 'preact/hooks';

/** Set to the filter text to open the coding-agent menu from a typed "/". The
 *  menu consumes it. */
export const codingAgentMenuOpenRequest = signal<string | null>(null);

/** The ⋯ box a folded rung control opens its list against. The ⋯ menu row sets
 *  it, and the control, which stays mounted while folded, takes it. */
export const rungFoldOpenRequest = signal<HTMLElement | null>(null);

/** The same for a folded coding-agent menu. */
export const codingAgentFoldOpenRequest = signal<HTMLElement | null>(null);

/** Counts requests to shut an open control menu. A panel a ⋯ row opened hangs
 *  from the ⋯ and ignores a press on it, so the ⋯ opening asks here. */
export const controlMenuCloseRequests = signal(0);

export function closeControlMenus(): void {
  controlMenuCloseRequests.value++;
}

/** Close the menu on each request made after it mounted. */
export function useControlMenuCloseRequests(open: Signal<boolean>, close: () => void): void {
  const seen = useRef(controlMenuCloseRequests.peek());
  useSignalEffect(() => {
    const requests = controlMenuCloseRequests.value;
    if (requests === seen.current) return;
    seen.current = requests;
    if (open.peek()) close();
  });
}

/** Whether the mounted coding-agent menu has a command list to open. The menu
 *  writes it, so its ⋯ row can dim as the standing button does. */
export const codingAgentCommandsReady = signal(false);

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
