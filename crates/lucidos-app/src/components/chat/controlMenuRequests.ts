import { signal, useSignalEffect, type Signal } from '@preact/signals';
import { useRef } from 'preact/hooks';

export const codingAgentMenuOpenRequest = signal<string | null>(null);

export const rungFoldOpenRequest = signal<HTMLElement | null>(null);

export const codingAgentFoldOpenRequest = signal<HTMLElement | null>(null);

export const controlMenuCloseRequests = signal(0);

export function closeControlMenus(): void {
  controlMenuCloseRequests.value++;
}

export function useControlMenuCloseRequests(open: Signal<boolean>, close: () => void): void {
  const seen = useRef(controlMenuCloseRequests.peek());
  useSignalEffect(() => {
    const requests = controlMenuCloseRequests.value;
    if (requests === seen.current) return;
    seen.current = requests;
    if (open.peek()) close();
  });
}

export const codingAgentCommandsReady = signal(false);

export type ControlMenuKind = 'coding-agent' | 'rung';

export const controlMenuPressed = signal<ControlMenuKind | null>(null);

let stopWatching: (() => void) | null = null;

function dropControlMenuPress(): void {
  stopWatching?.();
  stopWatching = null;
  controlMenuPressed.value = null;
}

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

export function takeControlMenuPress(kind: ControlMenuKind): boolean {
  if (controlMenuPressed.peek() !== kind) return false;
  dropControlMenuPress();
  return true;
}
