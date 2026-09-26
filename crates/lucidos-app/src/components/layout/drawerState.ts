/**
 * The menu drawer's open state, apart from the drawer itself. The store opens
 * and closes it (`store/actions/pane.ts`), and the store is in the entry chunk
 * while the drawer renders from the shell chunk (ADR 0288). Keeping the state
 * here is what lets the component stay out of first paint's critical path.
 */
import { signal } from '@preact/signals';
import { isMobile } from '../../utils/viewport';

export const drawerOpen = signal(false);
export const drawerClosing = signal(false);
/** Hamburger button that opened the drawer. Several `.hamburger-panel` buttons
 *  exist: a per-layout copy, plus one per mobile pane header. Only the one the
 *  user pressed fires openDrawer(). So this captures the right element for the
 *  dismiss hook's anchor exemption. */
export const drawerAnchor = signal<HTMLElement | null>(null);

export type DrawerSide = 'left' | 'right';

/** Which edge the drawer slides out from. */
export const drawerSide = signal<DrawerSide>('left');

/** The drawer emerges from under the button that opened it. The mobile thread
 *  pane header keeps its hamburger at the row's trailing edge, mirroring the
 *  thread drawer toggle at the leading edge. A panel sliding in from the far
 *  side of the screen would read as unrelated to the tap.
 *
 *  Desktop is always `left`. Its single hamburger sits at the content pane's
 *  leading edge, and the panel emerges from the split divider, not from a
 *  viewport edge. So the anchor's absolute x says nothing useful there. Pure
 *  so the rule is testable without a DOM. */
export function drawerSideFor(anchorCenterX: number, viewportWidth: number, mobile: boolean): DrawerSide {
  if (!mobile) return 'left';
  return anchorCenterX > viewportWidth / 2 ? 'right' : 'left';
}

/** Open the drawer, resetting any stuck closing state */
export function openDrawer(anchor?: HTMLElement) {
  drawerClosing.value = false;
  drawerOpen.value = true;
  if (anchor) {
    drawerAnchor.value = anchor;
    const rect = anchor.getBoundingClientRect();
    drawerSide.value = drawerSideFor(rect.left + rect.width / 2, window.innerWidth, isMobile());
  }
}

/** Immediately close the drawer without animation (e.g. pane switching). */
export function forceCloseDrawer() {
  drawerOpen.value = false;
  drawerClosing.value = false;
}
