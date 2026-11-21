/**
 * The menu drawer's open state, apart from the drawer itself. The store opens
 * and closes it (`store/actions/pane.ts`), and the store is in the entry chunk
 * while the drawer renders from the shell chunk (ADR 0288). Keeping the state
 * here is what lets the component stay out of first paint's critical path.
 */
import { signal } from '@preact/signals';
import { isMobile } from '../../utils/viewport';
import { isReducedMotion, scaledDurationMs } from '../../utils/motion';
import { lazyComponent, whenLoaded, type PendingOpen } from '../../utils/lazyComponent';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';

/** The drawer itself, fetched once the boot splash lifts (ADR 0436). It opens
 *  only once it is in memory, so the shell never reads as open with no panel. */
export const DrawerChunk = lazyComponent(() => import('./Drawer').then((m) => m.Drawer));
prefetchWhenIdle(DrawerChunk);

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

let pendingOpen: PendingOpen | null = null;

/** Open the drawer, resetting any stuck closing state. A shortcut passes no
 *  anchor, which clears the one a past button press left. Waits for the
 *  drawer's chunk, which is at once when warm. A second open takes a waiting
 *  one back, and so does a press away from the button that asked. */
export function openDrawer(anchor?: HTMLElement) {
  if (pendingOpen?.pending) {
    pendingOpen.cancel();
    return;
  }
  pendingOpen = whenLoaded(DrawerChunk, () => {
    drawerClosing.value = false;
    drawerOpen.value = true;
    drawerAnchor.value = anchor ?? null;
    if (anchor) {
      const rect = anchor.getBoundingClientRect();
      drawerSide.value = drawerSideFor(rect.left + rect.width / 2, window.innerWidth, isMobile());
    }
  }, anchor ?? null);
}

/** Immediately close the drawer without animation (e.g. pane switching). */
export function forceCloseDrawer() {
  pendingOpen?.cancel();
  drawerOpen.value = false;
  drawerClosing.value = false;
}

/** Close the drawer. Returns `false` when it is already closed or closing, so
 *  the dismiss hook keeps the paired click un-swallowed. `drawerOpen` normally
 *  stays `true` through the slide-out. Without that `false` the hook would eat
 *  a tap on a neighbor button (file-search, content actions) as a dismiss.
 *
 *  Reduced motion has no slide-out to wait for. The CSS drops the animation on
 *  `.drawer.closing`, and an element with no animation fires no `animationend`.
 *  So the close reads `isReducedMotion()`, the value the CSS keys on through
 *  `data-motion`, and closes at once. A scaled fallback timer covers the rest:
 *  motion turning reduced mid-slide, or a frame that never paints. A real close
 *  still returns `true` and the tap is still swallowed. */
export function closeDrawer(): boolean {
  if (!drawerOpen.value || drawerClosing.value) return false;
  if (isReducedMotion()) {
    forceCloseDrawer();
    return true;
  }
  drawerClosing.value = true;
  const close = ++closeGeneration;
  setTimeout(() => {
    if (close === closeGeneration && drawerClosing.value) forceCloseDrawer();
  }, scaledDurationMs(DRAWER_SLIDE_OUT_MS) + DRAWER_SLIDE_OUT_SLACK_MS);
  return true;
}

/** The `.drawer.closing` slide-out at 1x (`--duration-normal`, mobile.css). */
const DRAWER_SLIDE_OUT_MS = 200;
const DRAWER_SLIDE_OUT_SLACK_MS = 100;
/** Bumped per close, so a stale fallback cannot cut a later close short. */
let closeGeneration = 0;
