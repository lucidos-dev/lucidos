import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { readWindowItem, removeWindowItem, writeWindowItem } from '../../utils/windowStorage';
import {
  mobileView, threadDrawerOpen, threadDrawerWidth, splitRatio, focusedPane,
  DEFAULT_DRAWER_WIDTH, THREAD_DRAWER_WIDTH_KEY,
  MOBILE_VIEWS, PANE_INDEX, setMobileView, getInitialMobileView, type MobileView,
} from '../store';
import { drawerOpen } from '../../components/layout/drawerState';
import { DEFAULT_SPLIT_RATIO, KEYBOARD_RESIZE_STEP_PX } from '../../components/layout/splitHelpers';
import { drawnDrawerWidth, drawnSplitRatio, layoutGeometry } from '../../components/layout/drawnLayout';
import { dashboardRailShown } from '../dashboardRail';
import { dividerBandPx, minDrawerWidth, minThreadPanePx, minContentPanePx, paneFloors } from '../paneMinimums';
import {
  navigateToPane, checkPaneConsistency, toggleThreads, focusPane, revealContentPane, revealThreadPane,
  toggleThreadPane, toggleContentPane, stepThreadPaneWidth, stepThreadDrawerWidth, resetPaneLayout,
  toggleSplitFromDivider,
} from './pane';


function resetState(view: MobileView = 'thread') {
  mobileView.value = view;
  threadDrawerOpen.value = false;
  drawerOpen.value = false;
  focusedPane.value = 'thread';
}

describe('navigateToPane', () => {
  beforeEach(() => resetState());

  it('sets mobileView to the target pane', () => {
    for (const view of MOBILE_VIEWS) {
      navigateToPane(view);
      expect(mobileView.value).toBe(view);
    }
  });

  it('closes thread drawer when navigating away from thread', () => {
    mobileView.value = 'thread';
    threadDrawerOpen.value = true;
    navigateToPane('content');
    expect(threadDrawerOpen.value).toBe(false);
  });

  it('keeps thread drawer open when navigating to thread pane', () => {
    mobileView.value = 'thread';
    threadDrawerOpen.value = true;
    navigateToPane('thread');
    expect(threadDrawerOpen.value).toBe(true);
  });

  it('closes hamburger drawer when navigating', () => {
    drawerOpen.value = true;
    navigateToPane('threads');
    expect(drawerOpen.value).toBe(false);
  });

  it('closes both drawers when navigating', () => {
    mobileView.value = 'thread';
    threadDrawerOpen.value = true;
    drawerOpen.value = true;
    navigateToPane('content');
    expect(threadDrawerOpen.value).toBe(false);
    expect(drawerOpen.value).toBe(false);
    expect(mobileView.value).toBe('content');
  });

  it('navigating to current pane is idempotent (except drawer close)', () => {
    for (const view of MOBILE_VIEWS) {
      mobileView.value = view;
      navigateToPane(view);
      expect(mobileView.value).toBe(view);
    }
  });

  it('enforces the invariant from every starting state', () => {
    const drawerStates = [false, true];
    for (const from of MOBILE_VIEWS) {
      for (const to of MOBILE_VIEWS) {
        for (const drawerStart of drawerStates) {
          resetState(from);
          threadDrawerOpen.value = drawerStart;
          drawerOpen.value = from === 'content';

          navigateToPane(to);

          const error = checkPaneConsistency();
          expect(error, `${from}(drawer=${drawerStart}) → ${to}`).toBeNull();
          const expectedDrawer = to === 'thread' ? drawerStart : false;
          expect(threadDrawerOpen.value).toBe(expectedDrawer);
          expect(drawerOpen.value).toBe(false);
          expect(mobileView.value).toBe(to);
        }
      }
    }
  });
});

