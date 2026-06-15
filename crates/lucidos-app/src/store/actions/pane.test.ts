import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  mobileView, threadDrawerOpen, threadDrawerWidth, splitRatio,
  DEFAULT_DRAWER_WIDTH, MIN_DRAWER_WIDTH, THREAD_DRAWER_WIDTH_KEY,
  MOBILE_VIEWS, PANE_INDEX, setMobileView, getInitialMobileView, type MobileView,
} from '../store';
import { drawerOpen } from '../../components/layout/Drawer';
import {
  DEFAULT_SPLIT_RATIO, KEYBOARD_RESIZE_STEP_PX, MIN_THREAD_PANE_PX, MIN_CONTENT_PANE_PX,
} from '../../components/layout/splitHelpers';
import {
  navigateToPane, checkPaneConsistency, toggleThreads,
  toggleThreadPane, toggleContentPane, stepThreadPaneWidth, stepThreadDrawerWidth, resetPaneLayout,
} from './pane';

// ─────────────────────────────────────────────────────────────────────────────
// Pane state consistency tests
//
// Invariants:
//   - mobileView is always one of MOBILE_VIEWS
//   - threadDrawerOpen can only be true when mobileView === 'thread'
//   - After any navigateToPane call, both drawers are closed
// ─────────────────────────────────────────────────────────────────────────────

function resetState(view: MobileView = 'thread') {
  mobileView.value = view;
  threadDrawerOpen.value = false;
  drawerOpen.value = false;
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
    // Drawer is allowed to be open only on the thread pane, so navigating to
    // 'thread' must preserve it (e.g. dismissing the last review lands the
    // user on the compose view with the drawer still visible).
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
          // Drawer is preserved when target is 'thread'; closed otherwise.
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

  it('on desktop: toggles threadDrawerOpen on', () => {
    (globalThis as any).innerWidth = 1024;
    threadDrawerOpen.value = false;
    toggleThreads();
    expect(threadDrawerOpen.value).toBe(true);
    // mobileView unchanged on desktop
    expect(mobileView.value).toBe('thread');
  });

  it('on desktop: toggles threadDrawerOpen off', () => {
    (globalThis as any).innerWidth = 1024;
    threadDrawerOpen.value = true;
    toggleThreads();
    expect(threadDrawerOpen.value).toBe(false);
  });
});

// ─────────────────────────────────────────────────────────────────────────────
// Keyboard pane intents (toggles, divider steps, layout reset)
// ─────────────────────────────────────────────────────────────────────────────

/** Element stub for the mocked document.querySelector: offsetWidth for the
 *  step math, classList for setSplitRatio's triggerSnapAnimate. */
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

describe('toggleThreadPane / toggleContentPane', () => {
  beforeEach(() => {
    resetState();
    (globalThis as any).innerWidth = 1024;
    splitRatio.value = 0.5;
  });
  afterEach(() => vi.restoreAllMocks());

  it('desktop: collapses the thread pane, then restores it to the default ratio', () => {
    toggleThreadPane();
    expect(splitRatio.value).toBe(0);
    toggleThreadPane();
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
  });

  it('desktop: collapses the content pane, then restores it to the default ratio', () => {
    toggleContentPane();
    expect(splitRatio.value).toBe(1);
    toggleContentPane();
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
  });

  it('mobile: navigates to the pane instead of touching the split ratio', () => {
    (globalThis as any).innerWidth = 375;
    toggleThreadPane();
    expect(mobileView.value).toBe('thread');
    toggleContentPane();
    expect(mobileView.value).toBe('content');
    expect(splitRatio.value).toBe(0.5);
    expect(checkPaneConsistency()).toBeNull();
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
  // Visible thread + content panes reserve their minimums against the drawer.
  const MAX = ROW - MIN_THREAD_PANE_PX - MIN_CONTENT_PANE_PX;

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
    threadDrawerWidth.value = MIN_DRAWER_WIDTH + 10;
    stepThreadDrawerWidth(-1);
    expect(threadDrawerWidth.value).toBe(MIN_DRAWER_WIDTH);
    stepThreadDrawerWidth(-1);
    expect(threadDrawerWidth.value).toBe(MIN_DRAWER_WIDTH);
  });

  it('clamps so the visible split panes keep their minimum widths', () => {
    threadDrawerWidth.value = MAX - 10;
    stepThreadDrawerWidth(1);
    expect(threadDrawerWidth.value).toBe(MAX);
  });

  it('no-ops while the drawer is hidden (closed, or thread pane collapsed)', () => {
    threadDrawerOpen.value = false;
    stepThreadDrawerWidth(1);
    expect(threadDrawerWidth.value).toBe(400);

    threadDrawerOpen.value = true;
    splitRatio.value = 0; // drawer hides with the collapsed thread pane
    stepThreadDrawerWidth(1);
    expect(threadDrawerWidth.value).toBe(400);
  });
});

describe('resetPaneLayout', () => {
  beforeEach(() => {
    resetState();
    (globalThis as any).innerWidth = 1024;
  });
  afterEach(() => vi.restoreAllMocks());

  it('restores the default split ratio and drawer width, persisting the width', () => {
    splitRatio.value = 0.8;
    threadDrawerWidth.value = 500;
    resetPaneLayout();
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(threadDrawerWidth.value).toBe(DEFAULT_DRAWER_WIDTH);
    expect(localStorage.getItem(THREAD_DRAWER_WIDTH_KEY)).toBe(String(DEFAULT_DRAWER_WIDTH));
  });

  it('no-ops on mobile', () => {
    (globalThis as any).innerWidth = 375;
    splitRatio.value = 0.8;
    resetPaneLayout();
    expect(splitRatio.value).toBe(0.8);
  });
});

// iOS PWA cold-start: when iOS kills the PWA after the user opened an app and
// never swiped back, the next launch must land on the thread pane — sessionStorage
// dies with the process, and any pre-fix localStorage value must not leak in.
describe('mobileView session-only persistence', () => {
  beforeEach(() => {
    sessionStorage.removeItem('lucidos-mobile-view');
    localStorage.removeItem('lucidos-mobile-view');
    resetState();
  });

  it('cold start with stale localStorage="content" still defaults to thread', () => {
    // Simulate the bug scenario: an old build wrote `content` to localStorage,
    // sessionStorage is empty (process was killed). The user must land on thread.
    localStorage.setItem('lucidos-mobile-view', 'content');
    expect(getInitialMobileView()).toBe('thread');
  });

  it('defaults to thread when sessionStorage is empty', () => {
    expect(getInitialMobileView()).toBe('thread');
  });

  it('falls back to thread for an invalid sessionStorage value', () => {
    sessionStorage.setItem('lucidos-mobile-view', 'bogus');
    expect(getInitialMobileView()).toBe('thread');
  });

  it('round-trips through sessionStorage', () => {
    for (const view of MOBILE_VIEWS) {
      sessionStorage.setItem('lucidos-mobile-view', view);
      expect(getInitialMobileView()).toBe(view);
    }
  });

  it('setMobileView writes to sessionStorage, not localStorage', () => {
    setMobileView('content');
    expect(sessionStorage.getItem('lucidos-mobile-view')).toBe('content');
    expect(localStorage.getItem('lucidos-mobile-view')).toBeNull();
  });
});

describe('no browser history side effects', () => {
  let pushStateSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    resetState();
    (globalThis as any).innerWidth = 375; // mobile
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
