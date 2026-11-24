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
import { osReducesMotion } from '../../../utils/motion';

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

function passHold() {
  vi.advanceTimersByTime(NAV_FOCUS_RAMP_MS + NAV_FOCUS_HOLD_MS);
}

beforeEach(() => {
  vi.useFakeTimers();
  clearNavFocus();
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
    clearNavFocus();
    expect(hasNavFocus()).toBe(false);
  });

  it('a user scroll gesture fades the marker out, then removes it', () => {
    const el = makeEl();
    applyNavFocus(el);
    expect(hasNavFocus()).toBe(true);

    passHold();
    document.dispatchEvent(new Event('wheel'));
    expect(el.classList._classes.has(FADING)).toBe(true);
    expect(el.classList._classes.has(STUCK)).toBe(true);
    expect(hasNavFocus()).toBe(true);

    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(el.classList._classes.has(STUCK)).toBe(false);
    expect(el.classList._classes.has(FADING)).toBe(false);
    expect(hasNavFocus()).toBe(false);
  });

  it('retires the ref one frame after dismissal, while the highlight dissolves on', () => {
    const el = makeEl();
    applyNavFocus(el);

    passHold();
    document.dispatchEvent(new Event('wheel'));
    expect(navFocusElement()).toBe(el);

    vi.advanceTimersByTime(20);
    expect(navFocusElement()).toBeNull();
    expect(hasNavFocus()).toBe(false);
    expect(el.classList._classes.has(STUCK)).toBe(true);
    expect(el.classList._classes.has(FADING)).toBe(true);

    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(el.classList._classes.has(STUCK)).toBe(false);
    expect(el.classList._classes.has(FADING)).toBe(false);
  });

  it('clearNavFocus() strips the classes of an element already past its ref expiry', () => {
    const el = makeEl();
    applyNavFocus(el);
    passHold();
    document.dispatchEvent(new Event('wheel'));
    vi.advanceTimersByTime(20);
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

    const keydown = (key: string) => {
      const e = new Event('keydown') as Event & { key: string };
      e.key = key;
      return e;
    };

    passHold();
    document.dispatchEvent(keydown('a'));
    expect(el.classList._classes.has(FADING)).toBe(true);

    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(hasNavFocus()).toBe(false);
  });

  it('a follow-up action during the fade does not throw or re-arm', () => {
    const el = makeEl();
    applyNavFocus(el);

    passHold();
    document.dispatchEvent(new Event('wheel'));
    expect(el.classList._classes.has(FADING)).toBe(true);

    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS - 200);
    expect(el.classList._classes.has(FADING)).toBe(true);
    document.dispatchEvent(new Event('pointerdown'));

    vi.advanceTimersByTime(300);
    expect(el.classList._classes.has(STUCK)).toBe(false);
    expect(el.classList._classes.has(FADING)).toBe(false);
    expect(hasNavFocus()).toBe(false);
  });

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
      document.dispatchEvent(new Event('wheel'));

      vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
      expect(el.classList._classes.has(STUCK)).toBe(false);
      expect(frames.length).toBe(1);

      applyNavFocus(el);
      expect(navFocusElement()).toBe(el);

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

    passHold();
    document.dispatchEvent(new Event('wheel'));
    expect(hasNavFocus()).toBe(true);
    expect(el.classList._classes.has(FADING)).toBe(false);

    settling = false;
    document.dispatchEvent(new Event('wheel'));
    expect(el.classList._classes.has(FADING)).toBe(true);
    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(el.classList._classes.has(STUCK)).toBe(false);
    expect(hasNavFocus()).toBe(false);
  });

  it('holds at full for the whole hold even when the user acts immediately', () => {
    const el = makeEl();
    applyNavFocus(el);

    document.dispatchEvent(new Event('wheel'));
    expect(el.classList._classes.has(FADING)).toBe(false);

    vi.advanceTimersByTime(20);
    expect(hasNavFocus()).toBe(false);

    vi.advanceTimersByTime(NAV_FOCUS_RAMP_MS + NAV_FOCUS_HOLD_MS - 40);
    expect(el.classList._classes.has(STUCK)).toBe(true);
    expect(el.classList._classes.has(FADING)).toBe(false);

    vi.advanceTimersByTime(20);
    expect(el.classList._classes.has(FADING)).toBe(true);

    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(el.classList._classes.has(STUCK)).toBe(false);
    expect(hasNavFocus()).toBe(false);
  });

  it('a burst of actions during the hold arms the ref expiry exactly once', () => {
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

      document.dispatchEvent(new Event('wheel'));
      document.dispatchEvent(new Event('wheel'));
      document.dispatchEvent(new Event('wheel'));
      expect(frames.length).toBe(1);
      expect(navFocusElement()).toBe(el);

      frames[0]();
      expect(navFocusElement()).toBeNull();
      expect(el.classList._classes.has(STUCK)).toBe(true);
      expect(el.classList._classes.has(FADING)).toBe(false);
    } finally {
      raf.mockRestore();
    }
  });

  it('stays lit indefinitely past the hold when the user does nothing', () => {
    const el = makeEl();
    applyNavFocus(el);

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
    applyNavFocus(first);

    vi.advanceTimersByTime(1000);
    applyNavFocus(second);
    document.dispatchEvent(new Event('wheel'));

    vi.advanceTimersByTime(NAV_FOCUS_RAMP_MS + NAV_FOCUS_HOLD_MS - 1000 + 20);
    expect(second.classList._classes.has(FADING)).toBe(false);
    expect(second.classList._classes.has(STUCK)).toBe(true);

    vi.advanceTimersByTime(1000);
    expect(second.classList._classes.has(FADING)).toBe(true);
  });

  it('under reduced motion the hold starts immediately, since there is no ramp', () => {
    osReducesMotion.value = true;
    try {
      const el = makeEl();
      applyNavFocus(el);
      document.dispatchEvent(new Event('wheel'));

      vi.advanceTimersByTime(NAV_FOCUS_HOLD_MS - 20);
      expect(el.classList._classes.has(STUCK)).toBe(true);

      vi.advanceTimersByTime(20);
      expect(el.classList._classes.has(STUCK)).toBe(false);
      expect(el.classList._classes.has(FADING)).toBe(false);
    } finally {
      osReducesMotion.value = false;
    }
  });

  it('a supersede during a fade cancels the pending removal cleanly', () => {
    const first = makeEl();
    const second = makeEl();
    applyNavFocus(first);
    passHold();
    document.dispatchEvent(new Event('wheel'));
    expect(first.classList._classes.has(FADING)).toBe(true);

    applyNavFocus(second);
    expect(first.classList._classes.has(STUCK)).toBe(false);
    expect(first.classList._classes.has(FADING)).toBe(false);
    expect(second.classList._classes.has(STUCK)).toBe(true);

    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(second.classList._classes.has(STUCK)).toBe(true);
    expect(hasNavFocus()).toBe(true);
  });
});