describe('checkPaneConsistency', () => {
  beforeEach(() => resetState());

  it('returns null for valid state: thread pane, drawer closed', () => {
    mobileView.value = 'thread';
    threadDrawerOpen.value = false;
    expect(checkPaneConsistency()).toBeNull();
  });

  it('returns null for valid state: thread pane, drawer open', () => {
    mobileView.value = 'thread';
    threadDrawerOpen.value = true;
    expect(checkPaneConsistency()).toBeNull();
  });

  it('returns null for valid state: content pane', () => {
    mobileView.value = 'content';
    threadDrawerOpen.value = false;
    expect(checkPaneConsistency()).toBeNull();
  });

  it('detects thread drawer open on non-thread pane', () => {
    mobileView.value = 'content';
    threadDrawerOpen.value = true;
    expect(checkPaneConsistency()).not.toBeNull();
  });

  it('detects thread drawer open on threads pane', () => {
    mobileView.value = 'threads';
    threadDrawerOpen.value = true;
    expect(checkPaneConsistency()).not.toBeNull();
  });
});

describe('PANE_INDEX consistency', () => {
  it('indices are contiguous starting from 0 and unique', () => {
    const indices = new Set(MOBILE_VIEWS.map(v => PANE_INDEX[v]));
    expect(indices.size).toBe(MOBILE_VIEWS.length);
    for (let i = 0; i < MOBILE_VIEWS.length; i++) {
      expect(PANE_INDEX[MOBILE_VIEWS[i]]).toBe(i);
    }
  });
});

describe('toggleThreads', () => {
  beforeEach(() => resetState());

  it('on mobile: navigates to threads pane', () => {
    (globalThis as any).innerWidth = 375;
    mobileView.value = 'thread';
    toggleThreads();
    expect(mobileView.value).toBe('threads');
    expect(threadDrawerOpen.value).toBe(false);
  });

  it('on mobile: closes thread drawer if it was somehow open', () => {
    (globalThis as any).innerWidth = 375;
    mobileView.value = 'thread';
    threadDrawerOpen.value = true;
    toggleThreads();
    expect(mobileView.value).toBe('threads');
    expect(threadDrawerOpen.value).toBe(false);
  });

  it('on mobile: pane consistency holds after toggleThreads', () => {
    (globalThis as any).innerWidth = 375;
    mobileView.value = 'thread';
    toggleThreads();
    expect(checkPaneConsistency()).toBeNull();
  });

  it('on desktop: hidden drawer → shows it without changing focus', () => {
    (globalThis as any).innerWidth = 1024;
    threadDrawerOpen.value = false;
    focusedPane.value = 'thread';
    toggleThreads();
    expect(threadDrawerOpen.value).toBe(true);
    expect(focusedPane.value).toBe('thread');
    expect(mobileView.value).toBe('thread');
  });

  it('on desktop: visible drawer → hides it (no focus stage)', () => {
    (globalThis as any).innerWidth = 1024;
    threadDrawerOpen.value = true;
    focusedPane.value = 'thread';
    toggleThreads();
    expect(threadDrawerOpen.value).toBe(false);
    expect(focusedPane.value).toBe('thread');
  });

  it('on desktop: hiding a focused drawer drops focus to the thread pane', () => {
    (globalThis as any).innerWidth = 1024;
    threadDrawerOpen.value = true;
    focusedPane.value = 'drawer';
    toggleThreads();
    expect(threadDrawerOpen.value).toBe(false);
    expect(focusedPane.value).toBe('thread');
  });
});


function fakeEl(offsetWidth: number) {
  return { offsetWidth, classList: { add: () => {}, remove: () => {} } } as unknown as Element;
}

function mockLayoutWidths({ splitLayout, contentRow }: { splitLayout?: number; contentRow?: number }) {
  vi.spyOn(document, 'querySelector').mockImplementation((sel: string) => {
    if (sel === '.split-layout' && splitLayout !== undefined) return fakeEl(splitLayout);
    if (sel === '.content-row' && contentRow !== undefined) return fakeEl(contentRow);
    return null;
  });
}

