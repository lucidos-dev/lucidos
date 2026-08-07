import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import {
  parseSavedScroll,
  isFullyRestorable,
  hasSavedScroll,
  resetContentScroll,
  contentScrollKey,
  formatSavedScroll,
  parseSavedRevision,
  savedScrollIsStale,
  dropStaleSavedScroll,
  attachScrollMemory,
  type ScrollMemoryLive,
} from './useScrollMemory';

describe('isFullyRestorable', () => {
  it('true when scrollable range covers the saved offset', () => {
    expect(isFullyRestorable(200, 1000, 500)).toBe(true);
  });

  it('true when saved exactly equals maxScroll', () => {
    expect(isFullyRestorable(500, 1000, 500)).toBe(true);
  });

  it('false when content has not grown enough yet', () => {
    expect(isFullyRestorable(300, 600, 500)).toBe(false);
  });

  it('false when content fits viewport (no scroll possible)', () => {
    expect(isFullyRestorable(200, 400, 500)).toBe(false);
  });

  it('true for saved=0 — restoring to top is always achievable', () => {
    // Distinguishes "user scrolled to top" (saved=0) from "no save" (key absent).
    // Without this, restore is skipped and ThreadView's auto-scroll snaps to bottom.
    expect(isFullyRestorable(0, 1000, 500)).toBe(true);
    expect(isFullyRestorable(0, 400, 500)).toBe(true);
  });

  it('false for negative saved values', () => {
    expect(isFullyRestorable(-10, 1000, 500)).toBe(false);
  });
});

describe('parseSavedScroll', () => {
  it('parses valid non-negative integer string', () => {
    expect(parseSavedScroll('250')).toBe(250);
  });

  it('parses 0', () => {
    expect(parseSavedScroll('0')).toBe(0);
  });

  it('returns null for null input', () => {
    expect(parseSavedScroll(null)).toBeNull();
  });

  it('returns null for empty string', () => {
    expect(parseSavedScroll('')).toBeNull();
  });

  it('returns null for non-numeric input', () => {
    expect(parseSavedScroll('abc')).toBeNull();
  });

  it('returns null for negative values', () => {
    expect(parseSavedScroll('-10')).toBeNull();
  });

  it('parses fractional values to integer', () => {
    // scrollTop is normally an integer, but be defensive on read
    expect(parseSavedScroll('250.7')).toBe(250);
  });

  it('returns null for NaN', () => {
    expect(parseSavedScroll('NaN')).toBeNull();
  });
});

describe('hasSavedScroll', () => {
  beforeEach(() => localStorage.clear());

  it('true when key holds a positive integer', () => {
    localStorage.setItem('k', '42');
    expect(hasSavedScroll('k')).toBe(true);
  });

  it('false when key absent', () => {
    expect(hasSavedScroll('missing')).toBe(false);
  });

  it('true when key holds 0 — user scrolled to top, distinct from no-save', () => {
    // Bug: scrolling to the very top cleared the key, so on remount
    // ThreadView's auto-scroll-to-bottom kicked in instead of restoring to 0.
    localStorage.setItem('k', '0');
    expect(hasSavedScroll('k')).toBe(true);
  });

  it('false when key holds garbage', () => {
    localStorage.setItem('k', 'abc');
    expect(hasSavedScroll('k')).toBe(false);
  });

  it('false when key is null (caller had no key)', () => {
    expect(hasSavedScroll(null)).toBe(false);
  });
});

describe('contentScrollKey', () => {
  it('matches the key shape ContentPane writes', () => {
    // Tests the contract between writer (ContentPane) and invalidators
    // (e.g., submitTrigger). If these drift, "reset on save" silently no-ops.
    expect(contentScrollKey('triggers')).toBe('lucidos-scroll-content-triggers');
  });
});

