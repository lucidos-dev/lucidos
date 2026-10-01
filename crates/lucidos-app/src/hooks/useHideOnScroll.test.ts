import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import {
  BARS_SHOWN,
  headerOffsetPx,
  landWithoutGlide,
  nextBarsIntent,
  promptCanSlide,
  promptOffsetPx,
  shouldKeepHeaderVisible,
  countsAsReaderScroll,
  spacerHeightPx,
  type BarsIntent,
} from './useHideOnScroll';

/** The travel threshold these tests use, in px. */
const THRESHOLD = 12;

describe('nextBarsIntent (dynamic bars)', () => {
  function fold(deltas: number[], from: BarsIntent = BARS_SHOWN): BarsIntent {
    return deltas.reduce((intent, delta) => nextBarsIntent(intent, delta, THRESHOLD), from);
  }

  it('sends the bars away after a threshold of travel down, and back after one up', () => {
    const away = fold([THRESHOLD]);
    expect(away.away).toBe(true);
    expect(fold([-THRESHOLD], away).away).toBe(false);
  });

  it('adds up travel in one direction across events', () => {
    expect(fold([5, 5]).away).toBe(false);
    expect(fold([5, 5, 5]).away).toBe(true);
  });

  it('never flips on jitter, since a reversal starts the travel afresh', () => {
    expect(fold([8, -8, 8, -8, 8]).away).toBe(false);
    const away = fold([100]);
    expect(fold([-8, 8, -8, 8, -8], away).away).toBe(true);
  });

  it('changes nothing for a zero delta', () => {
    const away = fold([100]);
    expect(nextBarsIntent(away, 0, THRESHOLD)).toBe(away);
  });
});

describe('headerOffsetPx (dynamic bars)', () => {
  const chromeHeight = 90;

  it('moves the header and title bar their whole height away, or not at all', () => {
    expect(headerOffsetPx({ away: true, scrollTop: 500, chromeHeight })).toBe(-chromeHeight);
    expect(headerOffsetPx({ away: false, scrollTop: 500, chromeHeight })).toBe(0);
  });

  it('keeps them shown within their own height of the top', () => {
    // Away there, the top of the pane would show an empty band where they were.
    expect(headerOffsetPx({ away: true, scrollTop: 0, chromeHeight })).toBe(0);
    expect(headerOffsetPx({ away: true, scrollTop: chromeHeight, chromeHeight })).toBe(0);
    expect(headerOffsetPx({ away: true, scrollTop: chromeHeight + 1, chromeHeight })).toBe(-chromeHeight);
  });
});

describe('promptOffsetPx (dynamic bars)', () => {
  const promptHeight = 80;
  const far = 10_000;

  it('moves the prompt its whole height away, or not at all', () => {
    expect(promptOffsetPx({ away: true, promptHeight, distanceToBottom: far })).toBe(promptHeight);
    expect(promptOffsetPx({ away: false, promptHeight, distanceToBottom: far })).toBe(0);
  });

  it('keeps it shown within its own height of the bottom, whoever put the reader there', () => {
    expect(promptOffsetPx({ away: true, promptHeight, distanceToBottom: promptHeight })).toBe(0);
    expect(promptOffsetPx({ away: true, promptHeight, distanceToBottom: 0 })).toBe(0);
    expect(promptOffsetPx({ away: true, promptHeight, distanceToBottom: promptHeight + 1 })).toBe(promptHeight);
  });
});

describe('countsAsReaderScroll (dynamic bars)', () => {
  it("counts the reader's own scroll", () => {
    expect(countsAsReaderScroll({ navigation: false, headerPinned: false })).toBe(true);
  });

  it('re-bases for every write of ours, and while a deep link pins the header', () => {
    expect(countsAsReaderScroll({ navigation: true, headerPinned: false })).toBe(false);
    expect(countsAsReaderScroll({ navigation: false, headerPinned: true })).toBe(false);
  });
});

describe('promptCanSlide (dynamic bars)', () => {
  it('slides only with the bars dynamic, no keyboard, and a real thread', () => {
    expect(promptCanSlide({ disabled: false, keyboardOpen: false, composeEmpty: false })).toBe(true);
  });

  it('stays shown while pinned, while typing, and in the compose-empty view', () => {
    expect(promptCanSlide({ disabled: true, keyboardOpen: false, composeEmpty: false })).toBe(false);
    expect(promptCanSlide({ disabled: false, keyboardOpen: true, composeEmpty: false })).toBe(false);
    expect(promptCanSlide({ disabled: false, keyboardOpen: false, composeEmpty: true })).toBe(false);
  });
});