describe('toggleThreadPane / toggleContentPane (two-stage focus → hide)', () => {
  beforeEach(() => {
    resetState();
    (globalThis as any).innerWidth = 1024;
    splitRatio.value = 0.5;
  });
  afterEach(() => vi.restoreAllMocks());

  it('desktop thread pane: already focused → collapses (focus follows to content), then restores + refocuses', () => {
    toggleThreadPane();
    expect(splitRatio.value).toBe(0);
    expect(focusedPane.value).toBe('content');
    toggleThreadPane();
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(focusedPane.value).toBe('thread');
  });

  it('desktop thread pane: visible-but-unfocused → focuses first, hides only on the next press', () => {
    focusedPane.value = 'content';
    toggleThreadPane();
    expect(splitRatio.value).toBe(0.5);
    expect(focusedPane.value).toBe('thread');
    toggleThreadPane();
    expect(splitRatio.value).toBe(0);
    expect(focusedPane.value).toBe('content');
  });

  it('desktop content pane: focuses first, then collapses, then restores + refocuses', () => {
    toggleContentPane();
    expect(splitRatio.value).toBe(0.5);
    expect(focusedPane.value).toBe('content');
    toggleContentPane();
    expect(splitRatio.value).toBe(1);
    expect(focusedPane.value).toBe('thread');
    toggleContentPane();
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(focusedPane.value).toBe('content');
  });

  it('mobile: navigates to the pane instead of touching the split ratio or focus', () => {
    (globalThis as any).innerWidth = 375;
    toggleThreadPane();
    expect(mobileView.value).toBe('thread');
    toggleContentPane();
    expect(mobileView.value).toBe('content');
    expect(splitRatio.value).toBe(0.5);
    expect(focusedPane.value).toBe('thread');
    expect(checkPaneConsistency()).toBeNull();
  });
});

describe('toggleSplitFromDivider (divider double-click)', () => {
  beforeEach(() => {
    resetState();
    (globalThis as any).innerWidth = 1024;
    splitRatio.value = 0.5;
  });
  afterEach(() => vi.restoreAllMocks());

  it('collapsing the thread pane moves the marker off it, so Tab still works', () => {
    toggleSplitFromDivider();
    expect(splitRatio.value).toBe(0);
    expect(focusedPane.value).toBe('content');
  });

  it('collapsing from the drawer moves the marker too, since the drawer hides with it', () => {
    focusedPane.value = 'drawer';
    toggleSplitFromDivider();
    expect(focusedPane.value).toBe('content');
  });

  it('expanding a collapsed split restores the default ratio and leaves the marker', () => {
    splitRatio.value = 0;
    focusedPane.value = 'content';
    toggleSplitFromDivider();
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(focusedPane.value).toBe('content');
  });
});

describe('a Canvas that yielded for room: every way to show it shows it', () => {
  const floors = paneFloors();
  const layoutAt = (px: number) => ({ windowPx: px, floors, dividerPx: dividerBandPx() });
  const narrow = floors.minThreadPx + floors.minContentPx - 100;
  const railWasShown = dashboardRailShown.value;

  beforeEach(() => {
    resetState();
    (globalThis as any).innerWidth = 1024;
    splitRatio.value = 0.5;
    dashboardRailShown.value = false;
    layoutGeometry.value = layoutAt(narrow);
  });
  afterEach(() => {
    layoutGeometry.value = null;
    dashboardRailShown.value = railWasShown;
    vi.restoreAllMocks();
  });

  it('is drawn collapsed while the stored ratio stays put', () => {
    expect(drawnSplitRatio.value).toBe(1);
    expect(splitRatio.value).toBe(0.5);
  });

  it('the Canvas toggle swaps the panes, and its next press swaps back', () => {
    toggleContentPane();
    expect(splitRatio.value).toBe(0);
    expect(drawnSplitRatio.value).toBe(0);
    expect(focusedPane.value).toBe('content');
    toggleContentPane();
    expect(drawnSplitRatio.value).toBe(1);
    expect(focusedPane.value).toBe('thread');
  });

  it('a content navigation swaps the panes rather than restoring a split that cannot fit', () => {
    revealContentPane();
    expect(drawnSplitRatio.value).toBe(0);
  });

  it('the divider double-click shows the Canvas', () => {
    toggleSplitFromDivider();
    expect(drawnSplitRatio.value).toBe(0);
  });

  it('a split that has room again restores the default split, as before', () => {
    layoutGeometry.value = layoutAt(narrow + 400);
    splitRatio.value = 1;
    toggleContentPane();
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
  });
});

