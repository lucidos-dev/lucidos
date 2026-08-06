import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

import {
  applyNavFocus,
  clearNavFocus,
  hasNavFocus,
  navFocusElement,
  NAV_FOCUS_FADE_MS,
  NAV_FOCUS_HOLD_MS,
  NAV_FOCUS_RAMP_MS,
} from '../focusMarker';

/** A fake element with a tracked classList — enough for the marker's class
 *  add/remove. */
function makeEl() {
  const classes = new Set<string>();
  const el: any = {
    classList: {
      _classes: classes,
      add: (c: string) => classes.add(c),
      remove: (c: string) => classes.delete(c),
    },
  };
  return el as HTMLElement & { classList: { _classes: Set<string> } };
}

const STUCK = 'nav-focus-stuck';
const FADING = 'nav-focus-fading';

/** Run out the hold. The marker will not begin to dissolve before this
 *  has elapsed however fast the user engages, so any test about dismissal has to get
 *  past it first. Its own behaviour is pinned by the hold tests below. */
function passHold() {
  vi.advanceTimersByTime(NAV_FOCUS_RAMP_MS + NAV_FOCUS_HOLD_MS);
}

beforeEach(() => {
  vi.useFakeTimers();
  clearNavFocus(); // reset module state + tear down any armed listeners
});
afterEach(() => {
  clearNavFocus();
  vi.clearAllTimers();
  vi.useRealTimers();
});

