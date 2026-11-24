
import { signal } from '@preact/signals';

export interface OverlayEntry {
  id: string;
  dismiss: () => void;
  hasPanel: boolean;
}

export const overlayStack = signal<readonly OverlayEntry[]>([]);

export function pushOverlay(entry: OverlayEntry): void {
  const without = overlayStack.value.filter((e) => e.id !== entry.id);
  overlayStack.value = [...without, entry];
}

export function removeOverlay(id: string): void {
  const next = overlayStack.value.filter((e) => e.id !== id);
  if (next.length !== overlayStack.value.length) overlayStack.value = next;
}

export function topOverlay(): OverlayEntry | null {
  const s = overlayStack.value;
  return s.length > 0 ? s[s.length - 1] : null;
}

export function topPanelOverlay(): OverlayEntry | null {
  const s = overlayStack.value;
  for (let i = s.length - 1; i >= 0; i--) {
    if (s[i].hasPanel) return s[i];
  }
  return null;
}

export const MAX_MODAL_STACK_DEPTH = 49;

export function overlayStackDepth(stack: readonly OverlayEntry[], id: string): number {
  const i = stack.findIndex((e) => e.id === id);
  return Math.min(i === -1 ? stack.length : i, MAX_MODAL_STACK_DEPTH);
}

export function dismissTopOverlay(): boolean {
  const top = topOverlay();
  if (!top) return false;
  top.dismiss();
  return true;
}

export function _resetOverlayStackForTesting(): void {
  overlayStack.value = [];
}