describe('focusPane', () => {
  beforeEach(() => resetState());
  afterEach(() => vi.restoreAllMocks());

  it('desktop: sets the focused pane', () => {
    (globalThis as any).innerWidth = 1024;
    focusPane('content');
    expect(focusedPane.value).toBe('content');
    focusPane('drawer');
    expect(focusedPane.value).toBe('drawer');
  });

  it('mobile: is a no-op (panes are navigated, not focused)', () => {
    (globalThis as any).innerWidth = 375;
    focusPane('content');
    expect(focusedPane.value).toBe('thread');
  });
});

describe('revealContentPane', () => {
  beforeEach(() => {
    resetState();
    splitRatio.value = 0.5;
  });
  afterEach(() => vi.restoreAllMocks());

  it('desktop: activates the Content pane group (signal-only)', () => {
    (globalThis as any).innerWidth = 1024;
    revealContentPane();
    expect(focusedPane.value).toBe('content');
  });

  it('desktop: re-expands a collapsed split (Threads group maximized)', () => {
    (globalThis as any).innerWidth = 1024;
    splitRatio.value = 1;
    revealContentPane();
    expect(focusedPane.value).toBe('content');
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
  });

  it('desktop: leaves an open split untouched (only focus moves)', () => {
    (globalThis as any).innerWidth = 1024;
    splitRatio.value = 0.5;
    revealContentPane();
    expect(splitRatio.value).toBe(0.5);
  });

  it('desktop: idempotent when already focused on content', () => {
    (globalThis as any).innerWidth = 1024;
    focusedPane.value = 'content';
    revealContentPane();
    expect(focusedPane.value).toBe('content');
  });

  it('mobile: navigates to the content pane and never touches focusedPane', () => {
    (globalThis as any).innerWidth = 375;
    revealContentPane();
    expect(mobileView.value).toBe('content');
    expect(focusedPane.value).toBe('thread');
  });
});

describe('revealThreadPane', () => {
  beforeEach(() => {
    resetState();
    splitRatio.value = 0.5;
  });
  afterEach(() => vi.restoreAllMocks());

  it('desktop: re-activates the Threads pane group from the content group', () => {
    (globalThis as any).innerWidth = 1024;
    focusedPane.value = 'content';
    revealThreadPane();
    expect(focusedPane.value).toBe('thread');
  });

  it('desktop: leaves an existing drawer focus alone (no cross-group switch)', () => {
    (globalThis as any).innerWidth = 1024;
    focusedPane.value = 'drawer';
    revealThreadPane();
    expect(focusedPane.value).toBe('drawer');
  });

  it('desktop: idempotent when already focused on the thread pane', () => {
    (globalThis as any).innerWidth = 1024;
    focusedPane.value = 'thread';
    revealThreadPane();
    expect(focusedPane.value).toBe('thread');
  });

  it('desktop: leaves an open split untouched (only focus moves)', () => {
    (globalThis as any).innerWidth = 1024;
    focusedPane.value = 'content';
    splitRatio.value = 0.5;
    revealThreadPane();
    expect(splitRatio.value).toBe(0.5);
  });

  it('desktop: re-expands a collapsed split (Content group maximized)', () => {
    (globalThis as any).innerWidth = 1024;
    focusedPane.value = 'content';
    splitRatio.value = 0;
    revealThreadPane();
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(focusedPane.value).toBe('thread');
  });

  it('desktop: a re-expand overrides a stale drawer focus', () => {
    (globalThis as any).innerWidth = 1024;
    focusedPane.value = 'drawer';
    splitRatio.value = 0;
    revealThreadPane();
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(focusedPane.value).toBe('thread');
  });

  it('mobile: navigates to the thread pane and never touches focusedPane', () => {
    (globalThis as any).innerWidth = 375;
    mobileView.value = 'content';
    revealThreadPane();
    expect(mobileView.value).toBe('thread');
    expect(focusedPane.value).toBe('thread');
  });

  it('mobile: keeps the thread drawer open when navigating to the thread pane', () => {
    (globalThis as any).innerWidth = 375;
    mobileView.value = 'thread';
    threadDrawerOpen.value = true;
    revealThreadPane();
    expect(mobileView.value).toBe('thread');
    expect(threadDrawerOpen.value).toBe(true);
  });
});

