/**
 * The menu drawer slides out from the edge its opener sits nearest, so the panel
 * reads as coming from under the button the user pressed.
 *
 * This exists because the mobile thread pane header puts its hamburger at the
 * row's TRAILING edge (mirroring the thread drawer toggle at the leading edge),
 * while the mobile content pane header keeps its own at the leading edge. One
 * fixed side would be wrong for one of them.
 *
 * Desktop is always `left`: the panel is positioned to emerge from the split
 * divider rather than from a viewport edge (`@media (min-width: 769px)` in
 * mobile.css), so an anchor sitting right of the viewport middle (a wide
 * Conversation side pushes the content header's hamburger there) must NOT flip
 * it. The `.drawer-right` CSS is scoped to the mobile breakpoint for the same
 * reason, belt and braces.
 */
import { beforeEach, describe, expect, it } from 'vitest';
import {
  drawerSide, drawerSideFor, openDrawer, forceCloseDrawer,
} from '../Drawer';

/** Minimal stand-in for the hamburger element: openDrawer only measures it. */
function anchorAt(left: number, width = 28): HTMLElement {
  return { getBoundingClientRect: () => ({ left, width }) } as unknown as HTMLElement;
}

beforeEach(() => {
  forceCloseDrawer();
  drawerSide.value = 'left';
  window.innerWidth = 1024;
});

describe('drawerSideFor', () => {
  it('follows the anchor across the viewport middle on mobile', () => {
    expect(drawerSideFor(20, 400, true)).toBe('left');
    expect(drawerSideFor(380, 400, true)).toBe('right');
  });

  it('treats the exact middle as left, so the default never flips on a tie', () => {
    expect(drawerSideFor(200, 400, true)).toBe('left');
  });

  it('is always left on desktop, even for a right-of-middle anchor', () => {
    expect(drawerSideFor(900, 1024, false)).toBe('left');
  });
});

describe('openDrawer', () => {
  it('records the right edge for a trailing-edge anchor on a mobile viewport', () => {
    window.innerWidth = 390;
    openDrawer(anchorAt(350));
    expect(drawerSide.value).toBe('right');
  });

  it('records the left edge for a leading-edge anchor on a mobile viewport', () => {
    window.innerWidth = 390;
    openDrawer(anchorAt(8));
    expect(drawerSide.value).toBe('left');
  });

  it('clears a previous right side when reopened from the leading edge', () => {
    window.innerWidth = 390;
    openDrawer(anchorAt(350));
    forceCloseDrawer();
    openDrawer(anchorAt(8));
    expect(drawerSide.value).toBe('left');
  });
});
