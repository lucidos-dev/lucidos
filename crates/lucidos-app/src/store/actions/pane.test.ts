import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { mobileView, threadDrawerOpen, MOBILE_VIEWS, PANE_INDEX, setMobileView, getInitialMobileView, type MobileView } from '../store';
import { drawerOpen } from '../../components/layout/Drawer';
import { navigateToPane, checkPaneConsistency, toggleThreads } from './pane';

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