describe('stepThreadPaneWidth', () => {
  const TOTAL = 1000;

  beforeEach(() => {
    resetState();
    (globalThis as any).innerWidth = 1024;
    splitRatio.value = 0.5;
  });
  afterEach(() => vi.restoreAllMocks());

  it('moves the split divider by one step in each direction', () => {
    mockLayoutWidths({ splitLayout: TOTAL });
    stepThreadPaneWidth(1);
    expect(splitRatio.value).toBe((500 + KEYBOARD_RESIZE_STEP_PX) / TOTAL);
    stepThreadPaneWidth(-1);
    expect(splitRatio.value).toBe(0.5);
  });

  it('no-ops without a mounted split layout', () => {
    stepThreadPaneWidth(1);
    expect(splitRatio.value).toBe(0.5);
  });

  it('no-ops on mobile', () => {
    (globalThis as any).innerWidth = 375;
    mockLayoutWidths({ splitLayout: TOTAL });
    stepThreadPaneWidth(1);
    expect(splitRatio.value).toBe(0.5);
  });
});

describe('stepThreadDrawerWidth', () => {
  const ROW = 1600;
  const MAX = ROW - minThreadPanePx() - minContentPanePx();

  beforeEach(() => {
    resetState();
    (globalThis as any).innerWidth = 1024;
    threadDrawerOpen.value = true;
    splitRatio.value = 0.5;
    threadDrawerWidth.value = 400;
    mockLayoutWidths({ contentRow: ROW });
  });
  afterEach(() => vi.restoreAllMocks());

  it('steps the drawer width and persists it', () => {
    stepThreadDrawerWidth(1);
    expect(threadDrawerWidth.value).toBe(400 + KEYBOARD_RESIZE_STEP_PX);
    expect(localStorage.getItem(THREAD_DRAWER_WIDTH_KEY)).toBe(String(400 + KEYBOARD_RESIZE_STEP_PX));
    stepThreadDrawerWidth(-1);
    expect(threadDrawerWidth.value).toBe(400);
  });

  it('clamps at the drawer minimum instead of closing', () => {
    const min = minDrawerWidth();
    threadDrawerWidth.value = min + 10;
    stepThreadDrawerWidth(-1);
    expect(threadDrawerWidth.value).toBe(min);
    stepThreadDrawerWidth(-1);
    expect(threadDrawerWidth.value).toBe(min);
  });

  it('clamps so the visible split panes keep their minimum widths', () => {
    threadDrawerWidth.value = MAX - 10;
    stepThreadDrawerWidth(1);
    expect(threadDrawerWidth.value).toBe(MAX);
  });

  it('steps from the width drawn, which a smaller window holds below the stored one', () => {
    const floors = paneFloors();
    dashboardRailShown.value = false;
    threadDrawerWidth.value = 900;
    layoutGeometry.value = { windowPx: floors.minThreadPx + dividerBandPx() + 450, floors, dividerPx: dividerBandPx() };
    try {
      const drawnPx = drawnDrawerWidth.value;
      expect(drawnPx).toBeLessThan(900);
      stepThreadDrawerWidth(-1);
      expect(threadDrawerWidth.value).toBe(drawnPx - KEYBOARD_RESIZE_STEP_PX);
    } finally {
      layoutGeometry.value = null;
    }
  });

  it('no-ops while the drawer is hidden (closed, or thread pane collapsed)', () => {
    threadDrawerOpen.value = false;
    stepThreadDrawerWidth(1);
    expect(threadDrawerWidth.value).toBe(400);

    threadDrawerOpen.value = true;
    splitRatio.value = 0;
    stepThreadDrawerWidth(1);
    expect(threadDrawerWidth.value).toBe(400);
  });
});

