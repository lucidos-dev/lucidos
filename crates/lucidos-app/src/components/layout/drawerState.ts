import { signal } from '@preact/signals';
import { isMobile } from '../../utils/viewport';
import { isReducedMotion, scaledDurationMs } from '../../utils/motion';
import { lazyComponent, whenLoaded, type PendingOpen } from '../../utils/lazyComponent';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';

export const DrawerChunk = lazyComponent(() => import('./Drawer').then((m) => m.Drawer));
prefetchWhenIdle(DrawerChunk);

export const drawerOpen = signal(false);
export const drawerClosing = signal(false);
export const drawerAnchor = signal<HTMLElement | null>(null);

export type DrawerSide = 'left' | 'right';

export const drawerSide = signal<DrawerSide>('left');

export function drawerSideFor(anchorCenterX: number, viewportWidth: number, mobile: boolean): DrawerSide {
  if (!mobile) return 'left';
  return anchorCenterX > viewportWidth / 2 ? 'right' : 'left';
}

let pendingOpen: PendingOpen | null = null;

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

export function forceCloseDrawer() {
  pendingOpen?.cancel();
  drawerOpen.value = false;
  drawerClosing.value = false;
}

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

const DRAWER_SLIDE_OUT_MS = 200;
const DRAWER_SLIDE_OUT_SLACK_MS = 100;
let closeGeneration = 0;
