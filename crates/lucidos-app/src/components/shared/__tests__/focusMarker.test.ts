import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

import { applyNavFocus, clearNavFocus, hasNavFocus, NAV_FOCUS_FADE_MS } from '../focusMarker';

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
  it('applies just the sticky border (no entrance animation, no fade class)', () => {
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

  it('clearNavFocus() removes the border immediately and is idempotent', () => {
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
    // Fade in flight: the fading class is added, the border still present.
    expect(el.classList._classes.has(FADING)).toBe(true);
    expect(el.classList._classes.has(STUCK)).toBe(true);
    expect(hasNavFocus()).toBe(true);

    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(el.classList._classes.has(STUCK)).toBe(false);
    expect(el.classList._classes.has(FADING)).toBe(false);
    expect(hasNavFocus()).toBe(false);
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
    // Listeners are gone, so a second action is a no-op (would-be double fade).
    document.dispatchEvent(new Event('pointerdown'));

    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(hasNavFocus()).toBe(false);
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