describe('resetPaneLayout', () => {
  beforeEach(() => {
    resetState();
    (globalThis as any).innerWidth = 1024;
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('restores the default split ratio and drawer width, persisting the width', () => {
    const width = Math.max(DEFAULT_DRAWER_WIDTH, minDrawerWidth());
    splitRatio.value = 0.8;
    threadDrawerWidth.value = 500;
    resetPaneLayout();
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(threadDrawerWidth.value).toBe(width);
    expect(localStorage.getItem(THREAD_DRAWER_WIDTH_KEY)).toBe(String(width));
  });

  it('never resets BELOW the drawer floor, which the default falls under', () => {
    vi.stubGlobal('getComputedStyle', () => ({ fontSize: '32px', getPropertyValue: () => '' }));
    const floor = minDrawerWidth();
    expect(floor, 'a 32px root must put the floor above the default').toBeGreaterThan(DEFAULT_DRAWER_WIDTH);

    threadDrawerWidth.value = 500;
    resetPaneLayout();
    expect(threadDrawerWidth.value).toBe(floor);
    expect(localStorage.getItem(THREAD_DRAWER_WIDTH_KEY)).toBe(String(floor));
  });

  it('resets to the default where the floor is below it, under 100% ui-scale', () => {
    vi.stubGlobal('getComputedStyle', () => ({ fontSize: '12px', getPropertyValue: () => '' }));
    expect(minDrawerWidth(), 'a 12px root must put the floor below the default')
      .toBeLessThan(DEFAULT_DRAWER_WIDTH);

    threadDrawerWidth.value = 500;
    resetPaneLayout();
    expect(threadDrawerWidth.value).toBe(DEFAULT_DRAWER_WIDTH);
  });

  it('no-ops on mobile', () => {
    (globalThis as any).innerWidth = 375;
    splitRatio.value = 0.8;
    resetPaneLayout();
    expect(splitRatio.value).toBe(0.8);
  });
});

describe('mobileView persistence (survives PWA close)', () => {
  beforeEach(() => {
    sessionStorage.removeItem('lucidos-mobile-view');
    removeWindowItem('lucidos-mobile-view');
    resetState();
  });

  it('cold start restores the last pane from localStorage', () => {
    writeWindowItem('lucidos-mobile-view', 'content');
    expect(getInitialMobileView()).toBe('content');
  });

  it('defaults to thread when localStorage is empty', () => {
    expect(getInitialMobileView()).toBe('thread');
  });

  it('falls back to thread for an invalid localStorage value', () => {
    writeWindowItem('lucidos-mobile-view', 'bogus');
    expect(getInitialMobileView()).toBe('thread');
  });

  it('round-trips through localStorage', () => {
    for (const view of MOBILE_VIEWS) {
      writeWindowItem('lucidos-mobile-view', view);
      expect(getInitialMobileView()).toBe(view);
    }
  });

  it('setMobileView writes to localStorage so it survives a PWA kill', () => {
    setMobileView('content');
    expect(readWindowItem('lucidos-mobile-view')).toBe('content');
  });
});

describe('no browser history side effects', () => {
  let pushStateSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    resetState();
    (globalThis as any).innerWidth = 375;
    pushStateSpy = vi.spyOn(history, 'pushState').mockImplementation(() => {});
  });

  afterEach(() => {
    vi.restoreAllMocks();
    (globalThis as any).innerWidth = 1024;
  });

  it('does not push history entries on pane change', () => {
    navigateToPane('content');
    expect(pushStateSpy).not.toHaveBeenCalled();
  });

  it('does not push history on desktop either', () => {
    (globalThis as any).innerWidth = 1024;
    navigateToPane('content');
    expect(pushStateSpy).not.toHaveBeenCalled();
  });
});
