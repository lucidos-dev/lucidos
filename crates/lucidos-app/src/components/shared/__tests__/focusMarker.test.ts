import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

import {
  applyNavFocus,
  clearNavFocus,
  hasNavFocus,
  navFocusElement,
  NAV_FOCUS_FADE_MS,
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

    document.dispatchEvent(new Event('wheel'));
    // Fade in flight: the fading class is added, the highlight still present.
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

    document.dispatchEvent(keydown('a')); // any key, not just scroll keys
    expect(el.classList._classes.has(FADING)).toBe(true);

    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(hasNavFocus()).toBe(false);
  });

  it('a follow-up action during the fade does not throw or re-arm', () => {
    const el = makeEl();
    applyNavFocus(el);

    document.dispatchEvent(new Event('wheel')); // starts the fade + tears listeners down
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

    // Guard active → the landing scroll's gesture is ignored, marker stays.
    document.dispatchEvent(new Event('wheel'));
    expect(hasNavFocus()).toBe(true);
    expect(el.classList._classes.has(FADING)).toBe(false);

    // Guard released → the next action is the user engaging → fades out.
    settling = false;
    document.dispatchEvent(new Event('wheel'));
    expect(el.classList._classes.has(FADING)).toBe(true);
    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(el.classList._classes.has(STUCK)).toBe(false);
    expect(hasNavFocus()).toBe(false);
  });

  it('a supersede during a fade cancels the pending removal cleanly', () => {
    const first = makeEl();
    const second = makeEl();
    applyNavFocus(first);
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
