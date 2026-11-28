import { useLayoutEffect, useRef } from 'preact/hooks';
import type { RefObject } from 'preact';
import { overlayStack, type OverlayEntry } from '../../store/overlayStack';
import { focusedPane } from '../../store/store';
import { paneHolding, tabTargetWithin, visibleFocusables } from '../layout/paneFocus';
import { hasHoverPointer } from '../../utils/platform';


export type OverlayTabAction =
  | { kind: 'focus'; index: number }
  | { kind: 'stay' }
  | { kind: 'dismiss' }
  | { kind: 'native' };

export function overlayTabAction(opts: {
  modal: boolean;
  activeInPanel: boolean;
  count: number;
  target: number | null;
}): OverlayTabAction {
  const { modal, activeInPanel, count, target } = opts;
  if (!activeInPanel && !modal) return { kind: 'dismiss' };
  if (count === 0) return { kind: 'stay' };
  return target === null ? { kind: 'native' } : { kind: 'focus', index: target };
}

const MODAL_ATTR = 'data-overlay-modal';

export function overlayModalAttrs(modal: boolean): Record<string, string> {
  return modal ? { [MODAL_ATTR]: '' } : {};
}

function panelFor(entry: OverlayEntry): HTMLElement | null {
  return document.querySelector<HTMLElement>(`[data-overlay-panel="${entry.id}"]`);
}

export function handleOverlayTab(e: KeyboardEvent): boolean {
  const active = document.activeElement as HTMLElement | null;
  const entries = overlayStack.value.filter((entry) => entry.hasPanel);
  for (let i = entries.length - 1; i >= 0; i--) {
    const panel = panelFor(entries[i]);
    if (!panel) continue;
    const modal = panel.hasAttribute(MODAL_ATTR);
    const activeInPanel = !!active && panel.contains(active);
    if (!activeInPanel && !modal) {
      entries[i].dismiss();
      continue;
    }
    const focusables = visibleFocusables(panel);
    const action = overlayTabAction({
      modal, activeInPanel, count: focusables.length,
      target: tabTargetWithin(panel, focusables, active, e.shiftKey),
    });
    if (action.kind === 'native') return true;
    e.preventDefault();
    if (action.kind === 'focus') focusables[action.index].focus({ preventScroll: true });
    return true;
  }
  return false;
}

export function shouldRestoreFocus(opts: {
  focusLost: boolean;
  openerUsable: boolean;
  openerPane: string | null;
  markerPane: string;
}): boolean {
  if (!opts.focusLost || !opts.openerUsable) return false;
  return opts.openerPane === null || opts.openerPane === opts.markerPane;
}

export function useOverlayFocus(
  open: boolean,
  panelRef: RefObject<HTMLElement>,
  modal: boolean,
  anchor: HTMLElement | null,
): void {
  const openerRef = useRef<Element | null>(null);
  const wasOpenRef = useRef(false);
  if (open && !wasOpenRef.current && typeof document !== 'undefined') {
    const active = document.activeElement;
    openerRef.current = active && active !== document.body ? active : anchor;
  }
  wasOpenRef.current = open;

  useLayoutEffect(() => {
    if (!open || !hasHoverPointer()) return;
    const panel = panelRef.current;
    const frame = modal ? requestAnimationFrame(() => focusIntoDialog(panel)) : 0;
    return () => {
      cancelAnimationFrame(frame);
      restoreOpenerFocus(openerRef.current, panel);
    };
  }, [open]);
}

function focusIntoDialog(panel: HTMLElement | null): void {
  if (!panel || panel.contains(document.activeElement)) return;
  visibleFocusables(panel)[0]?.focus({ preventScroll: true });
}

function restoreOpenerFocus(opener: Element | null, panel: HTMLElement | null): void {
  const active = document.activeElement;
  const usable = opener instanceof HTMLElement && opener.isConnected && opener !== document.body
    && opener.getClientRects().length > 0 && !panel?.contains(opener);
  if (!shouldRestoreFocus({
    focusLost: !active || active === document.body || !!panel?.contains(active),
    openerUsable: usable,
    openerPane: usable ? paneHolding(opener) : null,
    markerPane: focusedPane.value,
  })) return;
  (opener as HTMLElement).focus({ preventScroll: true });
}