describe('resetContentScroll', () => {
  beforeEach(() => localStorage.clear());

  it('removes the saved offset for the view', () => {
    localStorage.setItem('lucidos-scroll-content-triggers', '500');
    resetContentScroll('triggers');
    expect(localStorage.getItem('lucidos-scroll-content-triggers')).toBeNull();
  });

  it('is a no-op when nothing is saved', () => {
    expect(() => resetContentScroll('triggers')).not.toThrow();
  });

  it('does not touch other views', () => {
    localStorage.setItem('lucidos-scroll-content-triggers', '500');
    localStorage.setItem('lucidos-scroll-content-apps', '200');
    resetContentScroll('triggers');
    expect(localStorage.getItem('lucidos-scroll-content-apps')).toBe('200');
  });
});

// ---------------------------------------------------------------------------
// A reading position is scoped to the transcript it was taken in. Restoring one
// after the thread has gained turns parks the reader in the middle of a thread
// they opened to see the new part of, which reads to them as an open that
// failed to go to the end. The stamp is what makes "has anything happened
// since?" answerable at restore time.
// ---------------------------------------------------------------------------
describe('formatSavedScroll / parseSavedRevision', () => {
  it('stamps the revision when one is tracked', () => {
    expect(formatSavedScroll(1500, 12)).toBe('1500:12');
    expect(parseSavedRevision('1500:12')).toBe(12);
  });

  it('leaves a revision-less caller on the historic bare-number format', () => {
    // ContentPane's per-view offsets track no revision and must keep working
    // byte-for-byte as before.
    expect(formatSavedScroll(1500)).toBe('1500');
    expect(parseSavedRevision('1500')).toBeNull();
  });

  it('keeps the offset readable through the stamp', () => {
    // Every reader of a POSITION (the restore, hasSavedScroll) is unaffected.
    expect(parseSavedScroll(formatSavedScroll(1500, 12))).toBe(1500);
    expect(hasSavedScroll('k')).toBe(false);
    localStorage.setItem('k', formatSavedScroll(0, 3));
    expect(hasSavedScroll('k')).toBe(true);
    localStorage.removeItem('k');
  });

  it('reads a malformed stamp as unstamped rather than as revision zero', () => {
    expect(parseSavedRevision('1500:')).toBeNull();
    expect(parseSavedRevision('1500:abc')).toBeNull();
  });
});

describe('savedScrollIsStale', () => {
  it('is false while nothing has happened since the save', () => {
    expect(savedScrollIsStale('1500:12', 12)).toBe(false);
  });

  it('is true once the thread has gained a turn', () => {
    expect(savedScrollIsStale('1500:12', 13)).toBe(true);
  });

  it('is false for a streaming turn that grows without adding one', () => {
    // The revision is the EXCHANGE count precisely so a response growing under
    // a parked reader does not retire their position mid-read.
    expect(savedScrollIsStale('1500:12', 12)).toBe(false);
  });

  it('is false when nothing is saved', () => {
    expect(savedScrollIsStale(null, 20)).toBe(false);
    expect(savedScrollIsStale('', 20)).toBe(false);
  });

  it('is false before the content is rendered, however old the save', () => {
    // A count of 0 means "not loaded yet", never "the thread is empty":
    // focusThread runs before the events land, and discarding there would wipe
    // a position on every fast thread switch.
    expect(savedScrollIsStale('1500:12', 0)).toBe(false);
    expect(savedScrollIsStale('1500:12', Number.NaN)).toBe(false);
  });

  it('retires an unstamped save as soon as there is content', () => {
    // Values written before the stamp existed cannot be checked, and a wrong
    // restore is the failure this prevents. This is what heals a browser
    // carrying a position saved from a stranded transcript.
    expect(savedScrollIsStale('1500', 12)).toBe(true);
    expect(savedScrollIsStale('1500', 0)).toBe(false);
  });

  it('never retires a save from a thread that SHRANK', () => {
    // A collapsed / re-windowed transcript is the same conversation.
    expect(savedScrollIsStale('1500:20', 12)).toBe(false);
  });
});

