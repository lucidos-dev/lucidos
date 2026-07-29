import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  mobileView, threadDrawerOpen, threadDrawerWidth, splitRatio, focusedPane,
  DEFAULT_DRAWER_WIDTH, MIN_DRAWER_WIDTH, THREAD_DRAWER_WIDTH_KEY,
  MOBILE_VIEWS, PANE_INDEX, setMobileView, getInitialMobileView, type MobileView,
} from '../store';
import { drawerOpen } from '../../components/layout/Drawer';
import {
  DEFAULT_SPLIT_RATIO, KEYBOARD_RESIZE_STEP_PX, MIN_THREAD_PANE_PX, MIN_CONTENT_PANE_PX,
} from '../../components/layout/splitHelpers';
import {
  navigateToPane, checkPaneConsistency, toggleThreads, focusPane, revealContentPane, revealThreadPane,
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

  it('on desktop: hidden drawer → shows it without changing focus', () => {
    (globalThis as any).innerWidth = 1024;
    threadDrawerOpen.value = false;
    focusedPane.value = 'thread';
    toggleThreads();
    expect(threadDrawerOpen.value).toBe(true);
    // Pure show/hide: the toggle never moves focus onto the drawer.
    expect(focusedPane.value).toBe('thread');
    // mobileView unchanged on desktop
    expect(mobileView.value).toBe('thread');
  });

  it('on desktop: visible drawer → hides it (no focus stage)', () => {
    (globalThis as any).innerWidth = 1024;
    threadDrawerOpen.value = true;
    focusedPane.value = 'thread';
    toggleThreads();
    // One click hides — no intermediate "take focus" stage.
    expect(threadDrawerOpen.value).toBe(false);
    expect(focusedPane.value).toBe('thread');
  });

  it('on desktop: hiding a focused drawer drops focus to the thread pane', () => {
    (globalThis as any).innerWidth = 1024;
    threadDrawerOpen.value = true;
    focusedPane.value = 'drawer';
    toggleThreads();
    expect(threadDrawerOpen.value).toBe(false);
    // A hidden pane must not stay the focused pane (would strand keyboard focus).
    expect(focusedPane.value).toBe('thread');
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

describe('toggleThreadPane / toggleContentPane (two-stage focus → hide)', () => {
  beforeEach(() => {
    resetState(); // focusedPane = 'thread'
    (globalThis as any).innerWidth = 1024;
    splitRatio.value = 0.5;
  });
  afterEach(() => vi.restoreAllMocks());

  it('desktop thread pane: already focused → collapses (focus follows to content), then restores + refocuses', () => {
    // focusedPane starts 'thread' and the pane is visible → first press hides it.
    toggleThreadPane();
    expect(splitRatio.value).toBe(0);
    expect(focusedPane.value).toBe('content');
    // Collapsed → next press expands and refocuses the thread pane.
    toggleThreadPane();
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(focusedPane.value).toBe('thread');
  });

  it('desktop thread pane: visible-but-unfocused → focuses first, hides only on the next press', () => {
    focusedPane.value = 'content'; // thread pane visible but not focused
    toggleThreadPane();
    expect(splitRatio.value).toBe(0.5); // focus only — no collapse
    expect(focusedPane.value).toBe('thread');
    toggleThreadPane();
    expect(splitRatio.value).toBe(0); // now it hides
    expect(focusedPane.value).toBe('content');
  });

  it('desktop content pane: focuses first, then collapses, then restores + refocuses', () => {
    // focusedPane starts 'thread' → first press just focuses content.
    toggleContentPane();
    expect(splitRatio.value).toBe(0.5);
    expect(focusedPane.value).toBe('content');
    // Focused + visible → next press collapses it (focus falls to thread).
    toggleContentPane();
    expect(splitRatio.value).toBe(1);
    expect(focusedPane.value).toBe('thread');
    // Collapsed → next press expands and refocuses content.
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
    expect(focusedPane.value).toBe('thread'); // unchanged on mobile
    expect(checkPaneConsistency()).toBeNull();
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

// ─────────────────────────────────────────────────────────────────────────────
// revealContentPane — every content navigation activates the Content pane group
// so keyboard tabbing (handlePaneTab, anchored on focusedPane) lands on the
// freshly-navigated view rather than the previously-focused pane.
// ─────────────────────────────────────────────────────────────────────────────
describe('revealContentPane', () => {
  beforeEach(() => {
    resetState(); // focusedPane = 'thread'
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
    splitRatio.value = 1; // content collapsed
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
    expect(focusedPane.value).toBe('thread'); // mobile navigates, never focuses
  });
});

// ─────────────────────────────────────────────────────────────────────────────
// revealThreadPane — the mirror of revealContentPane for thread navigation
// (focusThread / unfocusThread / sendMessage's raw-new-thread path). Desktop:
// re-activate the Threads pane group ONLY from the cross-group case so drawer/
// thread focus is left alone. Mobile: swipe to the thread pane.
// ─────────────────────────────────────────────────────────────────────────────
describe('revealThreadPane', () => {
  beforeEach(() => {
    resetState(); // focusedPane = 'thread', mobileView = 'thread'
    splitRatio.value = 0.5;
  });
  afterEach(() => vi.restoreAllMocks());

  it('desktop: re-activates the Threads pane group from the content group', () => {
    (globalThis as any).innerWidth = 1024;
    focusedPane.value = 'content'; // arriving from the Content pane group
    revealThreadPane();
    expect(focusedPane.value).toBe('thread');
  });

  it('desktop: leaves an existing drawer focus alone (no cross-group switch)', () => {
    (globalThis as any).innerWidth = 1024;
    focusedPane.value = 'drawer'; // browsing the thread list via keyboard
    revealThreadPane();
    expect(focusedPane.value).toBe('drawer');
  });

  it('desktop: idempotent when already focused on the thread pane', () => {
    (globalThis as any).innerWidth = 1024;
    focusedPane.value = 'thread';
    revealThreadPane();
    expect(focusedPane.value).toBe('thread');
  });

  it('desktop: does not move the split ratio (focus-only)', () => {
    (globalThis as any).innerWidth = 1024;
    focusedPane.value = 'content';
    splitRatio.value = 0.5;
    revealThreadPane();
    expect(splitRatio.value).toBe(0.5);
  });

  it('mobile: navigates to the thread pane and never touches focusedPane', () => {
    (globalThis as any).innerWidth = 375;
    mobileView.value = 'content';
    revealThreadPane();
    expect(mobileView.value).toBe('thread');
    expect(focusedPane.value).toBe('thread'); // mobile navigates, never focuses
  });

  it('mobile: keeps the thread drawer open when navigating to the thread pane', () => {
    (globalThis as any).innerWidth = 375;
    mobileView.value = 'thread';
    threadDrawerOpen.value = true;
    revealThreadPane();
    // navigateToPane('thread') keeps the drawer (the `view !== 'thread'` guard) —
    // this is what lets the archive "last review → compose, drawer stays open"
    // behavior survive the move from a manual navigateToPane to revealThreadPane.
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

// iOS PWA cold-start: the last-viewed pane must survive the PWA being killed, so
// reopening lands on the pane the user left (e.g. content), not a forced reset to
// thread. The pane lives in localStorage (survives process death); the content
// pane's actual content (open app/file) is independently restored from the
// localStorage nav stack, so landing on `content` is never a stranded blank pane.
describe('mobileView persistence (survives PWA close)', () => {
  beforeEach(() => {
    sessionStorage.removeItem('lucidos-mobile-view');
    localStorage.removeItem('lucidos-mobile-view');
    resetState();
  });

  it('cold start restores the last pane from localStorage', () => {
    // Simulate the reopen scenario: the user closed on the content pane, iOS
    // killed the PWA (sessionStorage gone), localStorage survives → land on content.
    localStorage.setItem('lucidos-mobile-view', 'content');
    expect(getInitialMobileView()).toBe('content');
  });

  it('defaults to thread when localStorage is empty', () => {
    expect(getInitialMobileView()).toBe('thread');
  });

  it('falls back to thread for an invalid localStorage value', () => {
    localStorage.setItem('lucidos-mobile-view', 'bogus');
    expect(getInitialMobileView()).toBe('thread');
  });

  it('round-trips through localStorage', () => {
    for (const view of MOBILE_VIEWS) {
      localStorage.setItem('lucidos-mobile-view', view);
      expect(getInitialMobileView()).toBe(view);
    }
  });

  it('setMobileView writes to localStorage so it survives a PWA kill', () => {
    setMobileView('content');
    expect(localStorage.getItem('lucidos-mobile-view')).toBe('content');
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