describe('--mobile-header-offset stays off the document root', () => {
  // Custom properties inherit, so writing the var on `documentElement`
  // invalidates style for every node in the document. The thread transcript is
  // the largest tree in the app, so the var goes on its two consumers instead.
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

  const mobileCss = readFileSync(
    resolve(dirname(fileURLToPath(import.meta.url)), '../styles/mobile.css'),
    'utf8',
  );

  it('moves the down chevron with the prompt by translate, never by a layout property', () => {
    // Only the prompt's HEIGHT, which moves on a resize, may reach `bottom`.
    const offsetUses = mobileCss.split('\n').filter((line: string) => line.includes('--mobile-prompt-offset'));
    expect(offsetUses.length).toBeGreaterThan(0);
    for (const line of offsetUses) expect(line).toMatch(/^\s*translate: 0 var\(--mobile-prompt-offset/);
  });

  it('keeps the prompt offset off the root too, on the prompt and the down chevron', () => {
    // The prompt takes a translate directly; the down chevron reads the var.
    expect(hookSource).not.toMatch(/documentElement\.style\.setProperty\(\s*['"]--mobile-prompt-offset/);
    expect(hookSource).toMatch(/promptEl\.style\.translate = promptOffsetRem/);
    expect(hookSource).toMatch(/downChevronEl\?\.style\.setProperty\(\s*['"]--mobile-prompt-offset/);
  });

  it('moves every bar by `translate` alone, never by `transform`', () => {
    // A `transform` write would bypass the glide, and on the title bar it would
    // also carry the repaint-nudge counter into the transition.
    expect(hookSource).not.toMatch(/\.style\.transform\s*=/);
    expect(hookSource).toMatch(/headerRef\.current\.style\.translate = /);
  });

  // The scroll-delta logic below is a hand-written MIRROR of the hook, so these
  // keep the mirror honest about the behaviour it pins.
  it('gates the navigation reveal on the anchor and carry kinds and the anchored position', () => {
    // Our own placements reveal the bars. Anchor writes and the follow's carry
    // hold them where they were: only the reader's finger moves them.
    expect(hookSource).toMatch(/countsAsReaderScroll\(\{\s*navigation: isNavigationScroll\(\),\s*headerPinned:/);
    expect(hookSource).toMatch(/if \(!isAnchorScroll\(\) && !isCarryScroll\(\) && !atRebasedTop\) revealBars\(\);/);
  });

  it('reveals on a pane swipe before it rebinds, so the outgoing bars glide in too', () => {
    expect(hookSource).toMatch(
      /if \(container !== currentContainer\) \{\s*revealBars\(\);\s*applyTransform\(\);\s*\}[\s\S]*?bindOffsetConsumers\(container\)/,
    );
  });

  it('writes scrollTop only through holdAcrossRelayout, so a send landing survives the keyboard', () => {
    // The keyboard compensation runs after a send's landing calls. A bare write
    // reads as the reader scrolling and cancels the landing, so the reader never
    // saw the message they just sent (scroll-follow-the-live-edge.test.ts).
    expect(hookSource).not.toMatch(/\.scrollTop\s*[+-]?=(?!=)/);
    expect(hookSource.match(/holdAcrossRelayout\(currentContainer/g) ?? []).toHaveLength(2);
  });

  it('re-takes its baseline at the anchor write, not on the scroll event', () => {
    expect(hookSource).toMatch(/onRebasedScroll\(\(el\) => \{[\s\S]*prevScrollTop = clampedScrollTop\(el\)/);
  });

  it('re-takes it again on the settling frame, and cancels that frame on teardown', () => {
    // The write is not the end of a reveal. A shrinking transcript is still
    // settling, and the browser's own clamp lands a frame later. Unabsorbed,
    // the header spent that clamp as a full reveal over the line the correction
    // had just held. The mirror below models the synchronous half only, so the
    // frame is pinned here.
    expect(hookSource).toMatch(/rebaseSettleRaf = requestAnimationFrame\(\(\) => \{[\s\S]*prevScrollTop = clampedScrollTop\(el\)/);
    expect(hookSource).toMatch(/if \(rebaseSettleRaf !== null\) cancelAnimationFrame\(rebaseSettleRaf\);[\s\S]*observer\.disconnect\(\)/);
  });

  it('keeps the offset off `top`, which would reinstate the forced layout', () => {
    // Both consumers take the offset on `translate` (composited) so the write
    // cannot dirty layout. A `top` that reads it means the next scroll event's
    // scrollTop read forces a style+layout flush of the whole transcript again.
    const offsetOnTop = mobileCss.match(/top:[^;]*--mobile-header-offset/g) ?? [];
    expect(offsetOnTop).toHaveLength(0);
    const offsetOnTransform = mobileCss.match(/transform:[^;]*--mobile-header-offset/g) ?? [];
    expect(offsetOnTransform).toHaveLength(0);
    expect(mobileCss.match(/translate:[^;]*--mobile-header-offset/g) ?? []).toHaveLength(2);
  });
});

describe('the dynamic bars glide on a compositor transition', () => {
  const read = (rel: string): string =>
    readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), rel), 'utf8');
  const mobileCss = read('../styles/mobile.css');
  const inputCss = read('../styles/chat/input-messages.css');
  const GLIDE = /translate var\(--bars-glide-duration, 0s\) ease/;

  /** The `transition` value of the first rule whose selector ends in `selector`. */
  function transitionOf(css: string, selector: string): string {
    const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const rule = new RegExp(`${escaped}\\s*\\{([^}]*)\\}`).exec(css);
    expect(rule, `no rule for ${selector}`).not.toBeNull();
    return /transition:([^;]*);/.exec(rule![1])?.[1] ?? '';
  }

  it('gives every bar the glide on `translate`', () => {
    expect(transitionOf(mobileCss, '.app-header')).toMatch(GLIDE);
    expect(transitionOf(mobileCss, '.mobile-swipe-pane .mobile-thread-title-row')).toMatch(GLIDE);
    expect(transitionOf(mobileCss, ':root[data-mobile-dynamic-bars] .thread-pane:not(.compose-empty) .prompt-area')).toMatch(GLIDE);
    // Both chevrons, from their shared base rule.
    expect(transitionOf(inputCss, '.scroll-to-top,\n.scroll-to-bottom')).toMatch(GLIDE);
  });

  it('runs the glide only while the bars are dynamic', () => {
    // Pinned bars never move, so they keep no duration. The var is declared
    // under the attribute alone, and every use falls back to 0s.
    const declarations = [mobileCss, inputCss].join('\n').match(/--bars-glide-duration:[^;]*;/g) ?? [];
    expect(declarations).toEqual(['--bars-glide-duration: var(--duration-slow);']);
    expect(mobileCss).toMatch(/:root\[data-mobile-dynamic-bars\] \{\s*--bars-glide-duration: var\(--duration-slow\);\s*\}/);
    const uses = [mobileCss, inputCss].join('\n').match(/var\(--bars-glide-duration[^)]*\)/g) ?? [];
    for (const use of uses) expect(use).toBe('var(--bars-glide-duration, 0s)');
  });
});

describe('a prompt about to take focus lands without its glide', () => {
  // iOS Safari can refuse the keyboard to a prompt still under a `translate`.
  // A plain reveal leaves it there for the whole glide, so focus must not wait.
  const read = (rel: string): string =>
    readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), rel), 'utf8');

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('flushes style with the transition off, then restores it', () => {
    const el = { style: { transition: '' } } as unknown as HTMLElement;
    const transitionAtFlush: string[] = [];
    vi.stubGlobal('getComputedStyle', (target: HTMLElement) => {
      transitionAtFlush.push(target.style.transition);
      return { translate: 'none' };
    });

    landWithoutGlide(el);

    expect(transitionAtFlush).toEqual(['none']);
    expect(el.style.transition).toBe('');
  });

  it('is what the focus path asks for, before it focuses', () => {
    expect(read('../components/chat/promptFocus.ts')).toMatch(
      /new CustomEvent<RevealBarsDetail>\('reveal-mobile-bars', \{ detail: \{ instant: true \} \}\)\);\s*el\.focus\(/,
    );
    expect(read('useHideOnScroll.ts')).toMatch(/detail\?\.instant\) landWithoutGlide\(promptEl\);/);
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
  isCarryScroll: () => boolean = () => false,
) {
  let prevScrollTop = 0;
  let viewScrollTop = 0;
  let intent = BARS_SHOWN;
  let rebasedTop = -1;
  const cachedHeight = 48;
  let currentPane: string | null = null;
  let keyboardOpen = false;
  let disabled = false;

  function setCurrentPane(pane: string | null) {
    currentPane = pane;
  }

  function applyScrollDelta(scrollTop: number, scrollHeight: number, clientHeight: number) {
    // The real handler returns first thing while the header is pinned.
    if (disabled) return;
    // The iOS compositor-recovery nudge writes ±1px and puts it back a frame
    // later. Skip WITHOUT advancing prevScrollTop, so the round trip leaves the
    // baseline exactly where the user left it. A live drag overrides the window:
    // a nudge is never written while the user scrolls, so suppressing then could
    // only eat the user's own events.
    if (isRepaintNudging() && !isUserScrolling()) return;

    const maxScroll = Math.max(0, scrollHeight - clientHeight);
    const clamped = Math.min(Math.max(0, scrollTop), maxScroll);

    // Within a pixel of where the last anchor write left us. Anywhere else the
    // reader has really moved, so the stamp is spent.
    const atRebasedTop = rebasedTop >= 0 && Math.abs(clamped - rebasedTop) <= 1;
    if (!atRebasedTop) rebasedTop = -1;

    // One of our own navigations is writing scrollTop frame by frame (a chevron
    // tap, turn-nav, a deep-link glide). Reset the header to visible rather than
    // hiding it on the way down: those scroll events are not the reader.
    if (!countsAsReaderScroll({ navigation: isNavigationScroll(), headerPinned: false })) {
      prevScrollTop = clamped;
      viewScrollTop = clamped;
      // An ANCHOR write and the follow's CARRY hold the chrome where the reader
      // left it: neither is a place they asked to go. The kind is read from
      // module state a later mark can overwrite, so the position answers too.
      if (!isAnchorScroll() && !isCarryScroll() && !atRebasedTop) intent = BARS_SHOWN;
      return;
    }

    const active = getActiveElement();
    if (active && (active.tagName === 'TEXTAREA' || active.tagName === 'INPUT' || active.tagName === 'SELECT')) {
      // Only suppress if the focused input is in the same pane as the scroll container
      if (active.pane === currentPane) return;
    }

    intent = nextBarsIntent(intent, clamped - prevScrollTop, THRESHOLD);
    prevScrollTop = clamped;
    viewScrollTop = clamped;
  }

  /** Switch to a new scroll container. A pane swipe reveals the bars. */
  function switchContainer(containerScrollTop: number | null) {
    // Stamped against the container we just left, so it says nothing here.
    rebasedTop = -1;
    intent = BARS_SHOWN;
    prevScrollTop = Math.max(0, containerScrollTop ?? 0);
    viewScrollTop = prevScrollTop;
  }

  /** The app re-based the container without taking the reader anywhere, by an
   *  anchor or carry write (`onRebasedScroll`). Re-take the baseline at the write, so the scroll event
   *  it fires carries a delta of zero whenever it lands, and stamp the position
   *  so the navigation path can recognise that event too. */
  function rebase(scrollTop: number, scrollHeight: number, clientHeight: number) {
    const maxScroll = Math.max(0, scrollHeight - clientHeight);
    prevScrollTop = Math.min(Math.max(0, scrollTop), maxScroll);
    rebasedTop = prevScrollTop;
  }

  /** The keyboard closed: the reader was just typing, so the bars return. */
  function syncToScroll(containerScrollTop: number) {
    prevScrollTop = Math.max(0, containerScrollTop);
    viewScrollTop = prevScrollTop;
    intent = BARS_SHOWN;
  }

  /** The keyboard opens (input gains focus), which hides the header. */
  function onFocusIn(containerScrollTop: number) {
    keyboardOpen = true;
    prevScrollTop = Math.max(0, containerScrollTop);
  }

  /** Re-measure on a DOM mutation: content may have shrunk (e.g. steps
   *  collapsed) under a still reader. */
  function correctForScrollPosition(containerScrollTop: number) {
    viewScrollTop = Math.max(0, containerScrollTop);
  }

  /** Where the header sits by scroll alone, keyboard and pin aside. */
  function headerOffset(): number {
    return headerOffsetPx({ away: intent.away, scrollTop: viewScrollTop, chromeHeight: cachedHeight });
  }

  /** Returns the effective header offset, accounting for keyboard and disabled state.
   *  When disabled (app UI active), always returns 0 (fully visible).
   *  When keyboard is open, always returns fully hidden (-cachedHeight). */
  function getEffectiveOffset(): number {
    if (disabled) return 0;
    return keyboardOpen ? -cachedHeight : headerOffset();
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
    rebase,
    switchContainer,
    syncToScroll,
    onFocusIn,
    correctForScrollPosition,
    setCurrentPane,
    setKeyboardOpen,
    recoverKeyboardState,
    get headerOffset() { return headerOffset(); },
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
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);
  });

  it('ignores scroll when textarea has focus in same pane', () => {
    mockActive = { tagName: 'TEXTAREA', pane: 'threads' };

    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(50, 1000, 500);
    tracker.applyScrollDelta(100, 800, 300);

    expect(tracker.headerOffset).toBe(0);
  });

  it('shows the bars after blur, then hides them on the next scroll down', () => {
    mockActive = { tagName: 'TEXTAREA', pane: 'threads' };

    tracker.applyScrollDelta(50, 1000, 500);
    expect(tracker.headerOffset).toBe(0);

    // Blur: the keyboard closes, and the reader was just typing.
    mockActive = null;
    tracker.syncToScroll(50);
    expect(tracker.headerOffset).toBe(0);

    tracker.applyScrollDelta(80, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);
  });

  it('ignores scroll with input element focused in same pane', () => {
    mockActive = { tagName: 'INPUT', pane: 'threads' };

    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(0);
  });

  it('hides header when input gains focus (keyboard opens)', () => {
    expect(tracker.getEffectiveOffset()).toBe(0);

    // User taps input — keyboard opens, header hides to avoid
    // iOS position:fixed issues with software keyboard
    tracker.onFocusIn(0);
    expect(tracker.getEffectiveOffset()).toBe(-48);
  });

  it('allows scroll when textarea has focus in a DIFFERENT pane (iOS swipe)', () => {
    // Simulates: user was typing in compose view (pane 'thread'),
    // then swiped to threads view (pane 'threads'). iOS Safari may not
    // blur the textarea, but scroll handling must still work in threads pane.
    tracker.setCurrentPane('threads');
    mockActive = { tagName: 'TEXTAREA', pane: 'thread' };

    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48); // header hides despite textarea focus
  });

  it('reveals the bars on a pane swipe, and again on the swipe back', () => {
    tracker.switchContainer(0);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);

    tracker.switchContainer(0);
    expect(tracker.headerOffset).toBe(0);

    // Back to a pane scrolled far down: shown, not the hidden state it left in.
    tracker.switchContainer(400);
    expect(tracker.headerOffset).toBe(0);
  });

  it('keeps the bars shown on arrival until the reader scrolls down', () => {
    tracker.switchContainer(200);
    expect(tracker.headerOffset).toBe(0);

    tracker.applyScrollDelta(230, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);

    tracker.applyScrollDelta(210, 1000, 500);
    expect(tracker.headerOffset).toBe(0);
  });

  it('resets header when container disappears (null)', () => {
    tracker.switchContainer(0);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);

    // Container removed from DOM — header must reset to visible, not stay hidden
    tracker.switchContainer(null);
    expect(tracker.headerOffset).toBe(0);
  });

  it('keeps header visible when switching to a container at top', () => {
    expect(tracker.headerOffset).toBe(0);
    tracker.switchContainer(0);
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
    tracker.switchContainer(0);

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
    tracker.switchContainer(0);

    navigating = true;
    tracker.applyScrollDelta(1500, 2000, 500); // scrollTop=1500, maxScroll=1500
    expect(tracker.headerOffset).toBe(0);

    navigating = false; // the tween finished

    // User scrolls up a bit, then back down — header hides on the down scroll
    tracker.applyScrollDelta(1470, 2000, 500); // scroll up 30px → header stays shown
    tracker.applyScrollDelta(1500, 2000, 500); // scroll down 30px → header away
    expect(tracker.headerOffset).toBe(-48);
  });

  it('keeps the header visible across a multi-frame glide', () => {
    const tracker = createScrollTracker(() => mockActive, () => navigating);

    navigating = true;
    tracker.switchContainer(0);

    // Each frame of the tween fires a scroll event.
    tracker.applyScrollDelta(500, 1000, 500);
    expect(tracker.headerOffset).toBe(0); // visible

    tracker.applyScrollDelta(1500, 2000, 500);
    expect(tracker.headerOffset).toBe(0); // still visible

    navigating = false;

    // User scrolls up then down — header hides on down scroll
    tracker.applyScrollDelta(1470, 2000, 500);
    tracker.applyScrollDelta(1500, 2000, 500);
    expect(tracker.headerOffset).toBe(-48);
  });
});

describe("useHideOnScroll under the standing follow's carry", () => {
  // The ride writes the live edge on every growth round. Only the reader's
  // finger moves the bars. So a carry leaves them where they were, shown or
  // away, and the next finger delta starts from where the carry left off.
  let carrying = false;
  beforeEach(() => { carrying = false; });

  function trackerUnderRide() {
    return createScrollTracker(() => null, () => carrying, () => false, () => false, () => false, () => carrying);
  }

  it('keeps shown bars shown as the ride carries the reader down', () => {
    const tracker = trackerUnderRide();
    tracker.switchContainer(1000);
    carrying = true;
    tracker.applyScrollDelta(1100, 3000, 500);
    tracker.applyScrollDelta(1400, 3000, 500);
    expect(tracker.headerOffset).toBe(0);
  });

  it('keeps away bars away as the ride carries the reader down', () => {
    const tracker = trackerUnderRide();
    tracker.switchContainer(1000);
    tracker.applyScrollDelta(1100, 3000, 500);
    expect(tracker.headerOffset).toBe(-48);
    carrying = true;
    tracker.applyScrollDelta(1400, 3000, 500);
    expect(tracker.headerOffset).toBe(-48);
  });

  it('ignores a carry whose scroll event lands after the navigation window', () => {
    // A heavy render can delay the event past the window, so the kind check
    // alone answers "the reader". The re-base at the write zeroes its delta.
    const tracker = trackerUnderRide();
    tracker.switchContainer(1000);
    tracker.rebase(1400, 3000, 500);
    tracker.applyScrollDelta(1400, 3000, 500);
    expect(tracker.headerOffset).toBe(0);
  });

  it("measures the reader's next scroll from where the carry left them", () => {
    const tracker = trackerUnderRide();
    tracker.switchContainer(1000);
    carrying = true;
    tracker.applyScrollDelta(1400, 3000, 500);
    carrying = false;
    // A small nudge down after a 400px carry: under the threshold, so nothing.
    tracker.applyScrollDelta(1405, 3000, 500);
    expect(tracker.headerOffset).toBe(0);
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
    tracker.switchContainer(0);
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
    tracker.switchContainer(0);
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
    tracker.switchContainer(0);
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
    tracker.switchContainer(0);
    nudgeRoundTrip(tracker, 1);

    tracker.applyScrollDelta(100, 2000, 500);
    expect(tracker.headerOffset).toBe(-48);
  });

  it('never suppresses a live drag, even inside the nudge window', () => {
    // The two gates are duals: forceWebKitRepaint refuses to WRITE a nudge while
    // isUserScrolling(), so a scroll event arriving during a drag is the user's.
    // Suppressing it would only make the header lag the finger, and lastNudgeAt
    // is module-global, so a repaint of a DIFFERENT pane must not be able to
    // freeze this pane's header mid-gesture.
    const tracker = trackerWithNudge();
    tracker.switchContainer(0);

    nudging = true;      // a nudge landed a moment ago (on some container)
    userScrolling = true; // ...and the user is now dragging
    tracker.applyScrollDelta(100, 2000, 500);

    expect(tracker.headerOffset).toBe(-48); // followed the finger, not suppressed
  });

  it('suppresses again once the drag window lapses', () => {
    // The bypass must not latch: at rest is exactly where the shake lives.
    const tracker = trackerWithNudge();
    tracker.switchContainer(0);
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
    tracker.switchContainer(0);

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
    tracker.switchContainer(0);

    // User scrolls to bottom
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(500, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);

    // Keyboard dismiss brings the bars back: the reader was just typing
    tracker.syncToScroll(500);
    expect(tracker.headerOffset).toBe(0);
    tracker.applyScrollDelta(600, 1500, 500);
    expect(tracker.headerOffset).toBe(-48);

    // Content re-renders shorter, scrollTop now 0. DOM mutation fires.
    tracker.correctForScrollPosition(0);
    expect(tracker.headerOffset).toBe(0); // recovered
  });

  it('does not force header visible when user is scrolled past header height', () => {
    tracker.switchContainer(0);

    // User scrolls down past header height
    tracker.applyScrollDelta(0, 2000, 500);
    tracker.applyScrollDelta(200, 2000, 500);
    expect(tracker.headerOffset).toBe(-48);

    // DOM mutation fires but scrollTop is still high — header stays hidden
    tracker.correctForScrollPosition(200);
    expect(tracker.headerOffset).toBe(-48);
  });

  it('shows the header once a shrink brings the scroll within its height', () => {
    tracker.switchContainer(0);

    // Hide header fully
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.headerOffset).toBe(-48);

    // Content shrinks, scrollTop settles at 20 (within header height)
    tracker.correctForScrollPosition(20);
    expect(tracker.headerOffset).toBe(0);
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
    tracker.switchContainer(0);

    // User focuses prompt input — keyboard opens, header hides
    tracker.setKeyboardOpen(true);
    tracker.onFocusIn(0);
    expect(tracker.getEffectiveOffset()).toBe(-48); // hidden

    // User swipes to threads pane — iOS Safari doesn't fire focusout
    // keyboardOpen stays true (no blur event)
    tracker.switchContainer(0);

    // Without recovery: header is STILL hidden because keyboardOpen overrides
    expect(tracker.keyboardOpen).toBe(true);
    expect(tracker.getEffectiveOffset()).toBe(-48); // BUG: stuck hidden

    // User scrolls in threads list — header should reveal but can't
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(50, 1000, 500);
    tracker.applyScrollDelta(20, 1000, 500); // scroll back up 30px
    // The scroll brought the header back, but getEffectiveOffset ignores it
    // because keyboardOpen is true
    expect(tracker.headerOffset).toBe(0);
    expect(tracker.getEffectiveOffset()).toBe(-48); // BUG: still stuck

    // FIX: recoverKeyboardState detects no text input focused, resets flag
    tracker.recoverKeyboardState(false); // no text input focused
    expect(tracker.keyboardOpen).toBe(false);
    expect(tracker.getEffectiveOffset()).toBe(0); // recovered!
  });

  it('does not reset keyboardOpen when text input is still focused', () => {
    tracker.switchContainer(0);
    tracker.setKeyboardOpen(true);
    tracker.onFocusIn(0);

    // Text input is still focused — keyboard is genuinely open
    tracker.recoverKeyboardState(true);
    expect(tracker.keyboardOpen).toBe(true);
    expect(tracker.getEffectiveOffset()).toBe(-48); // correctly hidden
  });

  it('recovers header on pane switch when focusout was missed', () => {
    tracker.switchContainer(0);
    tracker.setKeyboardOpen(true);
    tracker.onFocusIn(0);

    // Swipe to threads — iOS misses focusout, but no input is focused
    tracker.switchContainer(0);
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
    tracker.switchContainer(0);

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
    tracker.switchContainer(0);
    tracker.setDisabled(true);

    // Scroll happens inside iframe (propagated to parent or native jitter)
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(50, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);

    // headerOffset tracks internally but effective is always 0
    expect(tracker.getEffectiveOffset()).toBe(0);
  });

  it('picks the scroll back up where it was when disabled is cleared', () => {
    tracker.switchContainer(0);

    // Disable for app UI
    tracker.setDisabled(true);
    tracker.applyScrollDelta(0, 1000, 500);
    tracker.applyScrollDelta(100, 1000, 500);
    expect(tracker.getEffectiveOffset()).toBe(0);

    // App UI closed — re-enable. Nothing accumulated while pinned.
    tracker.setDisabled(false);
    expect(tracker.getEffectiveOffset()).toBe(0);
    tracker.applyScrollDelta(200, 1000, 500);
    expect(tracker.getEffectiveOffset()).toBe(-48);
  });

  it('disabled takes priority over keyboard state', () => {
    tracker.switchContainer(0);
    tracker.setKeyboardOpen(true);
    tracker.setDisabled(true);

    // Both keyboard and disabled — disabled wins (header visible)
    expect(tracker.getEffectiveOffset()).toBe(0);
  });

  it('clears stale keyboard state when entering disabled mode', () => {
    tracker.switchContainer(0);

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
 * the visual header.style.translate must be re-applied or the header stays
 * stuck at the keyboard-open offset, invisible above the viewport.
 */
function createTransformTracker() {
  const cachedHeight = 48;
  let keyboardOpen = false;
  let away = true;
  // Mirror of header.style.translate — only updated when applyTransform runs.
  let appliedOffset: number | null = null;

  function applyTransform() {
    appliedOffset = keyboardOpen ? -cachedHeight : away ? -cachedHeight : 0;
  }

  /** The keyboard closed: the bars return, wherever the reader is. */
  function syncToScroll() {
    away = false;
    applyTransform();
  }

  function onFocusIn() {
    keyboardOpen = true;
    applyTransform();
  }

  function recoverKeyboardState(isInputFocused: boolean) {
    if (!keyboardOpen) return;
    if (isInputFocused) return;
    keyboardOpen = false;
    syncToScroll();
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
    expect(shouldKeepHeaderVisible({ view: 'content', overlayType: 'app-ui', dynamicBars: true })).toBe(true);
  });

  it('app-ui overlay does NOT force visible on the threads pane', () => {
    expect(shouldKeepHeaderVisible({ view: 'threads', overlayType: 'app-ui', dynamicBars: true })).toBe(false);
  });

  it('app-ui overlay does NOT force visible on the chat pane', () => {
    expect(shouldKeepHeaderVisible({ view: 'thread', overlayType: 'app-ui', dynamicBars: true })).toBe(false);
  });

  it('non-app-ui overlay on content pane does NOT force visible', () => {
    expect(shouldKeepHeaderVisible({ view: 'content', overlayType: 'url-preview', dynamicBars: true })).toBe(false);
  });

  it('no overlay on any pane does NOT force visible', () => {
    expect(shouldKeepHeaderVisible({ view: 'threads', overlayType: null, dynamicBars: true })).toBe(false);
    expect(shouldKeepHeaderVisible({ view: 'thread', overlayType: null, dynamicBars: true })).toBe(false);
    expect(shouldKeepHeaderVisible({ view: 'content', overlayType: null, dynamicBars: true })).toBe(false);
  });

  it('pinned bars (dynamic bars off) force visible regardless of pane or overlay', () => {
    expect(shouldKeepHeaderVisible({ view: 'threads', overlayType: null, dynamicBars: false })).toBe(true);
    expect(shouldKeepHeaderVisible({ view: 'thread', overlayType: null, dynamicBars: false })).toBe(true);
    expect(shouldKeepHeaderVisible({ view: 'content', overlayType: 'app-ui', dynamicBars: false })).toBe(true);
    expect(shouldKeepHeaderVisible({ view: 'content', overlayType: 'url-preview', dynamicBars: false })).toBe(true);
  });
});

/**
 * Repro for: with the bars pinned (dynamic bars off), editing a device name on
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
  it('restores the header when iOS missed the focusout', () => {
    const t = createTransformTracker();

    t.onFocusIn();
    expect(t.appliedOffset).toBe(-48);

    t.recoverKeyboardState(false);

    expect(t.keyboardOpen).toBe(false);
    expect(t.appliedOffset).toBe(0);
  });

  it('shows the header after recovery, even scrolled far down', () => {
    // The scroll had sent the bars away before the keyboard opened.
    const t = createTransformTracker();

    t.onFocusIn();
    t.recoverKeyboardState(false);

    expect(t.appliedOffset).toBe(0);
  });

  it('leaves transform untouched when an input is still focused', () => {
    const t = createTransformTracker();

    t.onFocusIn();
    const before = t.appliedOffset;
    t.recoverKeyboardState(true);

    expect(t.keyboardOpen).toBe(true);
    expect(t.appliedOffset).toBe(before);
  });
});

describe('useHideOnScroll across an anchored reveal correction', () => {
  /* The step-log toggle changes the height of every turn. `withScrollAnchor`
   * re-bases `scrollTop` by whatever grew or went away above the control the
   * reader pressed, so that control does not move. That re-base is not the
   * reader scrolling, and the header must not spend it.
   *
   * It is the one navigation that must NOT reveal the header either. The other
   * three land content on `.chat-exchange`'s `scroll-margin-top`, which clears
   * a VISIBLE header. This one lands the reader exactly where they already
   * were, so revealing covers what they pressed by a header and a thread title.
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
    t.switchContainer(0);
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
    t.switchContainer(0);
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
    t.switchContainer(0);
    t.applyScrollDelta(3000, 12000, 800);
    expect(t.headerOffset).toBe(-48);

    t.rebase(2060, 8000, 800);
    t.applyScrollDelta(2060, 8000, 800);

    expect(t.headerOffset).toBe(-48);
  });

  it('reads the reader own scroll after the re-base from the new baseline', () => {
    const t = tracker();
    t.switchContainer(0);
    t.applyScrollDelta(3000, 12000, 800);

    t.rebase(2060, 8000, 800);
    // A small move of the reader's own reads as small. Against the old baseline
    // it would read as 934px up, and reveal.
    t.applyScrollDelta(2066, 8000, 800);
    expect(t.headerOffset).toBe(-48);

    t.applyScrollDelta(2030, 8000, 800);
    expect(t.headerOffset).toBe(0);
  });

  it('keeps the header hidden when a later mark has stolen the anchor kind', () => {
    // The third half, and the one the other two cannot cover. The event lands
    // inside the navigation window, so the re-based baseline is never read.
    // Something marked after the anchor write, so the flag says placement. Only
    // the POSITION still knows: the container sits where the anchor left it.
    const t = tracker();
    t.switchContainer(0);
    t.applyScrollDelta(3000, 12000, 800);
    expect(t.headerOffset).toBe(-48);

    t.rebase(2060, 8000, 800);
    navigating = true;
    anchoring = false;
    t.applyScrollDelta(2060, 8000, 800);

    expect(t.headerOffset).toBe(-48);
  });

  it('spends the 1px repaint nudge without reading it as the reader moving', () => {
    const t = tracker();
    t.switchContainer(0);
    t.applyScrollDelta(3000, 12000, 800);

    t.rebase(2060, 8000, 800);
    navigating = true;
    t.applyScrollDelta(2059, 8000, 800);

    expect(t.headerOffset).toBe(-48);
  });

  it('still reveals for a navigation that lands somewhere else', () => {
    // The stamp must not outlive the position it was taken at. The next chevron
    // tap would find the header stuck wherever the reader left it.
    const t = tracker();
    t.switchContainer(0);
    t.applyScrollDelta(3000, 12000, 800);

    t.rebase(2060, 8000, 800);
    navigating = true;
    t.applyScrollDelta(400, 8000, 800);

    expect(t.headerOffset).toBe(0);
  });

  it('spends the stamp once the reader scrolls off the anchored position', () => {
    const t = tracker();
    t.switchContainer(0);
    t.applyScrollDelta(3000, 12000, 800);

    t.rebase(2060, 8000, 800);
    t.applyScrollDelta(2400, 8000, 800); // the reader moves, of their own accord
    navigating = true;
    t.applyScrollDelta(2060, 8000, 800); // a navigation happens to come back

    expect(t.headerOffset).toBe(0);
  });
});