describe('dropStaleSavedScroll', () => {
  beforeEach(() => localStorage.clear());

  it('removes a stale position and reports it, so hasSavedScroll agrees', () => {
    // The two must answer the same question within one open: hasSavedScroll is
    // what makes focusThread skip its scroll-to-bottom, and a restore that
    // declined separately would leave the reader at neither position.
    localStorage.setItem('k', '1500:12');
    expect(dropStaleSavedScroll('k', 13)).toBe(true);
    expect(hasSavedScroll('k')).toBe(false);
  });

  it('keeps a current position', () => {
    localStorage.setItem('k', '1500:12');
    expect(dropStaleSavedScroll('k', 12)).toBe(false);
    expect(hasSavedScroll('k')).toBe(true);
  });

  it('is a no-op when nothing is saved', () => {
    expect(dropStaleSavedScroll('missing', 12)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// The teardown flush must write what THIS key observed, never what the next
// render is holding. `attachScrollMemory` is the hook's whole body, extracted so
// the lifecycle can be driven with a fake element (the shape `makeScrollObservers`
// already uses), because the defect here is entirely about WHEN a value is read
// and no assertion over the hook could reach it.
// ---------------------------------------------------------------------------
describe('attachScrollMemory teardown', () => {
  function makeEl(scrollTop: number, scrollHeight = 5000) {
    const listeners: Array<() => void> = [];
    return {
      scrollTop,
      scrollHeight,
      clientHeight: 800,
      addEventListener: (_t: string, fn: () => void) => { listeners.push(fn); },
      removeEventListener: (_t: string, fn: () => void) => {
        const i = listeners.indexOf(fn);
        if (i >= 0) listeners.splice(i, 1);
      },
      fireScroll: () => { for (const fn of [...listeners]) fn(); },
      listenerCount: () => listeners.length,
    } as any;
  }

  // A pre-seeded position sends the attach into its restore branch, which
  // observes the container. The test env has no DOM, so the observers are
  // inert stubs: this suite is about the SAVE path, and the restore has its
  // own coverage in `isFullyRestorable`.
  class InertObserver {
    observe() {}
    disconnect() {}
    takeRecords() { return []; }
  }
  let origRO: unknown;
  let origMO: unknown;

  beforeEach(() => {
    localStorage.clear();
    origRO = (globalThis as any).ResizeObserver;
    origMO = (globalThis as any).MutationObserver;
    (globalThis as any).ResizeObserver = InertObserver;
    (globalThis as any).MutationObserver = InertObserver;
  });
  afterEach(() => {
    (globalThis as any).ResizeObserver = origRO;
    (globalThis as any).MutationObserver = origMO;
  });

  it('writes the outgoing key with the offset and revision it actually saw', () => {
    // The switch that used to corrupt it: parked at 1800 in a 40-turn thread,
    // then tap a 3-turn thread. The cleanup runs after that render, so the
    // container already shows the new thread and `live()` already answers for it.
    const el = makeEl(1800);
    let live: ScrollMemoryLive = { shouldSave: () => true, revision: 40 };
    const detach = attachScrollMemory(el, 'k', { live: () => live });

    el.fireScroll();

    // ...the incoming thread's render lands before the cleanup does.
    el.scrollTop = 120;
    live = { shouldSave: () => true, revision: 3 };
    detach();

    expect(localStorage.getItem('k')).toBe('1800:40');
    // A revision of 3 written here would have made the position look older than
    // the thread it belongs to, so reopening would have retired it outright.
    expect(savedScrollIsStale(localStorage.getItem('k'), 40)).toBe(false);
  });

  it('does not clear the outgoing key because the incoming thread is at the bottom', () => {
    // `shouldSave` reads the SHARED `scrolledUp` signal, which focusThread resets
    // for the thread being opened. Evaluated at teardown it said "do not save"
    // about a thread the reader was parked in, and the key was deleted.
    const el = makeEl(1800);
    let live: ScrollMemoryLive = { shouldSave: () => true, revision: 40 };
    const detach = attachScrollMemory(el, 'k', { live: () => live });

    el.fireScroll();
    live = { shouldSave: () => false, revision: 3 };
    detach();

    expect(localStorage.getItem('k')).toBe('1800:40');
  });

  it('still clears the key when the reader themselves returned to the bottom', () => {
    localStorage.setItem('k', '1800:40');
    const el = makeEl(1800);
    const live: ScrollMemoryLive = { shouldSave: () => false, revision: 40 };
    const detach = attachScrollMemory(el, 'k', { live: () => live });

    el.fireScroll(); // the scroll that took them back to the bottom
    detach();

    expect(localStorage.getItem('k')).toBeNull();
  });

  it('ignores a scroll that lands after this key stopped being the current one', () => {
    // Leaving a thread you were parked in used to lose your place in it, by two
    // routes into the same window: the teardown is deferred past the render that
    // changed the key, so the listener is still attached while the shared
    // transcript already belongs to the next thread. Either focusThread's
    // synchronous pin moved it (the incoming thread had no position of its own)
    // or swapping in the incoming content clamped it, and the scroll event
    // carried the INCOMING thread's shouldSave.
    localStorage.setItem('k', '5000:12');
    // Tall enough that the restore completes on attach: until it does, the save
    // listener is gated behind `restoring` and the assertion would be vacuous.
    const el = makeEl(5000, 20000);
    let current = true;
    const live: ScrollMemoryLive = {
      shouldSave: () => false,   // the INCOMING thread's answer, not this key's
      revision: 3,               // ...and the incoming thread's turn count
    };
    const detach = attachScrollMemory(el, 'k', {
      live: () => live,
      isCurrent: () => current,
    });

    current = false;   // the render moved on to the next thread
    el.scrollTop = 0;  // its shorter content clamped the shared container
    el.fireScroll();
    detach();

    expect(localStorage.getItem('k')).toBe('5000:12');
  });

  it('still records the reader while this key IS the current one', () => {
    const el = makeEl(5000, 20000);
    const live: ScrollMemoryLive = { shouldSave: () => true, revision: 12 };
    const detach = attachScrollMemory(el, 'k', {
      live: () => live,
      isCurrent: () => true,
    });

    el.scrollTop = 3200;
    el.fireScroll();
    detach();

    expect(localStorage.getItem('k')).toBe('3200:12');
  });

  it('lets a deliberate go-to-bottom clear the position, pin or no pin', () => {
    // The guard is about WHOSE key the scroll belongs to, never about what
    // caused it. Suppressing app-caused scrolls instead would swallow this:
    // answering a question or tapping the down chevron pins the reader to the
    // newest turn, and their old parked position must not survive that.
    localStorage.setItem('k', '5000:12');
    const el = makeEl(5000, 20000);
    const live: ScrollMemoryLive = { shouldSave: () => false, revision: 12 };
    const detach = attachScrollMemory(el, 'k', {
      live: () => live,
      isCurrent: () => true,
    });

    el.scrollTop = 19200; // the pin's write
    el.fireScroll();
    detach();

    expect(localStorage.getItem('k')).toBeNull();
  });

  it('leaves a stored position untouched when this key saw no scroll at all', () => {
    localStorage.setItem('k', '1800:40');
    const el = makeEl(1800);
    const detach = attachScrollMemory(el, 'k', { live: () => ({ shouldSave: () => true }) });
    detach();
    expect(localStorage.getItem('k')).toBe('1800:40');
  });

  it('detaches its scroll listener', () => {
    const el = makeEl(0);
    const detach = attachScrollMemory(el, 'k', { live: () => ({}) });
    expect(el.listenerCount()).toBe(1);
    detach();
    expect(el.listenerCount()).toBe(0);
  });
});
