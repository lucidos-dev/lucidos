/**
 * The menu drawer must not depend on an animation that may not run.
 *
 * `closeDrawer()` normally only sets `drawerClosing`, and the panel's
 * `onAnimationEnd` handler is what clears `drawerOpen`. Under
 * `prefers-reduced-motion: reduce` the CSS drops the animation on
 * `.drawer.closing` (mobile.css), and an element with no animation fires no
 * `animationend`. So the drawer stayed open, and `<Overlay open>` held
 * `data-overlay-open` on `<html>`, which inerts the whole shell behind it.
 *
 * That media query is not scoped to a breakpoint, so desktop had no recovery
 * short of a reload. These tests dispatch no `animationend` at all and assert
 * synchronously: no event and no timer may stand between the call and the
 * closed state.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  closeDrawer, drawerClosing, drawerOpen, forceCloseDrawer, openDrawer,
} from '../Drawer';

/** Answer `prefers-reduced-motion` the way the OS setting would, and hand back
 *  the restore. Same stub shape as `chat/__tests__/scroll-to-top.test.ts`. */
function stubReducedMotion(reduce: boolean): () => void {
  const real = window.matchMedia;
  (window as any).matchMedia = (query: string) => ({
    matches: reduce && query.includes('prefers-reduced-motion'),
    addEventListener: () => {}, removeEventListener: () => {},
    addListener: () => {}, removeListener: () => {}, dispatchEvent: () => false,
  });
  return () => { (window as any).matchMedia = real; };
}

let restoreMatchMedia: (() => void) | null = null;

beforeEach(() => {
  forceCloseDrawer();
});

afterEach(() => {
  restoreMatchMedia?.();
  restoreMatchMedia = null;
  forceCloseDrawer();
});

describe('closing the menu drawer under reduced motion', () => {
  it('reaches the closed state with no animationend', () => {
    restoreMatchMedia = stubReducedMotion(true);
    openDrawer();

    expect(closeDrawer()).toBe(true);

    expect(drawerOpen.value).toBe(false);
    expect(drawerClosing.value).toBe(false);
  });

  it('leaves no stuck overlay for the rest of the session', () => {
    restoreMatchMedia = stubReducedMotion(true);
    openDrawer();
    closeDrawer();

    // A stuck `drawerOpen` keeps `<Overlay open>` mounted, which is what inerts
    // the thread list, the composer and every header control but the hamburger.
    expect(drawerOpen.value).toBe(false);
    // And the hamburger must reopen rather than toggle a panel that never left.
    openDrawer();
    expect(drawerOpen.value).toBe(true);
    expect(drawerClosing.value).toBe(false);
  });

  it('still reports the already-closed case as a no-op', () => {
    // The load-bearing `false`: the dismiss hook leaves the paired click
    // un-swallowed, so a tap on a neighbor button still reaches its handler.
    restoreMatchMedia = stubReducedMotion(true);

    expect(closeDrawer()).toBe(false);
    expect(drawerOpen.value).toBe(false);
  });
});

describe('closing the menu drawer with motion on', () => {
  it('waits for the slide-out rather than closing straight through', () => {
    restoreMatchMedia = stubReducedMotion(false);
    openDrawer();

    expect(closeDrawer()).toBe(true);

    expect(drawerClosing.value).toBe(true);
    expect(drawerOpen.value).toBe(true);
  });

  it('reports a second call mid-slide-out as a no-op', () => {
    restoreMatchMedia = stubReducedMotion(false);
    openDrawer();
    closeDrawer();

    expect(closeDrawer()).toBe(false);
  });
});
