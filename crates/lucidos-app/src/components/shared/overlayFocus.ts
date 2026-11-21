import { useLayoutEffect, useRef } from 'preact/hooks';
import type { RefObject } from 'preact';
import { overlayStack, type OverlayEntry } from '../../store/overlayStack';
import { focusedPane } from '../../store/store';
import { paneHolding, tabTargetWithin, visibleFocusables } from '../layout/paneFocus';
import { hasHoverPointer } from '../../utils/platform';

/** Keyboard focus for `<Overlay>`: where Tab goes while one is open, where
 *  focus lands when it opens, and where it returns when it closes. The shell
 *  behind an overlay is inert only to the pointer (`pointer-events`), so
 *  without this Tab walks straight into it. Two shapes:
 *
 *  - a **dialog** (a backdrop, or `aria-modal`) contains Tab. Focus outside its panel is pulled in,
 *    and Tab wraps at the panel's two ends.
 *  - an **anchored popover** wraps Tab only while focus is inside it. Tab from
 *    outside, typically its anchor or a combobox input, closes it and carries on.
 *
 *  Why two shapes and not one: ADR 0335, docs/adr/. */

/** What one Tab press does to one open overlay. */
export type OverlayTabAction =
  /** Move focus to `focusables[index]` and consume the key. */
  | { kind: 'focus'; index: number }
  /** Consume the key and leave focus where it is: a dialog with nothing to tab to. */
  | { kind: 'stay' }
  /** Close this popover and ask the overlay below, then the pane trap. */
  | { kind: 'dismiss' }
  /** Let the browser move focus, which stays inside the panel. */
  | { kind: 'native' };

/** Pure decision behind `handleOverlayTab`, for unit testing. `target` is
 *  what `tabTargetWithin` answered for the panel. */
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

/** Attributes `<Overlay>` puts on a dialog's panel. */
export function overlayModalAttrs(modal: boolean): Record<string, string> {
  return modal ? { [MODAL_ATTR]: '' } : {};
}

function panelFor(entry: OverlayEntry): HTMLElement | null {
  return document.querySelector<HTMLElement>(`[data-overlay-panel="${entry.id}"]`);
}

/** Route one Tab press through the open overlays, top first. Returns true when
 *  an overlay owned the key, and false when none is open or every one it met
 *  was a popover it closed. The caller then runs the pane trap. */
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

/** Whether closing an overlay may hand focus back to `opener`. Only when focus
 *  went down with the panel, and only when the overlay's own action left the
 *  focused-pane marker where the opener lives. A menu item that navigated to
 *  another pane has decided where the reader works next. */
export function shouldRestoreFocus(opts: {
  focusLost: boolean;
  openerUsable: boolean;
  openerPane: string | null;
  markerPane: string;
}): boolean {
  if (!opts.focusLost || !opts.openerUsable) return false;
  return opts.openerPane === null || opts.openerPane === opts.markerPane;
}

/** Focus-in on open for a dialog, and focus restore on close for any overlay.
 *  Keyboard devices only, since focusing a field on a phone raises its
 *  keyboard. `hasHoverPointer` is the same gate the choice-card seed uses. */
export function useOverlayFocus(
  open: boolean,
  panelRef: RefObject<HTMLElement>,
  modal: boolean,
  anchor: HTMLElement | null,
): void {
  // Captured during render, before any effect of the opening commit runs. A
  // child's layout effect may focus a field inside the panel first, and the
  // opener is whatever held focus before that. A click leaves no focus in
  // WebKit, so an anchored popover falls back to the control that opened it.
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

/** A dialog's own content usually focuses its main control. This covers the
 *  ones that do not, so Tab never starts from behind the dialog. */
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
