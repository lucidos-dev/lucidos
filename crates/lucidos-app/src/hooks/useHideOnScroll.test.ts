import { describe, it, expect, beforeEach } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { shouldKeepHeaderVisible, spacerHeightPx } from './useHideOnScroll';

describe('--mobile-header-offset stays off the document root', () => {
  // The var is rewritten on essentially every scroll frame. Custom properties
  // inherit, so writing it on `documentElement` invalidated style for every node
  // in the document, and the thread transcript is the largest tree in the app:
  // that was a whole-document style recalc per frame, and the jank that survived
  // moving the var off `top` onto `transform` (which removed only the LAYOUT
  // half). It is written on its two consumer elements instead.
  //
  // A source scan rather than a behavioral test because the regression is about
  // WHICH element is written, and this suite has no DOM (the scroll logic is
  // exercised through the pure mirror below).
  const hookSource = readFileSync(
    resolve(dirname(fileURLToPath(import.meta.url)), 'useHideOnScroll.ts'),
    'utf8',
  );

  it('never sets the offset on documentElement', () => {
    const rootWrites = hookSource.match(
      /documentElement\.style\.setProperty\(\s*['"]--mobile-header-offset/g,
    ) ?? [];
    expect(rootWrites).toHaveLength(0);
  });

  it('writes the offset on both consumer elements', () => {
    // Both, not one: the sticky title bar AND the scroll-to-top chevron read it
    // (styles/mobile.css). Dropping either leaves that element at its resting
    // position while the header scrolls away.
    expect(hookSource).toMatch(/titleBarEl\?\.style\.setProperty\(\s*['"]--mobile-header-offset/);
    expect(hookSource).toMatch(/chevronEl\?\.style\.setProperty\(\s*['"]--mobile-header-offset/);
  });

  // The scroll-delta logic below is a hand-written MIRROR of the hook, so these
  // two keep the mirror honest about the anchored-reveal behaviour it pins.
  it('gates the navigation reveal on the anchor kind', () => {
    expect(hookSource).toMatch(/if \(!isAnchorScroll\(\)\) headerOffset = 0;/);
  });

  it('re-takes its baseline at the anchor write, not on the scroll event', () => {
    expect(hookSource).toMatch(/onAnchorScroll\(\(el\) => \{[\s\S]*prevScrollTop = clampedScrollTop\(el\)/);
  });

  it('re-takes it again on the settling frame, and cancels that frame on teardown', () => {
    // The write is not the end of a reveal. A shrinking transcript is still
    // settling, and the browser's own clamp lands a frame later. Unabsorbed,
    // the header spent that clamp as a full reveal over the line the correction
    // had just held. The mirror below models the synchronous half only, so the
    // frame is pinned here.
    expect(hookSource).toMatch(/anchorSettleRaf = requestAnimationFrame\(\(\) => \{[\s\S]*prevScrollTop = clampedScrollTop\(el\)/);
    expect(hookSource).toMatch(/if \(anchorSettleRaf !== null\) cancelAnimationFrame\(anchorSettleRaf\);[\s\S]*observer\.disconnect\(\)/);
  });

  it('keeps the offset off `top`, which would reinstate the forced layout', () => {
    // The companion half of the fix, in CSS: both consumers take the offset on
    // `transform` (composited) so the write cannot dirty layout. A `top` that
    // reads the offset means the next scroll event's scrollTop read forces a
    // synchronous style+layout flush of the whole transcript again.
    const css = readFileSync(
      resolve(dirname(fileURLToPath(import.meta.url)), '../styles/mobile.css'),
      'utf8',
    );
    const offsetOnTop = css.match(/top:[^;]*--mobile-header-offset/g) ?? [];
    expect(offsetOnTop).toHaveLength(0);
    expect(css).toMatch(/transform:\s*translateY\([^;]*--mobile-header-offset/);
  });
});

// Tests scroll-delta and keyboard-suppression logic from useHideOnScroll.
// Uses string pane identity instead of DOM .closest() traversal.
function createScrollTracker(
  getActiveElement: () => { tagName: string; pane?: string } | null,
  isNavigationScroll: () => boolean = () => false,
  isRepaintNudging: () => boolean = () => false,
  isUserScrolling: () => boolean = () => false,
  isAnchorScroll: () => boolean = () => false,
) {
  let prevScrollTop = 0;
  let headerOffset = 0;
  const cachedHeight = 48;
  let currentPane: string | null = null;
  let currentViewKey: string | null = null;
  let keyboardOpen = false;
  let disabled = false;
  const paneState: Record<string, { headerOffset: number; prevScrollTop: number }> = {};

  function clampOffset(offset: number) {
    return Math.min(0, Math.max(-cachedHeight, offset)) || 0;
  }

  function setCurrentPane(pane: string | null) {
    currentPane = pane;
  }

  function applyScrollDelta(scrollTop: number, scrollHeight: number, clientHeight: number) {
    // One of our own navigations is writing scrollTop frame by frame (a chevron
    // tap, turn-nav, a deep-link glide). Reset the header to visible rather than
    // hiding it on the way down: those scroll events are not the reader.
    if (isNavigationScroll()) {
      const maxScroll = Math.max(0, scrollHeight - clientHeight);
      prevScrollTop = Math.min(Math.max(0, scrollTop), maxScroll);
      // An ANCHOR write is the exception: the app moved the container so the
      // reader's own line would NOT move, so the chrome stays where they left
      // it. Revealing it there covers that line.
      if (!isAnchorScroll()) headerOffset = 0;
      return;
    }

    const active = getActiveElement();
    if (active && (active.tagName === 'TEXTAREA' || active.tagName === 'INPUT' || active.tagName === 'SELECT')) {
      // Only suppress if the focused input is in the same pane as the scroll container
      if (active.pane === currentPane) return;
    }

    // The iOS compositor-recovery nudge writes ±1px and puts it back a frame
    // later. Skip WITHOUT advancing prevScrollTop, so the round trip leaves the
    // baseline exactly where the user left it. A live drag overrides the window:
    // a nudge is never written while the user scrolls, so suppressing then could
    // only eat the user's own events.
    if (isRepaintNudging() && !isUserScrolling()) return;

    const maxScroll = Math.max(0, scrollHeight - clientHeight);
    const clamped = Math.min(Math.max(0, scrollTop), maxScroll);
    const delta = clamped - prevScrollTop;
    headerOffset = clampOffset(headerOffset - delta);
    prevScrollTop = clamped;
  }

  /** Switch to a new scroll container with per-pane state isolation.
   *  Each pane remembers its own header offset independently. */
  function switchContainer(containerScrollTop: number | null, viewKey?: string) {
    if (currentViewKey) {
      paneState[currentViewKey] = { headerOffset, prevScrollTop };
    }

    if (containerScrollTop !== null) {
      const key = viewKey ?? `pane-${Object.keys(paneState).length}`;
      currentViewKey = key;
      const saved = paneState[key];
      if (saved) {
        headerOffset = saved.headerOffset;
        prevScrollTop = saved.prevScrollTop;
      } else {
        const scrollPos = Math.max(0, containerScrollTop);
        headerOffset = clampOffset(-scrollPos);
        prevScrollTop = scrollPos;
      }
    } else {
      currentViewKey = null;
      headerOffset = 0;
      prevScrollTop = 0;
    }
  }

  /** The app re-based the container to hold the reader on the same content
   *  (`onAnchorScroll`). Re-take the baseline at the write, so the scroll event
   *  it fires carries a delta of zero whenever it lands. */
  function rebaseAnchor(scrollTop: number, scrollHeight: number, clientHeight: number) {
    const maxScroll = Math.max(0, scrollHeight - clientHeight);
    prevScrollTop = Math.min(Math.max(0, scrollTop), maxScroll);
  }

  /** Sync header to match container scroll position (used after keyboard dismiss). */
  function syncToScroll(containerScrollTop: number) {
    const scrollPos = Math.max(0, containerScrollTop);
    headerOffset = clampOffset(-scrollPos);
    prevScrollTop = scrollPos;
  }

  /** Hide header when keyboard opens (input gains focus).
   *  Matches real onFocusIn which sets headerOffset = -cachedHeight. */
  function onFocusIn(containerScrollTop: number) {
    headerOffset = -cachedHeight;
    prevScrollTop = Math.max(0, containerScrollTop);
  }

  /** Correct header offset if scroll position warrants more visibility.
   *  Called on DOM mutations when the container hasn't changed but
   *  content may have shrunk (e.g. steps collapsed). */
  function correctForScrollPosition(containerScrollTop: number) {
    const actualScroll = Math.max(0, containerScrollTop);
    if (actualScroll < cachedHeight) {
      const corrected = clampOffset(-actualScroll);
      if (headerOffset < corrected) {
        headerOffset = corrected;
        prevScrollTop = actualScroll;
      }
    }
  }

  /** Returns the effective header offset, accounting for keyboard and disabled state.
   *  When disabled (app UI active), always returns 0 (fully visible).
   *  When keyboard is open, always returns fully hidden (-cachedHeight). */
  function getEffectiveOffset(): number {
    if (disabled) return 0;
    return keyboardOpen ? -cachedHeight : headerOffset;
  }

  function setDisabled(value: boolean) {
    disabled = value;
  }

  function setKeyboardOpen(open: boolean) {
    keyboardOpen = open;
  }

  /** Recover from stale keyboard state.
   *  Called when switching panes or on scroll — checks whether
   *  a text input is still focused. If not, resets keyboardOpen. */
  function recoverKeyboardState(isTextInputFocused: boolean) {
    if (keyboardOpen && !isTextInputFocused) {
      keyboardOpen = false;
    }
  }

  return {
    applyScrollDelta,
    rebaseAnchor,
    switchContainer,
    syncToScroll,
    onFocusIn,
    correctForScrollPosition,
    setCurrentPane,
    setKeyboardOpen,
    recoverKeyboardState,
    get headerOffset() { return headerOffset; },
    get keyboardOpen() { return keyboardOpen; },
    get disabled() { return disabled; },
    getEffectiveOffset,
    setDisabled,
  };
}

describe('useHideOnScroll keyboard suppression', () => {
  let mockActive: { tagName: string; pane?: string } | null = null;
  let tracker: ReturnType<typeof createScrollTracker>;

  beforeEach(() => {
    mockActive = null;
    tracker = createScrollTracker(() => mockActive);
    tracker.setCurrentPane('threads');
  });

  it('hides header on scroll-down when no input focused', () => {
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(30, 1000, 500);
    expect(tracker.headerOffset).toBe(-30);
  });

  it('ignores scroll when textarea has focus in same pane', () => {
    mockActive = { tagName: 'TEXTAREA', pane: 'threads' };

    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(50, 1000, 500);
    tracker.applyScrollDelta(100, 800, 300);

    expect(tracker.headerOffset).toBe(0);
  });

  it('resumes scroll tracking after blur with header synced to scroll position', () => {
    mockActive = { tagName: 'TEXTAREA', pane: 'threads' };

    tracker.applyScrollDelta(50, 1000, 500);
    expect(tracker.headerOffset).toBe(0);

    // Blur: sync header to scroll position (keyboard dismissed)
    mockActive = null;
    tracker.syncToScroll(50);
    expect(tracker.headerOffset).toBe(-48); // fully hidden at scrollTop=50

    tracker.applyScrollDelta(80, 1000, 500);
    expect(tracker.headerOffset).toBe(-48); // already fully hidden
  });

  it('ignores scroll with input element focused in same pane', () => {
    mockActive = { tagName: 'INPUT', pane: 'threads' };

    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(30, 1000, 500);
    expect(tracker.headerOffset).toBe(0);
  });

  it('hides header when input gains focus (keyboard opens)', () => {
    // Header starts visible
    expect(tracker.headerOffset).toBe(0);

    // User taps input — keyboard opens, header hides to avoid
    // iOS position:fixed issues with software keyboard
    tracker.onFocusIn(0);
    expect(tracker.headerOffset).toBe(-48); // fully hidden
  });

  it('allows scroll when textarea has focus in a DIFFERENT pane (iOS swipe)', () => {
    // Simulates: user was typing in compose view (pane 'thread'),
    // then swiped to threads view (pane 'threads'). iOS Safari may not
    // blur the textarea, but scroll handling must still work in threads pane.
    tracker.setCurrentPane('threads');
    mockActive = { tagName: 'TEXTAREA', pane: 'thread' };

    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(30, 1000, 500);
    expect(tracker.headerOffset).toBe(-30); // header hides despite textarea focus
  });

  it('each pane has independent header scroll state', () => {
    // Start in pane A, scroll down to hide header
    tracker.switchContainer(0, 'threads');
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48); // fully hidden

    // Switch to pane B at scrollTop=0 — header resets to visible (new pane, never scrolled)
    tracker.switchContainer(0, 'thread');
    expect(tracker.headerOffset).toBe(0); // NOT -48

    // Switch back to pane A — header offset is restored to hidden
    tracker.switchContainer(0, 'threads');
    expect(tracker.headerOffset).toBe(-48); // restored from saved state
  });

  it('reveals header when scrolling up in new container after switch', () => {
    // Switch to pane with scroll position 200 (header hides to match)
    tracker.switchContainer(200, 'thread');
    expect(tracker.headerOffset).toBe(-48);

    // Scroll up 30px — header partially reveals
    tracker.applyScrollDelta(170, 1000, 500);
    expect(tracker.headerOffset).toBe(-18); // -48 + 30 = -18
  });

  it('resets header when container disappears (null)', () => {
    // Hide header
    tracker.switchContainer(0, 'threads');
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);

    // Container removed from DOM (switchContainer with no scroll position)
    // — header must reset to visible, not stay hidden
    tracker.switchContainer(null);
    expect(tracker.headerOffset).toBe(0);
  });

  it('hides header when switching to a scrolled-down container', () => {
    // Header is visible (default)
    expect(tracker.headerOffset).toBe(0);

    // Switch to a container scrolled 200px down — header hides to match
    tracker.switchContainer(200, 'content');
    expect(tracker.headerOffset).toBe(-48); // fully hidden (200 > cachedHeight)
  });

  it('keeps header visible when switching to a container at top', () => {
    // Header is visible (default)
    expect(tracker.headerOffset).toBe(0);

    // Switch to a container at scrollTop=0 — header stays visible
    tracker.switchContainer(0, 'thread');
    expect(tracker.headerOffset).toBe(0);
  });
});

describe('useHideOnScroll during one of our own navigations', () => {
  // The branch used to read `getResizeMode() === 'scroll'`, which answered this
  // question only by accident: the bottom-pin's 500ms suppression window
  // happened to be open across the programmatic scrolls that mattered. It now
  // asks `isNavigationScroll()`, which is true for a live tween AND for the few
  // frames after any of our writes, because a scroll event lands after the write
  // that caused it rather than during it.
  let mockActive: { tagName: string; pane?: string } | null = null;
  let navigating = false;

  beforeEach(() => {
    mockActive = null;
    navigating = false;
  });

  it('resets the header to visible while a navigation is gliding', () => {
    const tracker = createScrollTracker(() => mockActive, () => navigating);
    tracker.switchContainer(0, 'thread');

    // User scrolls down — header hides
    tracker.applyScrollDelta(100, 2000, 500);
    expect(tracker.headerOffset).toBe(-48); // fully hidden

    // The down chevron is tapped: its tween writes scrollTop per frame.
    navigating = true;
    tracker.applyScrollDelta(1500, 2000, 500);

    // Header should be reset to visible, not hidden further
    expect(tracker.headerOffset).toBe(0);
  });

  it('resumes normal scroll tracking once the tween lands', () => {
    const tracker = createScrollTracker(() => mockActive, () => navigating);
    tracker.switchContainer(0, 'thread');

    navigating = true;
    tracker.applyScrollDelta(1500, 2000, 500); // scrollTop=1500, maxScroll=1500
    expect(tracker.headerOffset).toBe(0);

    navigating = false; // the tween finished

    // User scrolls up a bit, then back down — header hides on the down scroll
    tracker.applyScrollDelta(1470, 2000, 500); // scroll up 30px → header stays 0
    tracker.applyScrollDelta(1500, 2000, 500); // scroll down 30px → header -30
    expect(tracker.headerOffset).toBe(-30);
  });

  it('keeps the header visible across a multi-frame glide', () => {
    const tracker = createScrollTracker(() => mockActive, () => navigating);

    navigating = true;
    tracker.switchContainer(0, 'thread');

    // Each frame of the tween fires a scroll event.
    tracker.applyScrollDelta(500, 1000, 500);
    expect(tracker.headerOffset).toBe(0); // visible

    tracker.applyScrollDelta(1500, 2000, 500);
    expect(tracker.headerOffset).toBe(0); // still visible

    navigating = false;

    // User scrolls up then down — header hides on down scroll
    tracker.applyScrollDelta(1470, 2000, 500);
    tracker.applyScrollDelta(1500, 2000, 500);
    expect(tracker.headerOffset).toBe(-30);
  });
});

describe('useHideOnScroll iOS repaint nudge suppression', () => {
  // `forceWebKitRepaint` (utils/webkitRepaint.ts) recovers a blanked WKWebView
  // compositor layer by writing scrollTop ±1px and restoring it a frame later.
  // Both writes fire a real scroll event on the container the header listens to.
  // Reported on an iOS PWA, 2026-08-03: with "Keep header visible" off, the
  // header shook while the user was doing nothing. On a streaming thread the
  // nudge runs on a ~200ms throttle, so the shake is continuous.
  let mockActive: { tagName: string; pane?: string } | null = null;
  let nudging = false;
  let userScrolling = false;

  beforeEach(() => {
    mockActive = null;
    nudging = false;
    userScrolling = false;
  });

  function trackerWithNudge() {
    return createScrollTracker(
      () => mockActive,
      () => false, // no navigation of our own in flight
      () => nudging,
      () => userScrolling,
    );
  }

  /** Drive one nudge round trip at `scrollTop`. `forceWebKitRepaint` nudges UP first
   *  (`live > 0 ? live - 1 : live + 1`) and restores a frame later, so the legs
   *  are -1 then +1. Asserting only the end state misses the bug: the twitch is
   *  the intermediate leg, and the drift needs the legs in this exact order. */
  function nudgeRoundTrip(
    tracker: ReturnType<typeof createScrollTracker>,
    scrollTop: number,
    onFirstLeg?: () => void,
  ) {
    nudging = true;
    tracker.applyScrollDelta(scrollTop - 1, 2000, 500);
    onFirstLeg?.();
    tracker.applyScrollDelta(scrollTop, 2000, 500);
    nudging = false;
  }

  it('does not move the header on the nudge leg itself', () => {
    // The twitch. Even where the round trip nets out symmetrically (header fully
    // hidden, neither leg clamps), the -1px leg still revealed a pixel of header
    // for a frame before the restore took it back.
    const tracker = trackerWithNudge();
    tracker.switchContainer(0, 'thread');
    tracker.applyScrollDelta(400, 2000, 500);
    expect(tracker.headerOffset).toBe(-48); // fully hidden

    let midNudge = NaN;
    nudgeRoundTrip(tracker, 400, () => { midNudge = tracker.headerOffset; });

    expect(midNudge).toBe(-48);
    expect(tracker.headerOffset).toBe(-48);
  });

  it('does not leave a fully visible header oscillating by a pixel', () => {
    // The round trip does not even cancel once a leg clamps: with the header
    // already fully visible, the -1px (reveal) leg clamps at 0 while the +1px
    // (hide) leg is free to move, so the header settles a pixel low and then
    // flips 0 to -1 on every nudge after that. At the ~200ms streaming repaint
    // throttle that is a steady 1px shake with no user input at all.
    const tracker = trackerWithNudge();
    tracker.switchContainer(0, 'thread');
    tracker.applyScrollDelta(400, 2000, 500); // hide the header
    tracker.applyScrollDelta(300, 2000, 500); // scroll up 100 to reveal it fully
    expect(tracker.headerOffset).toBe(0);

    for (let i = 0; i < 20; i++) nudgeRoundTrip(tracker, 300);

    expect(tracker.headerOffset).toBe(0);
  });

  it('folds a real scroll that races the nudge into the next event', () => {
    // Why the skip must NOT advance prevScrollTop. The window can close over a
    // genuine scroll (a fling starting as a nudge lands). Leaving the baseline
    // alone defers that distance to the next event; advancing it would drop the
    // movement on the floor and leave the header out of sync with the content.
    const tracker = trackerWithNudge();
    tracker.switchContainer(0, 'thread');
    tracker.applyScrollDelta(400, 2000, 500);
    tracker.applyScrollDelta(300, 2000, 500); // header fully visible at 300
    expect(tracker.headerOffset).toBe(0);

    nudging = true;
    tracker.applyScrollDelta(400, 2000, 500); // user scrolls 100px inside the window
    nudging = false;
    tracker.applyScrollDelta(400, 2000, 500); // next event, same position

    expect(tracker.headerOffset).toBe(-48); // the 100px still hid the header
  });

  it('still tracks a real scroll once the window closes', () => {
    const tracker = trackerWithNudge();
    tracker.switchContainer(0, 'thread');
    nudgeRoundTrip(tracker, 1);

    tracker.applyScrollDelta(30, 2000, 500);
    expect(tracker.headerOffset).toBe(-30);
  });

  it('never suppresses a live drag, even inside the nudge window', () => {
    // The two gates are duals: forceWebKitRepaint refuses to WRITE a nudge while
    // isUserScrolling(), so a scroll event arriving during a drag is the user's.
    // Suppressing it would only make the header lag the finger, and lastNudgeAt
    // is module-global, so a repaint of a DIFFERENT pane must not be able to
    // freeze this pane's header mid-gesture.
    const tracker = trackerWithNudge();
    tracker.switchContainer(0, 'thread');

    nudging = true;      // a nudge landed a moment ago (on some container)
    userScrolling = true; // ...and the user is now dragging
    tracker.applyScrollDelta(30, 2000, 500);

    expect(tracker.headerOffset).toBe(-30); // tracked the finger, not suppressed
  });

  it('suppresses again once the drag window lapses', () => {
    // The bypass must not latch: at rest is exactly where the shake lives.
    const tracker = trackerWithNudge();
    tracker.switchContainer(0, 'thread');
    userScrolling = true;
    tracker.applyScrollDelta(400, 2000, 500);
    expect(tracker.headerOffset).toBe(-48);

    userScrolling = false;
    nudgeRoundTrip(tracker, 400, () => {
      expect(tracker.headerOffset).toBe(-48);
    });
    expect(tracker.headerOffset).toBe(-48);
  });
});

describe('useHideOnScroll DOM mutation correction', () => {
  let mockActive: { tagName: string; pane?: string } | null = null;
  let tracker: ReturnType<typeof createScrollTracker>;

  beforeEach(() => {
    mockActive = null;
    tracker = createScrollTracker(() => mockActive);
  });

  it('recovers header when content shrinks and scrollTop drops near top', () => {
    tracker.switchContainer(0, 'thread');

    // User scrolls down — header hides
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48); // fully hidden

    // Content shrinks (e.g. steps collapsed), scrollTop clamped to 0.
    // MutationObserver fires correctForScrollPosition — header should recover.
    tracker.correctForScrollPosition(0);
    expect(tracker.headerOffset).toBe(0);
  });

  it('recovers header after keyboard dismiss when content is short', () => {
    tracker.switchContainer(0, 'thread');

    // User scrolls to bottom
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(500, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);

    // Keyboard dismiss sets header based on scroll position
    tracker.syncToScroll(500);
    expect(tracker.headerOffset).toBe(-48); // still hidden (scrollPos > cachedHeight)

    // Content re-renders shorter, scrollTop now 0. DOM mutation fires.
    tracker.correctForScrollPosition(0);
    expect(tracker.headerOffset).toBe(0); // recovered
  });

  it('does not force header visible when user is scrolled past header height', () => {
    tracker.switchContainer(0, 'thread');

    // User scrolls down past header height
    tracker.applyScrollDelta(0, 2000, 500);
    tracker.applyScrollDelta(200, 2000, 500);
    expect(tracker.headerOffset).toBe(-48);

    // DOM mutation fires but scrollTop is still high — header stays hidden
    tracker.correctForScrollPosition(200);
    expect(tracker.headerOffset).toBe(-48);
  });

  it('partially recovers header when scrollTop is between 0 and cachedHeight', () => {
    tracker.switchContainer(0, 'thread');

    // Hide header fully
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);

    // Content shrinks, scrollTop settles at 20 (within header height)
    tracker.correctForScrollPosition(20);
    expect(tracker.headerOffset).toBe(-20); // partially visible
  });
});

describe('useHideOnScroll title bar height var', () => {
  /** Pure logic: convert measured title-bar height (px) to a CSS var value (rem),
   *  or return null when no measurable title bar (drives the chevron position
   *  fallback to 0). Mirrors updateTitleBarHeightVar in useHideOnScroll.ts. */
  function computeTitleBarHeightVar(heightPx: number, remSize: number): string | null {
    if (heightPx <= 0) return null;
    return `${heightPx / remSize}rem`;
  }

  it('single-line title (44px) converts to rem at 16px base', () => {
    expect(computeTitleBarHeightVar(44, 16)).toBe(`${44 / 16}rem`);
  });

  it('multiline title (80px) converts to rem — chevron must track wrapped height', () => {
    // A two-line title measures ~5rem (80px @ 16px). The chevron must use this
    // real height, not the 2.75rem single-line fallback.
    expect(computeTitleBarHeightVar(80, 16)).toBe(`${80 / 16}rem`);
  });

  it('mobile rem base (18px) — same px height yields smaller rem value', () => {
    // Mobile base font is 112.5% (18px), so 80px = 4.444rem not 5rem
    expect(computeTitleBarHeightVar(80, 18)).toBe(`${80 / 18}rem`);
  });

  it('returns null when title bar is absent (chevron uses fallback 0)', () => {
    expect(computeTitleBarHeightVar(0, 16)).toBe(null);
  });
});

describe('useHideOnScroll iOS keyboard VV offset', () => {
  /** Pure logic: compute header top from visualViewport.offsetTop.
   *  Mirrors onVVScroll in useHideOnScroll.ts. */
  function computeHeaderTop(vvOffsetTop: number, remSize: number): string {
    return vvOffsetTop > 0 ? `${vvOffsetTop / remSize}rem` : '';
  }

  it('adjusts header top when iOS scrolls visual viewport down', () => {
    // iOS scrolled layout viewport 50px to show focused input
    expect(computeHeaderTop(50, 18)).toBe(`${50 / 18}rem`);
  });

  it('no adjustment when visual viewport is at top', () => {
    expect(computeHeaderTop(0, 18)).toBe('');
  });

  it('resets to empty string (no inline style) when keyboard closes', () => {
    // After focusout, header.style.top is reset to ''
    // This test documents the expected reset value
    expect(computeHeaderTop(0, 16)).toBe('');
  });
});

describe('useHideOnScroll stale keyboard recovery', () => {
  let mockActive: { tagName: string; pane?: string } | null = null;
  let tracker: ReturnType<typeof createScrollTracker>;

  beforeEach(() => {
    mockActive = null;
    tracker = createScrollTracker(() => mockActive);
  });

  it('header stays permanently hidden when keyboardOpen is stale (iOS swipe bug)', () => {
    tracker.switchContainer(0, 'thread');

    // User focuses prompt input — keyboard opens, header hides
    tracker.setKeyboardOpen(true);
    tracker.onFocusIn(0);
    expect(tracker.getEffectiveOffset()).toBe(-48); // hidden

    // User swipes to threads pane — iOS Safari doesn't fire focusout
    // keyboardOpen stays true (no blur event)
    tracker.switchContainer(0, 'threads');

    // Without recovery: header is STILL hidden because keyboardOpen overrides
    expect(tracker.keyboardOpen).toBe(true);
    expect(tracker.getEffectiveOffset()).toBe(-48); // BUG: stuck hidden

    // User scrolls in threads list — header should reveal but can't
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(50, 1000, 500);
    tracker.applyScrollDelta(20, 1000, 500); // scroll back up 30px
    // headerOffset moved to -18 from scroll delta, but getEffectiveOffset
    // ignores it because keyboardOpen is true
    expect(tracker.headerOffset).toBe(-18);
    expect(tracker.getEffectiveOffset()).toBe(-48); // BUG: still stuck

    // FIX: recoverKeyboardState detects no text input focused, resets flag
    tracker.recoverKeyboardState(false); // no text input focused
    expect(tracker.keyboardOpen).toBe(false);
    expect(tracker.getEffectiveOffset()).toBe(-18); // recovered!
  });

  it('does not reset keyboardOpen when text input is still focused', () => {
    tracker.switchContainer(0, 'thread');
    tracker.setKeyboardOpen(true);
    tracker.onFocusIn(0);

    // Text input is still focused — keyboard is genuinely open
    tracker.recoverKeyboardState(true);
    expect(tracker.keyboardOpen).toBe(true);
    expect(tracker.getEffectiveOffset()).toBe(-48); // correctly hidden
  });

  it('recovers header on pane switch when focusout was missed', () => {
    tracker.switchContainer(0, 'thread');
    tracker.setKeyboardOpen(true);
    tracker.onFocusIn(0);

    // Swipe to threads — iOS misses focusout, but no input is focused
    tracker.switchContainer(0, 'threads');
    tracker.recoverKeyboardState(false);

    // Header should be fully visible at scrollTop=0
    expect(tracker.keyboardOpen).toBe(false);
    expect(tracker.headerOffset).toBe(0);
    expect(tracker.getEffectiveOffset()).toBe(0);
  });
});

describe('useHideOnScroll app UI disabled mode', () => {
  let mockActive: { tagName: string; pane?: string } | null = null;
  let tracker: ReturnType<typeof createScrollTracker>;

  beforeEach(() => {
    mockActive = null;
    tracker = createScrollTracker(() => mockActive);
  });

  it('returns 0 (fully visible) when disabled, even if header was hidden by scroll', () => {
    tracker.switchContainer(0, 'content');

    // Scroll down to hide header
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);
    expect(tracker.getEffectiveOffset()).toBe(-48);

    // App UI becomes active — disable hide-on-scroll
    tracker.setDisabled(true);
    expect(tracker.getEffectiveOffset()).toBe(0); // always visible
  });

  it('ignores scroll events while disabled (effective offset stays 0)', () => {
    tracker.switchContainer(0, 'content');
    tracker.setDisabled(true);

    // Scroll happens inside iframe (propagated to parent or native jitter)
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(50, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);

    // headerOffset tracks internally but effective is always 0
    expect(tracker.getEffectiveOffset()).toBe(0);
  });

  it('re-enables scroll tracking when disabled is cleared', () => {
    tracker.switchContainer(0, 'content');

    // Disable for app UI
    tracker.setDisabled(true);
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.getEffectiveOffset()).toBe(0);

    // App UI closed — re-enable
    tracker.setDisabled(false);
    // Header should reflect the scroll state that accumulated
    expect(tracker.getEffectiveOffset()).toBe(-48);
  });

  it('disabled takes priority over keyboard state', () => {
    tracker.switchContainer(0, 'content');
    tracker.setKeyboardOpen(true);
    tracker.setDisabled(true);

    // Both keyboard and disabled — disabled wins (header visible)
    expect(tracker.getEffectiveOffset()).toBe(0);
  });

  it('clears stale keyboard state when entering disabled mode', () => {
    tracker.switchContainer(0, 'content');

    // Keyboard open (iOS may miss focusout when navigating to app UI)
    tracker.setKeyboardOpen(true);
    expect(tracker.keyboardOpen).toBe(true);

    // App UI opens — disabled should reset keyboardOpen
    tracker.setDisabled(true);
    // The real hook resets keyboardOpen in the overlay subscription;
    // the test tracker doesn't auto-reset, but verifies the priority:
    // disabled takes precedence regardless of keyboardOpen state
    expect(tracker.getEffectiveOffset()).toBe(0);
  });
});

/**
 * iOS dynamic-island / safe-area clearance during keyboard open.
 *
 * Repro for: editing a device title (or any input near the top of a settings
 * list) on an iOS PWA puts the focused input behind the dynamic island.
 *
 * Cause: onFocusIn shrinks the spacer (--mobile-header-height) to fill the
 * hidden-header space, and subtracts the same delta from scrollTop to keep
 * content visually anchored. When the user is at scrollTop=0, the subtraction
 * clamps and content shifts up — by the full header height in the old
 * behavior, leaving an input at content-y=0 at layout-y=0, behind the
 * dynamic island.
 *
 * Fix: shrink the spacer only down to env(safe-area-inset-top), not 0, and
 * subtract the same smaller delta from scrollTop. The first row stays clear
 * of the dynamic island and scrolled-down inputs still anchor cleanly.
 */
function simulateFocusCompensation(opts: {
  cachedHeight: number;
  safeAreaTop: number;
  scrollTop: number;
  firstInputContentY: number;
}) {
  const { cachedHeight, safeAreaTop, scrollTop, firstInputContentY } = opts;
  const delta = cachedHeight - safeAreaTop;
  const newScrollTop = Math.max(0, scrollTop - delta);
  // Content shifts up by `delta` in content-space when the spacer shrinks.
  const newContentY = firstInputContentY - delta;
  // Layout-y where the input ends up in the viewport.
  const layoutY = newContentY - newScrollTop;
  return { newScrollTop, layoutY };
}

describe('useHideOnScroll keyboard open compensation respects safe-area', () => {
  // iPhone-with-dynamic-island numbers: ~3rem header content + ~50px safe area.
  const cachedHeight = 94;
  const safeAreaTop = 50;

  it('first input (at top of scroll) lands at safe-area-top, not behind dynamic island', () => {
    // First device row is at content-y = cachedHeight (just past the spacer).
    // User is at scrollTop = 0 (top of the devices list).
    const { newScrollTop, layoutY } = simulateFocusCompensation({
      cachedHeight,
      safeAreaTop,
      scrollTop: 0,
      firstInputContentY: cachedHeight,
    });

    expect(newScrollTop).toBe(0); // can't scroll above the top
    // Without the fix (safeAreaTop = 0), layoutY would be 0 — behind dynamic island.
    expect(layoutY).toBe(safeAreaTop);
  });

  it('scrolled-down input stays anchored to its pre-focus layout position', () => {
    // Input deep in the list — user scrolled well past the header.
    // The compensation delta must match the spacer delta so layout-y doesn't jump.
    const scrollTop = 500;
    const inputContentY = 600; // input at layout-y = 100 pre-focus

    const { newScrollTop, layoutY } = simulateFocusCompensation({
      cachedHeight,
      safeAreaTop,
      scrollTop,
      firstInputContentY: inputContentY,
    });

    // scrollTop drops by (cachedHeight - safeAreaTop) so the input visually stays put.
    expect(newScrollTop).toBe(scrollTop - (cachedHeight - safeAreaTop));
    expect(layoutY).toBe(inputContentY - scrollTop); // unchanged
  });

  it('no-op on platforms without a safe area (safeAreaTop = 0)', () => {
    // Android, pre-notch iPhones, desktop emulation — compensation matches
    // the old behaviour (subtract full cachedHeight, content shifts up by it).
    const { newScrollTop, layoutY } = simulateFocusCompensation({
      cachedHeight,
      safeAreaTop: 0,
      scrollTop: 0,
      firstInputContentY: cachedHeight,
    });

    expect(newScrollTop).toBe(0);
    expect(layoutY).toBe(0); // matches pre-fix behaviour — no regression
  });
});

/**
 * Spec for the recovery contract: flipping keyboardOpen alone isn't enough —
 * the visual header.style.transform must be re-applied or the header stays
 * stuck at the keyboard-open offset, invisible above the viewport.
 */
function createTransformTracker() {
  const cachedHeight = 48;
  let keyboardOpen = false;
  let headerOffset = 0;
  // Mirror of header.style.transform — only updated when applyTransform runs.
  let appliedOffset: number | null = null;

  const clampOffset = (o: number) => Math.min(0, Math.max(-cachedHeight, o)) || 0;

  function applyTransform() {
    appliedOffset = keyboardOpen ? -cachedHeight : headerOffset;
  }

  function syncToScroll(scrollTop: number) {
    headerOffset = clampOffset(-Math.max(0, scrollTop));
    applyTransform();
  }

  function onFocusIn() {
    keyboardOpen = true;
    applyTransform();
  }

  function recoverKeyboardState(isInputFocused: boolean, scrollTop: number) {
    if (!keyboardOpen) return;
    if (isInputFocused) return;
    keyboardOpen = false;
    syncToScroll(scrollTop);
  }

  return {
    get appliedOffset() { return appliedOffset; },
    get keyboardOpen() { return keyboardOpen; },
    onFocusIn,
    recoverKeyboardState,
  };
}

describe('shouldKeepHeaderVisible', () => {
  // The bug this guards against: opening an app pinned the header on every
  // pane (threads, chat, content) because the gate was `overlay === 'app-ui'`
  // alone. The fix narrows it to the content pane where the iframe lives —
  // on threads/chat panes the iframe is off-screen, so its scroll events
  // can't reach the parent and the header should hide normally.
  it('app-ui overlay forces visible on the content pane', () => {
    expect(shouldKeepHeaderVisible({ view: 'content', overlayType: 'app-ui', stickyPref: false })).toBe(true);
  });

  it('app-ui overlay does NOT force visible on the threads pane', () => {
    expect(shouldKeepHeaderVisible({ view: 'threads', overlayType: 'app-ui', stickyPref: false })).toBe(false);
  });

  it('app-ui overlay does NOT force visible on the chat pane', () => {
    expect(shouldKeepHeaderVisible({ view: 'thread', overlayType: 'app-ui', stickyPref: false })).toBe(false);
  });

  it('non-app-ui overlay on content pane does NOT force visible', () => {
    expect(shouldKeepHeaderVisible({ view: 'content', overlayType: 'url-preview', stickyPref: false })).toBe(false);
  });

  it('no overlay on any pane does NOT force visible', () => {
    expect(shouldKeepHeaderVisible({ view: 'threads', overlayType: null, stickyPref: false })).toBe(false);
    expect(shouldKeepHeaderVisible({ view: 'thread', overlayType: null, stickyPref: false })).toBe(false);
    expect(shouldKeepHeaderVisible({ view: 'content', overlayType: null, stickyPref: false })).toBe(false);
  });

  it('sticky preference forces visible regardless of pane or overlay', () => {
    expect(shouldKeepHeaderVisible({ view: 'threads', overlayType: null, stickyPref: true })).toBe(true);
    expect(shouldKeepHeaderVisible({ view: 'thread', overlayType: null, stickyPref: true })).toBe(true);
    expect(shouldKeepHeaderVisible({ view: 'content', overlayType: 'app-ui', stickyPref: true })).toBe(true);
    expect(shouldKeepHeaderVisible({ view: 'content', overlayType: 'url-preview', stickyPref: true })).toBe(true);
  });
});

/**
 * Repro for: with "Keep header visible" (sticky) on, editing a device name on
 * an iOS PWA rendered the input UNDER the still-visible header.
 *
 * Cause: the spacer (--mobile-header-height) collapsed to the safe-area inset
 * on focus regardless of whether the header was pinned. When the header is
 * pinned it stays at full height, so a collapsed spacer slides content up
 * behind it. The keyboard-open collapse must apply ONLY when the header is
 * actually sliding off (not pinned).
 */
describe('spacerHeightPx', () => {
  const cachedHeight = 94;
  const safeAreaTop = 50;

  it('collapses to safe-area-top when keyboard opens and header is NOT pinned', () => {
    expect(spacerHeightPx({ cachedHeight, safeAreaTop, keyboardOpen: true, disabled: false }))
      .toBe(safeAreaTop);
  });

  it('stays full height when header is pinned, even with keyboard open', () => {
    // The fix: pinned header → spacer must not collapse, or content slides
    // under the header (the device-name-under-header bug).
    expect(spacerHeightPx({ cachedHeight, safeAreaTop, keyboardOpen: true, disabled: true }))
      .toBe(cachedHeight);
  });

  it('stays full height when keyboard is closed', () => {
    expect(spacerHeightPx({ cachedHeight, safeAreaTop, keyboardOpen: false, disabled: false }))
      .toBe(cachedHeight);
    expect(spacerHeightPx({ cachedHeight, safeAreaTop, keyboardOpen: false, disabled: true }))
      .toBe(cachedHeight);
  });
});

describe('useHideOnScroll recovery re-applies header transform', () => {
  it('restores header at scrollTop=0 (iOS missed focusout, user at top)', () => {
    const t = createTransformTracker();

    t.onFocusIn();
    expect(t.appliedOffset).toBe(-48);

    t.recoverKeyboardState(false, 0);

    expect(t.keyboardOpen).toBe(false);
    expect(t.appliedOffset).toBe(0);
  });

  it('keeps header hidden when scrolled past it after recovery', () => {
    const t = createTransformTracker();

    t.onFocusIn();
    t.recoverKeyboardState(false, 100);

    expect(t.appliedOffset).toBe(-48);
  });

  it('leaves transform untouched when an input is still focused', () => {
    const t = createTransformTracker();

    t.onFocusIn();
    const before = t.appliedOffset;
    t.recoverKeyboardState(true, 0);

    expect(t.keyboardOpen).toBe(true);
    expect(t.appliedOffset).toBe(before);
  });
});

describe('useHideOnScroll across an anchored reveal correction', () => {
  /* The step-log toggle changes the height of every turn. `withScrollAnchor`
   * re-bases `scrollTop` by whatever grew or went away above the reader, so
   * their own line does not move. That re-base is not the reader scrolling, and
   * the header must not spend it.
   *
   * It is the one navigation that must NOT reveal the header either. The other
   * three land content on `.chat-exchange`'s `scroll-margin-top`, which clears
   * a VISIBLE header. This one lands the reader exactly where they already
   * were, so revealing covers their own line by a header plus a thread title.
   *
   * Both halves are pinned, because each covers a case the other cannot. The
   * FLAG covers the scroll event that arrives inside the navigation window. The
   * RE-BASE covers the one that does not, which is a large programmatic jump on
   * WebKit under load.
   */
  let anchoring = false;
  let navigating = false;

  beforeEach(() => {
    anchoring = false;
    navigating = false;
  });

  function tracker() {
    return createScrollTracker(
      () => null,
      () => navigating,
      () => false,
      () => false,
      () => anchoring,
    );
  }

  it('keeps the header hidden when the correction scrolls the reader up', () => {
    const t = tracker();
    t.switchContainer(0, 'thread');
    t.applyScrollDelta(3000, 12000, 800);
    expect(t.headerOffset).toBe(-48);

    // Hiding the steps takes 940px out of the transcript above the reader, and
    // the correction takes the same out of `scrollTop`.
    navigating = true;
    anchoring = true;
    t.applyScrollDelta(2060, 8000, 800);

    expect(t.headerOffset).toBe(-48);
  });

  it('still reveals the header for a navigation that is not an anchor', () => {
    const t = tracker();
    t.switchContainer(0, 'thread');
    t.applyScrollDelta(3000, 12000, 800);
    expect(t.headerOffset).toBe(-48);

    navigating = true;
    t.applyScrollDelta(2060, 12000, 800);

    expect(t.headerOffset).toBe(0);
  });

  it('spends nothing when the correction has already re-based the baseline', () => {
    // The scroll event arrives after the navigation window closed, so the flag
    // above says nothing. The re-base is what makes the delta zero.
    const t = tracker();
    t.switchContainer(0, 'thread');
    t.applyScrollDelta(3000, 12000, 800);
    expect(t.headerOffset).toBe(-48);

    t.rebaseAnchor(2060, 8000, 800);
    t.applyScrollDelta(2060, 8000, 800);

    expect(t.headerOffset).toBe(-48);
  });

  it('reads the reader own scroll after the re-base from the new baseline', () => {
    const t = tracker();
    t.switchContainer(0, 'thread');
    t.applyScrollDelta(3000, 12000, 800);

    t.rebaseAnchor(2060, 8000, 800);
    // The reader now scrolls UP 30px of their own accord, which reveals 30px.
    t.applyScrollDelta(2030, 8000, 800);

    expect(t.headerOffset).toBe(-18);
  });
});