describe('focusMarker', () => {
  it('applies just the sticky highlight class (no fade class)', () => {
    const el = makeEl();
    applyNavFocus(el);
    expect(el.classList._classes.has(STUCK)).toBe(true);
    expect(el.classList._classes.has(FADING)).toBe(false);
    expect(hasNavFocus()).toBe(true);
  });

  it('a second apply supersedes — clears the first element, marks the second', () => {
    const first = makeEl();
    const second = makeEl();
    applyNavFocus(first);
    applyNavFocus(second);

    expect(first.classList._classes.has(STUCK)).toBe(false);
    expect(first.classList._classes.has(FADING)).toBe(false);
    expect(second.classList._classes.has(STUCK)).toBe(true);
    expect(hasNavFocus()).toBe(true);
  });

  it('clearNavFocus() removes the highlight immediately and is idempotent', () => {
    const el = makeEl();
    applyNavFocus(el);
    clearNavFocus();
    expect(el.classList._classes.has(STUCK)).toBe(false);
    expect(el.classList._classes.has(FADING)).toBe(false);
    expect(hasNavFocus()).toBe(false);
    clearNavFocus(); // no throw on a second call
    expect(hasNavFocus()).toBe(false);
  });

  it('a user scroll gesture fades the marker out, then removes it', () => {
    const el = makeEl();
    applyNavFocus(el);
    expect(hasNavFocus()).toBe(true);

    passHold();
    document.dispatchEvent(new Event('wheel'));
    // Dissolve in flight: the fading class is added, the highlight still present.
    expect(el.classList._classes.has(FADING)).toBe(true);
    expect(el.classList._classes.has(STUCK)).toBe(true);
    expect(hasNavFocus()).toBe(true);

    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(el.classList._classes.has(STUCK)).toBe(false);
    expect(el.classList._classes.has(FADING)).toBe(false);
    expect(hasNavFocus()).toBe(false);
  });

  // The visual dissolve and "is this still the current landing?" are two different
  // clocks, and the gap between them is where a bug lived: `navFocusElement()` used to
  // report a dissolving marker as current for the WHOLE dissolve. scrollState.ts's
  // turn-nav anchors index-stepping on the marked turn precisely BECAUSE a marker means
  // the user hasn't scrolled since the last nav, so a stale ref made ⌘↑/⌘↓ step from the
  // turn the user had just scrolled away from. Tuning the dissolve longer widened it.
  // The ref now expires one frame after dismissal however long the dissolve runs, which
  // is the same deadline the reduced-motion path has always used, and is still late
  // enough for the Enter-toggle to read it during the very keydown that dismissed it
  // (ce327ed24).
  it('retires the ref one frame after dismissal, while the highlight dissolves on', () => {
    const el = makeEl();
    applyNavFocus(el);

    passHold();
    document.dispatchEvent(new Event('wheel'));
    // Same tick as the dismissing event: still current, so a bubble-phase handler
    // reacting to that event can act on it.
    //
    // This depends on rAF being QUEUED here, and it is: `vi.useFakeTimers()` above
    // replaces requestAnimationFrame with a timer-driven one for the whole suite.
    // src/test-setup.ts does carry a synchronous rAF stub, which would break this
    // assertion, but it installs only `if (typeof requestAnimationFrame ===
    // 'undefined')` and the fake timers shadow it either way (verified by probe).
    expect(navFocusElement()).toBe(el);

    vi.advanceTimersByTime(20); // one frame
    expect(navFocusElement()).toBeNull();
    expect(hasNavFocus()).toBe(false);
    // ...but the marker is still ON SCREEN, dissolving, for the rest of the duration.
    expect(el.classList._classes.has(STUCK)).toBe(true);
    expect(el.classList._classes.has(FADING)).toBe(true);

    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(el.classList._classes.has(STUCK)).toBe(false);
    expect(el.classList._classes.has(FADING)).toBe(false);
  });

  // Clearing mid-dissolve is the one path where the two clocks could strand state: the
  // ref has already expired, so a `if (_markedEl)`-style cleanup keyed on the accessor
  // would skip the element, and the cancelled timer would never strip its classes.
  it('clearNavFocus() strips the classes of an element already past its ref expiry', () => {
    const el = makeEl();
    applyNavFocus(el);
    passHold();
    document.dispatchEvent(new Event('wheel'));
    vi.advanceTimersByTime(20); // ref expired, classes still on, timer still pending
    expect(navFocusElement()).toBeNull();

    clearNavFocus();
    expect(el.classList._classes.has(STUCK)).toBe(false);
    expect(el.classList._classes.has(FADING)).toBe(false);
  });

  it('a click (pointerdown) dismisses the marker', () => {
    const el = makeEl();
    applyNavFocus(el);

    passHold();
    document.dispatchEvent(new Event('pointerdown'));
    expect(el.classList._classes.has(FADING)).toBe(true);

    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(hasNavFocus()).toBe(false);
  });

  it('any keypress dismisses the marker', () => {
    const el = makeEl();
    applyNavFocus(el);

    // Construct via a generic Event + `.key` so the test doesn't depend on a
    // `KeyboardEvent` global (absent in this test environment).
    const keydown = (key: string) => {
      const e = new Event('keydown') as Event & { key: string };
      e.key = key;
      return e;
    };

    passHold();
    document.dispatchEvent(keydown('a')); // any key, not just scroll keys
    expect(el.classList._classes.has(FADING)).toBe(true);

    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(hasNavFocus()).toBe(false);
  });

  it('a follow-up action during the fade does not throw or re-arm', () => {
    const el = makeEl();
    applyNavFocus(el);

    passHold();
    document.dispatchEvent(new Event('wheel')); // starts the dissolve + tears listeners down
    expect(el.classList._classes.has(FADING)).toBe(true);

    // Advance most of the way through BEFORE the second action. Dispatching it at the
    // same virtual instant as the first would make this test blind to the thing it is
    // named for: a re-arm would cancel and re-schedule at that same timestamp, so one
    // advance of NAV_FOCUS_FADE_MS would still complete it and every assertion would
    // still pass. Acting late means a re-arm pushes completion out of reach.
    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS - 200);
    expect(el.classList._classes.has(FADING)).toBe(true);
    // Listeners are gone, so a second action is a no-op (would-be double fade).
    document.dispatchEvent(new Event('pointerdown'));

    vi.advanceTimersByTime(300); // clears the ORIGINAL deadline, not a re-armed one
    expect(el.classList._classes.has(STUCK)).toBe(false);
    expect(el.classList._classes.has(FADING)).toBe(false);
    expect(hasNavFocus()).toBe(false);
  });

  // Cancellation is not always reachable, so the marker cannot rely on it. A dissolve
  // arms an rAF (ref expiry) and a timer (class removal), and in a HIDDEN TAB they fire
  // out of order: rAF callbacks are starved while setTimeout keeps running. The timer
  // then completes and drops `_teardown`, stranding a queued, uncancellable frame. If
  // that frame guarded on element identity it would still match a legitimately
  // re-marked SAME element and retire its ref, leaving a visible highlight that
  // turn-nav and Enter-toggle both treat as absent, with nothing pending to repair it.
  // A generation token is what makes the stale frame inert.
  it('a frame stranded by a hidden tab cannot retire the next marker', () => {
    const el = makeEl();
    const frames: Array<() => void> = [];
    const raf = vi
      .spyOn(globalThis, 'requestAnimationFrame')
      .mockImplementation(((cb: FrameRequestCallback) => {
        frames.push(() => cb(0));
        return frames.length;
      }) as typeof requestAnimationFrame);

    try {
      applyNavFocus(el);
      passHold();
      document.dispatchEvent(new Event('wheel')); // arms the rAF + the timer

      // Tab hidden: the timer runs to completion with the frame still queued.
      vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
      expect(el.classList._classes.has(STUCK)).toBe(false);
      expect(frames.length).toBe(1); // still pending, and no longer cancellable

      // A fresh navigation lands on the SAME element.
      applyNavFocus(el);
      expect(navFocusElement()).toBe(el);

      // Tab visible again: the stranded frame runs against a marker that is not its own.
      frames.forEach(run => run());
      expect(navFocusElement()).toBe(el);
      expect(el.classList._classes.has(STUCK)).toBe(true);
    } finally {
      raf.mockRestore();
    }
  });

  it('an action is deferred while the settle guard is active, and dismisses once it releases', () => {
    const el = makeEl();
    let settling = true;
    applyNavFocus(el, { settleGuard: () => settling });
    expect(hasNavFocus()).toBe(true);

    // Guard active → the landing scroll's gesture is ignored, marker stays. Note the
    // hold has already run out here, so this is the settle guard doing the work and
    // not the hold masking it: the two defer for different reasons and this test is
    // about the guard.
    passHold();
    document.dispatchEvent(new Event('wheel'));
    expect(hasNavFocus()).toBe(true);
    expect(el.classList._classes.has(FADING)).toBe(false);

    // Guard released → the next action is the user engaging → dissolves.
    settling = false;
    document.dispatchEvent(new Event('wheel'));
    expect(el.classList._classes.has(FADING)).toBe(true);
    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(el.classList._classes.has(STUCK)).toBe(false);
    expect(hasNavFocus()).toBe(false);
  });

  // The hold is what gives the marker an "on" phase at all. Without it a landing is
  // almost always followed within a moment by a scroll or a tap, so the dissolve began
  // about as soon as the turn-on finished and the light read as going out rather than
  // as being on. Distinct from the settle guard above: that one says "that wasn't you"
  // and discards the action, this one says "that was you, but wait" and banks it.
  it('holds at full for the whole hold even when the user acts immediately', () => {
    const el = makeEl();
    applyNavFocus(el);

    document.dispatchEvent(new Event('wheel')); // user engages at once
    expect(el.classList._classes.has(FADING)).toBe(false);

    // The REF retires immediately even so. Holding the paint must not hold the ref:
    // the user HAS scrolled, and that is the only question `navFocusElement` answers,
    // so turn-nav must fall back to scroll position rather than index-stepping from a
    // turn that is no longer where the user is looking.
    vi.advanceTimersByTime(20); // one frame
    expect(hasNavFocus()).toBe(false);

    // Still lit and un-dissolving, though, a frame short of the deadline. The deadline
    // includes the ramp: the hold is time at FULL brightness, so arming it at the
    // landing would spend the ramp out of it and deliver ~1.55s rather than the 2s
    // promised.
    vi.advanceTimersByTime(NAV_FOCUS_RAMP_MS + NAV_FOCUS_HOLD_MS - 40);
    expect(el.classList._classes.has(STUCK)).toBe(true);
    expect(el.classList._classes.has(FADING)).toBe(false);

    // The banked dismissal runs the instant the hold expires: acting early shortens
    // the wait, it does not shorten the marker.
    vi.advanceTimersByTime(20);
    expect(el.classList._classes.has(FADING)).toBe(true);

    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(el.classList._classes.has(STUCK)).toBe(false);
    expect(hasNavFocus()).toBe(false);
  });

  // A trackpad scroll is a BURST of wheel events, often more than one per frame. Since
  // arming the expiry cancels any pending one, re-arming per action would push the
  // deadline along ahead of the burst and keep the ref live for the whole gesture,
  // which is the stale-anchor bug the expiry exists to prevent. Only the first action
  // banks, so the deadline is fixed at the moment the user first engages.
  it('a burst of actions during the hold arms the ref expiry exactly once', () => {
    // Asserted by COUNTING frame requests rather than by advancing the clock. The fake
    // rAF snaps to fixed tick boundaries, so a re-arm always lands on the same virtual
    // deadline as the call it replaced and no amount of advancing can tell the two
    // apart; in a real browser each re-arm defers to the NEXT frame, which is what
    // walks the deadline along ahead of a burst. Driving rAF directly measures the
    // mechanism instead of a timing artefact of the stub.
    const frames: Array<() => void> = [];
    const raf = vi
      .spyOn(globalThis, 'requestAnimationFrame')
      .mockImplementation(((cb: FrameRequestCallback) => {
        frames.push(() => cb(0));
        return frames.length;
      }) as typeof requestAnimationFrame);

    try {
      const el = makeEl();
      applyNavFocus(el);

      // A trackpad gesture is a burst, often faster than one event per frame.
      document.dispatchEvent(new Event('wheel'));
      document.dispatchEvent(new Event('wheel'));
      document.dispatchEvent(new Event('wheel'));
      expect(frames.length).toBe(1); // armed by the first action only
      expect(navFocusElement()).toBe(el); // same tick: still readable

      frames[0]();
      expect(navFocusElement()).toBeNull();
      // Still holding, and still dissolving on the original schedule.
      expect(el.classList._classes.has(STUCK)).toBe(true);
      expect(el.classList._classes.has(FADING)).toBe(false);
    } finally {
      raf.mockRestore();
    }
  });

  it('stays lit indefinitely past the hold when the user does nothing', () => {
    const el = makeEl();
    applyNavFocus(el);

    // The hold is a floor, not a timeout. Nothing removes the marker on a schedule,
    // which is what keeps a slow load or a glance away from landing on nothing.
    vi.advanceTimersByTime(NAV_FOCUS_RAMP_MS + NAV_FOCUS_HOLD_MS + NAV_FOCUS_FADE_MS * 4);
    expect(el.classList._classes.has(STUCK)).toBe(true);
    expect(el.classList._classes.has(FADING)).toBe(false);
    expect(hasNavFocus()).toBe(true);

    document.dispatchEvent(new Event('wheel'));
    expect(el.classList._classes.has(FADING)).toBe(true);
  });

  it("a superseded marker's hold timer cannot dissolve the marker that replaced it", () => {
    const first = makeEl();
    const second = makeEl();
    applyNavFocus(first); // its hold would expire at RAMP + HOLD

    // STAGGER the two, so the old deadline falls strictly inside the new marker's own
    // hold. Applying both at the same instant would make this test pass on
    // `applyNavFocus`'s `_dismissQueued = false` reset alone, and it would keep passing
    // with BOTH the `clearTimeout(holdTimer)` and the generation guard stripped out
    // (verified). The offset is what makes it discriminate.
    vi.advanceTimersByTime(1000);
    applyNavFocus(second);
    document.dispatchEvent(new Event('wheel')); // banked against the SECOND marker

    // Now cross the FIRST marker's deadline but not the second's. A surviving timer
    // would find `_dismissQueued` true and dissolve a marker whose own hold is still
    // running.
    vi.advanceTimersByTime(NAV_FOCUS_RAMP_MS + NAV_FOCUS_HOLD_MS - 1000 + 20);
    expect(second.classList._classes.has(FADING)).toBe(false);
    expect(second.classList._classes.has(STUCK)).toBe(true);

    // The second marker's own hold still resolves its banked dismissal on schedule.
    vi.advanceTimersByTime(1000);
    expect(second.classList._classes.has(FADING)).toBe(true);
  });

  // The reduced-motion arm of the hold anchor had no coverage at all, and it is a real
  // branch: the CSS drops the ramp there, so the marker is at full from the first frame
  // and there is nothing to wait out. Waiting the ramp anyway would overshoot the
  // promised hold; not waiting it in normal motion would undershoot.
  it('under reduced motion the hold starts immediately, since there is no ramp', () => {
    const matchMedia = vi
      .spyOn(window, 'matchMedia')
      .mockImplementation(((q: string) => ({ matches: q.includes('reduce') })) as never);
    try {
      const el = makeEl();
      applyNavFocus(el);
      document.dispatchEvent(new Event('wheel'));

      // A frame short of the hold ALONE: still lit, nothing removed.
      vi.advanceTimersByTime(NAV_FOCUS_HOLD_MS - 20);
      expect(el.classList._classes.has(STUCK)).toBe(true);

      // At the hold, the banked dismissal runs, and reduced motion skips the dissolve
      // entirely rather than adding the fading class.
      vi.advanceTimersByTime(20);
      expect(el.classList._classes.has(STUCK)).toBe(false);
      expect(el.classList._classes.has(FADING)).toBe(false);
    } finally {
      matchMedia.mockRestore();
    }
  });

  it('a supersede during a fade cancels the pending removal cleanly', () => {
    const first = makeEl();
    const second = makeEl();
    applyNavFocus(first);
    passHold();
    document.dispatchEvent(new Event('wheel')); // first begins fading
    expect(first.classList._classes.has(FADING)).toBe(true);

    applyNavFocus(second); // supersede mid-fade → first cleared instantly
    expect(first.classList._classes.has(STUCK)).toBe(false);
    expect(first.classList._classes.has(FADING)).toBe(false);
    expect(second.classList._classes.has(STUCK)).toBe(true);

    // The first element's stale fade timer must not disturb the new marker.
    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(second.classList._classes.has(STUCK)).toBe(true);
    expect(hasNavFocus()).toBe(true);
  });
});
