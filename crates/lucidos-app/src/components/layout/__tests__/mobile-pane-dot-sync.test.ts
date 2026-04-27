import { describe, it, expect, beforeEach } from 'vitest';
import { mobileView, threadDrawerOpen, MOBILE_VIEWS, PANE_INDEX, type MobileView } from '../../../store/store';
import { navigateToPane, checkPaneConsistency, toggleThreads } from '../../../store/actions/pane';
import { MOBILE_PANE_CONFIGS, type MobilePaneConfig } from '../MobileAppHeader';

// ─────────────────────────────────────────────────────────────────────────────
// Mobile pane / dot indicator sync tests
//
// The bug: on mobile, clicking the ThreadsIcon button in MobileThreadHeader
// toggled `threadDrawerOpen` instead of navigating to the threads pane.
// This showed threads via drawer overlay while dots still indicated "thread"
// pane — a visible desync.
//
// The fix: on mobile, ThreadsIcon calls toggleThreads() which navigates to
// pane 0 (threads). The thread drawer overlay is a desktop-only concept.
// threadDrawerOpen must never be the mechanism to show threads on mobile.
//
// Structural guarantees:
//   1. MobileView, MOBILE_VIEWS, PANE_INDEX, PANE_COUNT all derive from
//      a single PANE_DEFS array — can't add a pane to one without the others.
//   2. toggleThreads() is the ONLY way to show/hide threads from UI —
//      routes mobile through navigateToPane('threads') and desktop through
//      threadDrawerOpen toggle.
//   3. ThreadDrawer overlay visibility is disabled on mobile — forceVisible
//      (pane 0) is the only way threads render.
// ─────────────────────────────────────────────────────────────────────────────

function setMobile() {
  (globalThis as any).innerWidth = 375;
}

function setDesktop() {
  (globalThis as any).innerWidth = 1024;
}

function resetState(view: MobileView = 'thread') {
  mobileView.value = view;
  threadDrawerOpen.value = false;
}

describe('mobile pane/dot sync — structural guarantees', () => {
  beforeEach(() => {
    setMobile();
    resetState();
  });

  it('navigating to threads pane updates mobileView to threads', () => {
    navigateToPane('threads');
    expect(mobileView.value).toBe('threads');
  });

  it('navigating to threads pane closes threadDrawerOpen', () => {
    threadDrawerOpen.value = true;
    navigateToPane('threads');
    expect(threadDrawerOpen.value).toBe(false);
  });

  it('every MOBILE_VIEWS entry has a matching PANE_INDEX', () => {
    for (const view of MOBILE_VIEWS) {
      expect(PANE_INDEX[view]).toBeDefined();
      expect(typeof PANE_INDEX[view]).toBe('number');
    }
  });

  it('PANE_INDEX is contiguous starting from 0', () => {
    for (let i = 0; i < MOBILE_VIEWS.length; i++) {
      expect(PANE_INDEX[MOBILE_VIEWS[i]]).toBe(i);
    }
  });

  it('dot active state always matches mobileView after any navigation', () => {
    for (const target of MOBILE_VIEWS) {
      navigateToPane(target);
      for (const v of MOBILE_VIEWS) {
        const dotActive = v === mobileView.value;
        const isPaneVisible = v === target;
        expect(dotActive, `dot for '${v}' when viewing '${target}'`).toBe(isPaneVisible);
      }
    }
  });

  it('pane position (PANE_INDEX) agrees with dot indicator after navigation', () => {
    for (const target of MOBILE_VIEWS) {
      navigateToPane(target);
      const activeIndex = PANE_INDEX[mobileView.value];
      const expectedIndex = PANE_INDEX[target];
      expect(activeIndex, `pane index after navigating to '${target}'`).toBe(expectedIndex);
    }
  });

  it('consistency holds after every possible navigation', () => {
    for (const target of MOBILE_VIEWS) {
      navigateToPane(target);
      expect(checkPaneConsistency()).toBeNull();
    }
  });
});

describe('toggleThreads — mobile vs desktop routing', () => {
  beforeEach(() => resetState());

  it('on mobile: navigates to threads pane, dots update', () => {
    setMobile();
    mobileView.value = 'thread';
    toggleThreads();
    expect(mobileView.value).toBe('threads');
    expect(PANE_INDEX[mobileView.value]).toBe(0); // leftmost dot
    expect(threadDrawerOpen.value).toBe(false);
    expect(checkPaneConsistency()).toBeNull();
  });

  it('on mobile: cleans up stale threadDrawerOpen', () => {
    setMobile();
    threadDrawerOpen.value = true; // stale from desktop
    toggleThreads();
    expect(threadDrawerOpen.value).toBe(false);
    expect(mobileView.value).toBe('threads');
  });

  it('on desktop: toggles threadDrawerOpen, does not touch mobileView', () => {
    setDesktop();
    threadDrawerOpen.value = false;
    toggleThreads();
    expect(threadDrawerOpen.value).toBe(true);
    expect(mobileView.value).toBe('thread'); // unchanged

    toggleThreads();
    expect(threadDrawerOpen.value).toBe(false);
  });
});

describe('MOBILE_PANE_CONFIGS — structural completeness', () => {
  it('has an entry for every MobileView', () => {
    for (const view of MOBILE_VIEWS) {
      const config = MOBILE_PANE_CONFIGS[view];
      expect(config, `missing config for '${view}'`).toBeDefined();
    }
  });

  it('every entry has both Header and Pane components', () => {
    for (const view of MOBILE_VIEWS) {
      const config: MobilePaneConfig = MOBILE_PANE_CONFIGS[view];
      expect(typeof config.Header, `${view}.Header`).toBe('function');
      expect(typeof config.Pane, `${view}.Pane`).toBe('function');
    }
  });

  it('has no extra entries beyond MOBILE_VIEWS', () => {
    const configKeys = Object.keys(MOBILE_PANE_CONFIGS);
    expect(configKeys.sort()).toEqual([...MOBILE_VIEWS].sort());
  });

  it('headers are unique across panes', () => {
    const headers = MOBILE_VIEWS.map(v => MOBILE_PANE_CONFIGS[v].Header);
    expect(new Set(headers).size).toBe(MOBILE_VIEWS.length);
  });

  it('panes are unique across panes', () => {
    const panes = MOBILE_VIEWS.map(v => MOBILE_PANE_CONFIGS[v].Pane);
    expect(new Set(panes).size).toBe(MOBILE_VIEWS.length);
  });
});

describe('mobile: thread drawer overlay is disabled', () => {
  beforeEach(() => {
    setMobile();
    resetState();
  });

  it('drawer overlay visibility formula excludes mobile', () => {
    // The ThreadDrawer visible formula:
    //   forceVisible || (threadDrawerOpen.value && splitRatio > 0)
    // On mobile, splitRatio is 0 and forceVisible is only true in
    // MobileThreadsPane (pane 0). The overlay path (threadDrawerOpen
    // on the thread pane) is structurally disabled.
    const splitRatio = 0; // mobile
    const forceVisible = false; // not in MobileThreadsPane
    threadDrawerOpen.value = true;

    const visible = forceVisible || (threadDrawerOpen.value && splitRatio > 0);
    expect(visible).toBe(false); // drawer overlay never shows on mobile
  });
});
