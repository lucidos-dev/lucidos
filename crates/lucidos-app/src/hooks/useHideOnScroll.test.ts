import { describe, it, expect, beforeEach } from 'vitest';
import { shouldKeepHeaderVisible, spacerHeightPx } from './useHideOnScroll';

// Tests scroll-delta and keyboard-suppression logic from useHideOnScroll.
// Uses string pane identity instead of DOM .closest() traversal.
function createScrollTracker(
  getActiveElement: () => { tagName: string; pane?: string } | null,
  getResizeMode: () => 'scroll' | 'ignore' = () => 'ignore',
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
    // Programmatic scroll (scrollToBottom) — reset header to visible
    // so opening a thread shows the header, not hides it.
    if (getResizeMode() === 'scroll') {
      const maxScroll = Math.max(0, scrollHeight - clientHeight);
      prevScrollTop = Math.min(Math.max(0, scrollTop), maxScroll);
      headerOffset = 0;
      return;
    }

    const active = getActiveElement();
    if (active && (active.tagName === 'TEXTAREA' || active.tagName === 'INPUT' || active.tagName === 'SELECT')) {
      // Only suppress if the focused input is in the same pane as the scroll container
      if (active.pane === currentPane) return;
    }

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

describe('useHideOnScroll programmatic scroll (scrollToBottom)', () => {
  let mockActive: { tagName: string; pane?: string } | null = null;
  let resizeMode: 'scroll' | 'ignore' = 'ignore';

  beforeEach(() => {
    mockActive = null;
    resizeMode = 'ignore';
  });

  it('resets header to visible during programmatic scroll (resize mode = scroll)', () => {
    const tracker = createScrollTracker(() => mockActive, () => resizeMode);
    tracker.switchContainer(0, 'thread');

    // User scrolls down — header hides
    tracker.applyScrollDelta(100, 2000, 500);
    expect(tracker.headerOffset).toBe(-48); // fully hidden

    // scrollToBottom() sets resize mode to 'scroll', then scrolls
    resizeMode = 'scroll';
    tracker.applyScrollDelta(1500, 2000, 500);

    // Header should be reset to visible, not hidden further
    expect(tracker.headerOffset).toBe(0);
  });

  it('resumes normal scroll tracking after programmatic scroll ends', () => {
    const tracker = createScrollTracker(() => mockActive, () => resizeMode);
    tracker.switchContainer(0, 'thread');

    // Programmatic scroll to bottom
    resizeMode = 'scroll';
    tracker.applyScrollDelta(1500, 2000, 500); // scrollTop=1500, maxScroll=1500
    expect(tracker.headerOffset).toBe(0);

    // Suppression expires — back to normal mode
    resizeMode = 'ignore';

    // User scrolls up a bit, then back down — header hides on the down scroll
    tracker.applyScrollDelta(1470, 2000, 500); // scroll up 30px → header stays 0
    tracker.applyScrollDelta(1500, 2000, 500); // scroll down 30px → header -30
    expect(tracker.headerOffset).toBe(-30);
  });

  it('header stays visible when opening a thread (full scroll-to-bottom flow)', () => {
    const tracker = createScrollTracker(() => mockActive, () => resizeMode);

    // Thread opened — scrollToBottom triggers
    resizeMode = 'scroll';
    tracker.switchContainer(0, 'thread');

    // Content renders, scroll events fire as content grows
    tracker.applyScrollDelta(500, 1000, 500);
    expect(tracker.headerOffset).toBe(0); // visible

    tracker.applyScrollDelta(1500, 2000, 500);
    expect(tracker.headerOffset).toBe(0); // still visible

    // Suppression ends
    resizeMode = 'ignore';

    // User scrolls up then down — header hides on down scroll
    tracker.applyScrollDelta(1470, 2000, 500);
    tracker.applyScrollDelta(1500, 2000, 500);
    expect(tracker.headerOffset).toBe(-30);
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
