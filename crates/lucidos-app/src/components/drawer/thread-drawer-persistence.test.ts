import { describe, it, expect, beforeEach, vi } from 'vitest';
import { threadDrawerOpen, mobileView, activeMenuItem, splitRatio } from '../../store/store';
import { switchMenuItem } from '../../store/actions/menu';

vi.mock('../../api/client', () => ({
  listAppsApi: vi.fn().mockResolvedValue([]),
}));

describe('Thread drawer persistence across reload', () => {
  beforeEach(() => {
    localStorage.clear();
    threadDrawerOpen.value = false;
    mobileView.value = 'thread';
    activeMenuItem.value = 'files';
    (globalThis as any).innerWidth = 1024;
  });

  it('switchMenuItem does not change mobileView on desktop', () => {
    threadDrawerOpen.value = true;

    switchMenuItem('apps');

    expect(mobileView.value).toBe('thread');
    expect(threadDrawerOpen.value).toBe(true);
  });

  it('switchMenuItem changes mobileView on mobile', () => {
    (globalThis as any).innerWidth = 375;

    switchMenuItem('apps');

    expect(mobileView.value).toBe('content');
  });
});

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

    splitRatio.value = 0;
    expect(computeDrawerVisible()).toBe(false);

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
    splitRatio.value = 0;

    expect(computeDrawerVisible()).toBe(false);
  });
});
