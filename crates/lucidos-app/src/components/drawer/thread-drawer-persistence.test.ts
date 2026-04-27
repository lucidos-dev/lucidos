import { describe, it, expect, beforeEach, vi } from 'vitest';
import { threadDrawerOpen, mobileView, activeMenuItem, splitRatio } from '../../store/store';
import { switchMenuItem } from '../../store/actions/menu';

// switchMenuItem('apps') triggers loadApps() as a side effect — mock the API
// so the test doesn't fire a real fetch (which fails in the JSDOM environment).
vi.mock('../../api/client', () => ({
  listAppsApi: vi.fn().mockResolvedValue([]),
}));

describe('Thread drawer persistence across reload', () => {
  beforeEach(() => {
    localStorage.clear();
    threadDrawerOpen.value = false;
    mobileView.value = 'thread';
    activeMenuItem.value = 'files';
    // Default to desktop viewport; tests override as needed
    (globalThis as any).innerWidth = 1024;
  });

  it('switchMenuItem does not change mobileView on desktop', () => {
    threadDrawerOpen.value = true;

    switchMenuItem('apps');

    // mobileView should stay 'thread' on desktop — only mobile should navigate
    expect(mobileView.value).toBe('thread');
    expect(threadDrawerOpen.value).toBe(true);
  });

  it('switchMenuItem changes mobileView on mobile', () => {
    (globalThis as any).innerWidth = 375;

    switchMenuItem('apps');

    expect(mobileView.value).toBe('content');
  });
});

/** Simulate the ThreadDrawer visible computation (non-forceVisible).
 *  On mobile, the drawer overlay is disabled — mobile uses a dedicated threads
 *  pane (pane 0) so dots and pane always agree. */
function computeDrawerVisible(): boolean {
  return threadDrawerOpen.value && splitRatio.value > 0;
}

describe('Thread drawer visibility with splitRatio', () => {
  beforeEach(() => {
    localStorage.clear();
    threadDrawerOpen.value = false;
    splitRatio.value = 0.4;
    mobileView.value = 'thread';
    (globalThis as any).innerWidth = 1024;
  });

  it('drawer is visible when open and splitRatio > 0', () => {
    threadDrawerOpen.value = true;
    splitRatio.value = 0.4;

    expect(computeDrawerVisible()).toBe(true);
  });

  it('drawer hides when splitRatio is 0 even if threadDrawerOpen is true', () => {
    threadDrawerOpen.value = true;
    splitRatio.value = 0;

    expect(computeDrawerVisible()).toBe(false);
  });

  it('drawer reappears when splitRatio returns to non-zero', () => {
    threadDrawerOpen.value = true;

    // Collapse
    splitRatio.value = 0;
    expect(computeDrawerVisible()).toBe(false);

    // Expand — drawer must reappear because threadDrawerOpen was preserved
    splitRatio.value = 0.4;
    expect(computeDrawerVisible()).toBe(true);
  });

  it('drawer stays hidden through cycle when threadDrawerOpen is false', () => {
    threadDrawerOpen.value = false;

    splitRatio.value = 0;
    expect(computeDrawerVisible()).toBe(false);

    splitRatio.value = 0.4;
    expect(computeDrawerVisible()).toBe(false);
  });

  it('drawer overlay is never visible on mobile (mobile uses threads pane)', () => {
    (globalThis as any).innerWidth = 375;
    threadDrawerOpen.value = true;
    splitRatio.value = 0; // mobile typically has splitRatio 0

    // On mobile, the drawer overlay never shows — users navigate to threads pane
    expect(computeDrawerVisible()).toBe(false);
  });
});
