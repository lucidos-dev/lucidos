import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  parseSavedScroll,
  isFullyRestorable,
  attachScrollMemory,
  LIVE_EDGE_VALUE,
} from './useScrollMemory';
import {
  clearPendingEventScroll,
  followingLiveEdge,
  hasPendingEventScroll,
  classifyScroll,
  makeScrollObservers,
  readerGestureForTest,
  setFollowLiveEdge,
  scrollToEventAndPulse,
  setActiveScrollElement,
  setTranscriptLive,
  stopFollowingBottom,
  type ScrollClass,
} from '../components/chat/scrollState';
import { onClassifiedScroll, onClassifiedScrollAsEngine } from '../components/chat/classifiedScroll';
import { _resetPageVisitForTesting } from '../utils/pageVisit';
import { USER_ACTION_EVENTS } from '../utils/userAction';
import { installFakePage } from '../utils/__tests__/fakePage';
import { mockTranscript } from '../components/chat/__tests__/scroll-test-helpers';
import { osReducesMotion } from '../utils/motion';

beforeEach(() => setFollowLiveEdge(false));
afterEach(() => setFollowLiveEdge(false));

afterEach(() => readerGestureForTest(null, false));

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
    expect(isFullyRestorable(0, 1000, 500)).toBe(true);
    expect(isFullyRestorable(0, 400, 500)).toBe(true);
  });

  it('false for negative saved values', () => {
    expect(isFullyRestorable(-10, 1000, 500)).toBe(false);
  });
});

describe('parseSavedScroll', () => {
  it('parses valid non-negative integer string', () => {
    expect(parseSavedScroll('250')).toEqual({ kind: 'offset', top: 250 });
  });

  it('parses 0', () => {
    expect(parseSavedScroll('0')).toEqual({ kind: 'offset', top: 0 });
  });

  it('parses the live edge, which is a position and not an offset', () => {
    expect(parseSavedScroll(LIVE_EDGE_VALUE)).toEqual({ kind: 'live-edge' });
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
    expect(parseSavedScroll('250.7')).toEqual({ kind: 'offset', top: 250 });
  });

  it('returns null for NaN', () => {
    expect(parseSavedScroll('NaN')).toBeNull();
  });

  it('reads an older build\'s armed marker as the bare place, holding no follow', () => {
    expect(parseSavedScroll('following:250')).toEqual({ kind: 'offset', top: 250 });
    expect(parseSavedScroll('following:anchor:-150:t2'))
      .toEqual({ kind: 'anchor', eventId: 't2', relTop: -150 });
  });

  it('returns null for a marker in front of nothing legible', () => {
    expect(parseSavedScroll('following:')).toBeNull();
    expect(parseSavedScroll('following:abc')).toBeNull();
  });

  it('reads a marked live edge as the plain one, which is armed already', () => {
    expect(parseSavedScroll(`following:${LIVE_EDGE_VALUE}`)).toEqual({ kind: 'live-edge' });
  });
});

describe('a stamped position written by an older build still parses', () => {
  it('reads the offset out and ignores the retired revision suffix', () => {
    expect(parseSavedScroll('1500:12')).toEqual({ kind: 'offset', top: 1500 });
    expect(parseSavedScroll('0:3')).toEqual({ kind: 'offset', top: 0 });
  });
});

describe('the line form parses, and every older form still opens where it did', () => {
  it.each([
    ['250', { kind: 'offset', top: 250 }],
    ['1500:12', { kind: 'offset', top: 1500 }],
    [LIVE_EDGE_VALUE, { kind: 'live-edge' }],
    ['anchor:-150:t2', { kind: 'anchor', eventId: 't2', relTop: -150 }],
    ['anchor:30:a:b', { kind: 'anchor', eventId: 'a:b', relTop: 30 }],
    ['row:-10:call-7', { kind: 'row', rowEventId: 'call-7', relTop: -10 }],
    ['following:row:0:call-7', { kind: 'row', rowEventId: 'call-7', relTop: 0 }],
    ['following:anchor:-150:t2', { kind: 'anchor', eventId: 't2', relTop: -150 }],
  ])('reads %s as before', (raw, expected) => {
    expect(parseSavedScroll(raw)).toEqual(expected);
  });

  it('reads the block offset, the turn offset, the id, the path and its kinds', () => {
    expect(parseSavedScroll('line:-400:-1320:t2:2~0a1b2c3d'))
      .toEqual({ kind: 'line', eventId: 't2', path: [2], kinds: '0a1b2c3d', relTop: -400, turnRelTop: -1320 });
    expect(parseSavedScroll('line:0:-80:t2:1.0.3~ffffffff'))
      .toEqual({ kind: 'line', eventId: 't2', path: [1, 0, 3], kinds: 'ffffffff', relTop: 0, turnRelTop: -80 });
  });

  it('keeps an id that carries colons whole', () => {
    expect(parseSavedScroll('line:-5:-90:call:abc:def:4~0a1b2c3d'))
      .toEqual({ kind: 'line', eventId: 'call:abc:def', path: [4], kinds: '0a1b2c3d', relTop: -5, turnRelTop: -90 });
  });

  it('reads a line behind an older build\'s marker as the bare line', () => {
    expect(parseSavedScroll('following:line:-5:-90:t2:4~0a1b2c3d'))
      .toEqual({ kind: 'line', eventId: 't2', path: [4], kinds: '0a1b2c3d', relTop: -5, turnRelTop: -90 });
  });

  it.each([
    'line:',
    'line:-5',
    'line:-5:-90',
    'line:-5:-90:t2',
    'line:-5:-90::4~0a1b2c3d',
    'line:-5:-90:t2:',
    'line:-5:-90:t2:4',
    'line:-5:-90:t2:4~',
    'line:-5:-90:t2:4~0A1B2C3D',
    'line:-5:-90:t2:4~0a1b2c3',
    'line:-5:-90:t2:1.~0a1b2c3d',
    'line:-5:-90:t2:a~0a1b2c3d',
    'line:x:-90:t2:4~0a1b2c3d',
    'line:-5:9x:t2:4~0a1b2c3d',
  ])('answers null for %s, a value we did not write', (raw) => {
    expect(parseSavedScroll(raw)).toBeNull();
  });
});

describe('attachScrollMemory teardown', () => {
  function makeEl(scrollTop: number, scrollHeight = 5000) {
    const listeners: Array<{ type: string; fn: () => void }> = [];
    return {
      scrollTop,
      scrollHeight,
      clientHeight: 800,
      parentElement: null,
      getBoundingClientRect: () => ({ width: 800, height: 800, top: 0, bottom: 800, left: 0, right: 800 }),
      addEventListener: (type: string, fn: () => void) => { listeners.push({ type, fn }); },
      removeEventListener: (type: string, fn: () => void) => {
        const i = listeners.findIndex((l) => l.type === type && l.fn === fn);
        if (i >= 0) listeners.splice(i, 1);
      },
      fireScroll: () => {
        for (const l of [...listeners]) if (l.type === 'scroll') l.fn();
      },
      listenerCount: () => listeners.filter((l) => l.type === 'scroll').length,
    } as any;
  }

  class InertObserver {
    observe() {}
    disconnect() {}
    takeRecords() { return []; }
  }

  function userAction(type: (typeof USER_ACTION_EVENTS)[number] = 'wheel') {
    document.dispatchEvent(new Event(type));
  }

  function captureObservers() {
    const callbacks: Array<(records: unknown[]) => void> = [];
    class Capturing {
      cb: (records: unknown[]) => void;
      constructor(cb: (records: unknown[]) => void) { this.cb = cb; callbacks.push(cb); }
      observe() {}
      disconnect() {
        const i = callbacks.indexOf(this.cb);
        if (i >= 0) callbacks.splice(i, 1);
      }
      takeRecords() { return []; }
    }
    (globalThis as any).ResizeObserver = Capturing;
    (globalThis as any).MutationObserver = Capturing;
    return {
      fire: () => { for (const cb of [...callbacks]) cb([]); },
      armed: () => callbacks.length,
    };
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

  it('writes the outgoing key with the offset it actually saw', () => {
    const el = makeEl(1800);
    const detach = attachScrollMemory(el, 'k', { live: () => ({}) });

    el.fireScroll();

    el.scrollTop = 120;
    detach();

    expect(localStorage.getItem('k')).toBe('1800');
  });

  it('records a reader who ended up at the bottom, rather than forgetting them', () => {
    const el = makeEl(4200);
    const detach = attachScrollMemory(el, 'k', { live: () => ({}) });

    el.fireScroll();
    detach();

    expect(localStorage.getItem('k')).toBe('4200');
  });

  it('records a reader who scrolled to the very top, distinctly from no save at all', () => {
    const el = makeEl(0, 5000);
    const detach = attachScrollMemory(el, 'k', { live: () => ({}) });

    el.fireScroll();
    detach();

    expect(localStorage.getItem('k')).toBe('0');
    expect(parseSavedScroll(localStorage.getItem('k'))).toEqual({ kind: 'offset', top: 0 });
  });

  it('ignores a scroll that lands after this key stopped being the current one', () => {
    localStorage.setItem('k', '5000');
    const el = makeEl(5000, 20000);
    let current = true;
    const detach = attachScrollMemory(el, 'k', {
      live: () => ({}),
      isCurrent: () => current,
    });

    current = false;
    el.scrollTop = 0;
    el.fireScroll();
    detach();

    expect(localStorage.getItem('k')).toBe('5000');
  });

  it('still records the reader while this key IS the current one', () => {
    const el = makeEl(5000, 20000);
    const detach = attachScrollMemory(el, 'k', {
      live: () => ({}),
      isCurrent: () => true,
    });

    el.scrollTop = 3200;
    el.fireScroll();
    detach();

    expect(localStorage.getItem('k')).toBe('3200');
  });

  it('overwrites the position when the reader takes the chevron to the bottom', () => {
    localStorage.setItem('k', '5000');
    const el = makeEl(5000, 20000);
    const detach = attachScrollMemory(el, 'k', {
      live: () => ({}),
      isCurrent: () => true,
    });

    el.scrollTop = 19200;
    el.fireScroll();
    detach();

    expect(localStorage.getItem('k')).toBe('19200');
  });

  it('leaves a stored position untouched when this key saw no scroll at all', () => {
    localStorage.setItem('k', '1800');
    const el = makeEl(1800);
    const detach = attachScrollMemory(el, 'k', { live: () => ({}) });
    detach();
    expect(localStorage.getItem('k')).toBe('1800');
  });


  it('opens at the top when there is no saved position', () => {
    const el = makeEl(4200, 20000);
    const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

    expect(el.scrollTop).toBe(0);
    detach();
  });

  it('restores a saved position instead of resetting to the top', () => {
    localStorage.setItem('k', '3200');
    const el = makeEl(0, 20000);
    const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

    expect(el.scrollTop).toBe(3200);
    detach();
  });


  it('never lands the reader on the live edge when the saved offset is out of reach', async () => {
    vi.useFakeTimers();
    try {
      localStorage.setItem('k', '12000');
      const el = makeEl(0, 5000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

      await vi.advanceTimersByTimeAsync(3000);

      expect(el.scrollTop).toBe(0);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('parks the reader at the top while it waits, not on the outgoing offset', () => {
    localStorage.setItem('k', '12000');
    const el = makeEl(4200, 5000);
    captureObservers();
    const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

    expect(el.scrollTop).toBe(0);
    detach();
  });

  it('does not park a reader whose offset is reachable right now', () => {
    localStorage.setItem('k', '3200');
    const el = makeEl(4200, 20000);
    const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

    expect(el.scrollTop).toBe(3200);
    detach();
  });


  it('takes one last look at the deadline, for growth neither observer saw', async () => {
    vi.useFakeTimers();
    try {
      localStorage.setItem('k', '3200');
      const el = makeEl(500, 1000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

      el.scrollHeight = 20000;
      await vi.advanceTimersByTimeAsync(3000);

      expect(el.scrollTop).toBe(3200);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });


  it('retires the whole wait on the reader\'s first gesture', () => {
    localStorage.setItem('k', '3200');
    const el = makeEl(0, 3000);
    const observers = captureObservers();
    const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

    userAction('wheel');
    el.scrollTop = 900;
    el.scrollHeight = 20000;
    observers.fire();

    expect(el.scrollTop).toBe(900);
    detach();
  });

  it('retires the deadline\'s last look with it', async () => {
    vi.useFakeTimers();
    try {
      localStorage.setItem('k', '3200');
      const el = makeEl(0, 1000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

      userAction('touchmove');
      el.scrollTop = 900;
      el.scrollHeight = 20000;
      await vi.advanceTimersByTimeAsync(3000);

      expect(el.scrollTop).toBe(900);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('reads the app\'s own writes as untouched, gesture-less as they are', () => {
    localStorage.setItem('k', '3200');
    const el = makeEl(0, 3000);
    const observers = captureObservers();
    const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

    el.scrollTop = 2200;
    el.scrollHeight = 20000;
    observers.fire();

    expect(el.scrollTop).toBe(3200);
    detach();
  });

  it('defers the RESTORE to a deep-link that owns the open', () => {
    localStorage.setItem('k', '3200');
    const el = makeEl(4200, 20000);
    const detach = attachScrollMemory(el, 'k', {
      live: () => ({ shouldRestore: () => false }),
      resetOnEmpty: true,
    });

    expect(el.scrollTop).toBe(4200);
    detach();
  });

  describe('a deep-link claiming the open mid-restore', () => {
    let origCSS: unknown;
    beforeEach(() => {
      origCSS = (globalThis as any).CSS;
      (globalThis as any).CSS = { escape: (s: string) => s };
    });
    afterEach(() => {
      (globalThis as any).CSS = origCSS;
      clearPendingEventScroll();
    });

    const transcript = () => ({ shouldRestore: () => !hasPendingEventScroll() });

    it('retires a restore armed before the claim, so the landing stands', () => {
      localStorage.setItem('k', '3200');
      const el = makeEl(0, 1000);
      const observers = captureObservers();
      const detach = attachScrollMemory(el, 'k', {
        live: transcript,
        resetOnEmpty: true,
      });
      expect(el.scrollTop).toBe(0);
      const armedBefore = observers.armed();
      expect(armedBefore).toBeGreaterThan(0);

      scrollToEventAndPulse('e1');
      expect(observers.armed()).toBeLessThan(armedBefore);

      el.scrollHeight = 20000;
      el.scrollTop = 8800;
      observers.fire();

      expect(el.scrollTop).toBe(8800);
      detach();
    });

    it('retires the restore DEADLINE with it', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', '3200');
        const el = makeEl(0, 1000);
        captureObservers();
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
        });

        scrollToEventAndPulse('e1');
        el.scrollHeight = 20000;
        await vi.advanceTimersByTimeAsync(3000);

        expect(el.scrollTop).toBe(0);
        detach();
      } finally {
        vi.useRealTimers();
      }
    });

    it('rescues the reader when the link it stood down for turns out dead', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', '3200');
        const el = makeEl(4200, 1000);
        captureObservers();
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
        });

        scrollToEventAndPulse('e1');
        el.scrollHeight = 20000;

        await vi.advanceTimersByTimeAsync(4000);
        clearPendingEventScroll();
        await vi.advanceTimersByTimeAsync(600);

        expect(el.scrollTop).toBe(3200);
        detach();
      } finally {
        vi.useRealTimers();
      }
    });

    function deadLinkOverNoRecord(el: any) {
      captureObservers();
      stopFollowingBottom();
      scrollToEventAndPulse('never-renders');
      return attachScrollMemory(el, 'k', {
        live: transcript,
        resetOnEmpty: true,
        followsLiveEdge: true,
      });
    }

    it('does not reset an armed reader to the top when the link turns out dead', async () => {
      vi.useFakeTimers();
      try {
        setFollowLiveEdge(true);
        const el = makeEl(4200, 20000);
        const detach = deadLinkOverNoRecord(el);

        expect(followingLiveEdge.value).toBe(true);
        await vi.advanceTimersByTimeAsync(4000);
        clearPendingEventScroll();
        await vi.advanceTimersByTimeAsync(600);

        expect(el.scrollTop).toBe(19200);
        expect(followingLiveEdge.value).toBe(true);
        detach();
      } finally {
        vi.useRealTimers();
      }
    });

    it('still opens an UNARMED reader at the top when the link turns out dead', async () => {
      vi.useFakeTimers();
      try {
        const el = makeEl(4200, 20000);
        const detach = deadLinkOverNoRecord(el);

        expect(followingLiveEdge.value).toBe(false);
        await vi.advanceTimersByTimeAsync(4000);
        clearPendingEventScroll();
        await vi.advanceTimersByTimeAsync(600);

        expect(el.scrollTop).toBe(0);
        detach();
      } finally {
        vi.useRealTimers();
      }
    });

    it('extends the rescue when a SECOND link is tapped mid-window', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', '3200');
        const el = makeEl(4200, 1000);
        captureObservers();
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
        });

        scrollToEventAndPulse('e1');
        el.scrollHeight = 20000;
        await vi.advanceTimersByTimeAsync(1000);
        scrollToEventAndPulse('e2');
        await vi.advanceTimersByTimeAsync(4600);

        expect(el.scrollTop).toBe(3200);
        detach();
      } finally {
        vi.useRealTimers();
      }
    });

    it('does not rescue a second link away from the first link\'s landing', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', '3200');
        const el = makeEl(4200, 1000);
        captureObservers();
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
        });

        scrollToEventAndPulse('e1');
        el.scrollHeight = 20000;
        el.scrollTop = 8800;
        await vi.advanceTimersByTimeAsync(1000);
        scrollToEventAndPulse('e2');
        await vi.advanceTimersByTimeAsync(4600);

        expect(el.scrollTop).toBe(8800);
        detach();
      } finally {
        vi.useRealTimers();
      }
    });

    function withFindableTarget(
      el: any,
      opts: { rectTop?: number; absTop?: number; reducedMotion?: boolean } = {},
    ) {
      const rectTopOf = () => (opts.absTop !== undefined ? opts.absTop - el.scrollTop : opts.rectTop ?? 0);
      el.getBoundingClientRect = () => ({ width: 800, height: 800, top: 0, bottom: 800, left: 0, right: 800 });
      const target = {
        parentElement: null,
        classList: { add: () => {}, remove: () => {} },
        getBoundingClientRect: () => {
          const top = rectTopOf();
          return { width: 200, height: 200, top, bottom: top + 200, left: 0, right: 200 };
        },
        matches: () => false,
        querySelector: () => null,
      } as any;
      const origQSA = (globalThis.document as any).querySelectorAll;
      const origCS = (globalThis as any).getComputedStyle;
      (globalThis.document as any).querySelectorAll = (sel: string) =>
        (sel.startsWith('[data-event-id') ? [target] : []);
      (globalThis as any).getComputedStyle = () => ({ scrollMarginTop: '0px' });
      if (opts.reducedMotion) osReducesMotion.value = true;
      setActiveScrollElement(el);
      return () => {
        setActiveScrollElement(null);
        (globalThis.document as any).querySelectorAll = origQSA;
        (globalThis as any).getComputedStyle = origCS;
        osReducesMotion.value = false;
      };
    }

    it('records where the landing ARRIVED, not where it set off from', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', '200');
        const el = makeEl(4200, 20000);
        const restoreDom = withFindableTarget(el, { rectTop: 1000, reducedMotion: true });
        captureObservers();
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
        });

        try {
          expect(el.scrollTop).toBe(200);
          scrollToEventAndPulse('e1');
          expect(el.scrollTop).toBe(1200);
          await vi.advanceTimersByTimeAsync(200);
          expect(localStorage.getItem('k')).toBe('1200');
        } finally {
          detach();
          restoreDom();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('arms no rescue when the link RESOLVED BEFORE this thread attached', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', '200');
        const el = makeEl(500, 1300);
        const restoreDom = withFindableTarget(el);
        captureObservers();

        scrollToEventAndPulse('e1');
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
        });

        try {
          await vi.advanceTimersByTimeAsync(4600);
          expect(el.scrollTop).toBe(500);
        } finally {
          detach();
          restoreDom();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('does not rescue a link that RESOLVED without moving the container', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', '3200');
        const el = makeEl(4200, 1000);
        const restoreDom = withFindableTarget(el);
        captureObservers();
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
        });

        try {
          expect(el.scrollTop).toBe(0);
          scrollToEventAndPulse('e1');
          el.scrollHeight = 20000;
          await vi.advanceTimersByTimeAsync(4600);
          expect(el.scrollTop).toBe(0);
        } finally {
          detach();
          restoreDom();
        }
      } finally {
        vi.useRealTimers();
      }
    });


    it('records a landing that moved nobody as this thread\'s position', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', '30000');
        const el = makeEl(4200, 20000);
        const restoreDom = withFindableTarget(el);
        captureObservers();
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
        });

        try {
          expect(el.scrollTop).toBe(0);
          scrollToEventAndPulse('e1');
          await vi.advanceTimersByTimeAsync(200);
          expect(localStorage.getItem('k')).toBe('0');
        } finally {
          detach();
          restoreDom();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('records a landing OFF the live edge as the offset, the link having ended the ride', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', LIVE_EDGE_VALUE);
        const el = makeEl(19200, 20000);
        const restoreDom = withFindableTarget(el, { rectTop: -15000, reducedMotion: true });
        captureObservers();
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
          followsLiveEdge: true,
        });
        setFollowLiveEdge(true);
        expect(classifyScroll(el).followSurvives).toBe(true);

        try {
          scrollToEventAndPulse('e1');
          await vi.advanceTimersByTimeAsync(200);
          expect(classifyScroll(el).followSurvives).toBe(false);
          expect(localStorage.getItem('k')).toBe(String(el.scrollTop));
          expect(localStorage.getItem('k')).not.toBe(LIVE_EDGE_VALUE);
        } finally {
          detach();
          stopFollowingBottom();
          restoreDom();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('records a landing ON the live edge as the LIVE EDGE, the ride surviving it', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', LIVE_EDGE_VALUE);
        const el = makeEl(4200, 5000);
        const restoreDom = withFindableTarget(el);
        captureObservers();
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
          followsLiveEdge: true,
        });
        setFollowLiveEdge(true);

        try {
          scrollToEventAndPulse('e1');
          await vi.advanceTimersByTimeAsync(200);
          expect(followingLiveEdge.value).toBe(true);
          expect(classifyScroll(el).followSurvives).toBe(true);
          expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
        } finally {
          detach();
          stopFollowingBottom();
          restoreDom();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('answers the same for that landing on an IDLE thread', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', LIVE_EDGE_VALUE);
        const el = makeEl(4200, 5000);
        const restoreDom = withFindableTarget(el);
        captureObservers();
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
          followsLiveEdge: true,
        });
        setFollowLiveEdge(true);

        try {
          scrollToEventAndPulse('e1');
          await vi.advanceTimersByTimeAsync(200);
          expect(followingLiveEdge.value).toBe(true);
          expect(classifyScroll(el).followSurvives).toBe(true);
          expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
        } finally {
          detach();
          stopFollowingBottom();
          restoreDom();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('resumes a recorded ride when the landing it MISSED was at the live edge', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', LIVE_EDGE_VALUE);
        const el = makeEl(4200, 5000);
        const restoreDom = withFindableTarget(el);
        captureObservers();

        scrollToEventAndPulse('e1');
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
          followsLiveEdge: true,
        });

        try {
          expect(followingLiveEdge.value).toBe(true);
          expect(el.scrollTop).toBe(4200);
          await vi.advanceTimersByTimeAsync(200);
          expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
        } finally {
          detach();
          stopFollowingBottom();
          restoreDom();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('takes over a landing still gliding to the edge, so the record stays the live edge', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', LIVE_EDGE_VALUE);
        const el = makeEl(2000, 20000);
        const restoreDom = withFindableTarget(el, { absTop: 19200 });
        captureObservers();

        scrollToEventAndPulse('e1');
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
          followsLiveEdge: true,
        });

        try {
          expect(followingLiveEdge.value).toBe(true);
          await vi.advanceTimersByTimeAsync(1500);
          expect(el.scrollTop).toBe(19200);
          el.fireScroll();
          await vi.advanceTimersByTimeAsync(200);
          expect(classifyScroll(el).followSurvives).toBe(true);
          expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
        } finally {
          detach();
          stopFollowingBottom();
          restoreDom();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('records a landing that happened BEFORE this thread attached', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', '200');
        const el = makeEl(500, 1300);
        const restoreDom = withFindableTarget(el);
        captureObservers();

        scrollToEventAndPulse('e1');
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
        });

        try {
          await vi.advanceTimersByTimeAsync(200);
          expect(localStorage.getItem('k')).toBe('500');
        } finally {
          detach();
          restoreDom();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('records the landing against the thread it is IN, never the one left', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('outgoing', '3200');
        const el = makeEl(4200, 20000);
        const restoreDom = withFindableTarget(el);
        captureObservers();
        const detach = attachScrollMemory(el, 'outgoing', {
          live: transcript,
          resetOnEmpty: true,
          isCurrent: () => false,
        });

        try {
          scrollToEventAndPulse('e1');
          await vi.advanceTimersByTimeAsync(200);
          expect(localStorage.getItem('outgoing')).toBe('3200');
        } finally {
          detach();
          restoreDom();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('leaves a container the claim is not about alone', () => {
      localStorage.setItem('k', '3200');
      const el = makeEl(0, 1000);
      const observers = captureObservers();
      const detach = attachScrollMemory(el, 'k', { live: () => ({}) });

      scrollToEventAndPulse('e1');

      el.scrollHeight = 20000;
      observers.fire();
      expect(el.scrollTop).toBe(3200);
      detach();
    });

    it('records no landing for a container the claim is not about', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', '3200');
        const el = makeEl(900, 20000);
        const restoreDom = withFindableTarget(el);
        captureObservers();
        const detach = attachScrollMemory(el, 'k', { live: () => ({}) });

        try {
          scrollToEventAndPulse('e1');
          await vi.advanceTimersByTimeAsync(200);
          expect(localStorage.getItem('k')).toBe('3200');
        } finally {
          detach();
          restoreDom();
        }
      } finally {
        vi.useRealTimers();
      }
    });


    function tapNotificationInto(el: any) {
      const restoreDom = withFindableTarget(el, { rectTop: 1000, reducedMotion: true });
      captureObservers();
      stopFollowingBottom();
      scrollToEventAndPulse('e1');
      const detach = attachScrollMemory(el, 'k', {
        live: transcript,
        resetOnEmpty: true,
        followsLiveEdge: true,
      });
      return () => { detach(); restoreDom(); };
    }

    it('ends the ride, because the LANDING answered it', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', LIVE_EDGE_VALUE);
        const el = makeEl(4200, 20000);
        const done = tapNotificationInto(el);

        try {
          expect(followingLiveEdge.value).toBe(false);
          expect(el.scrollTop).toBe(5200);
          await vi.advanceTimersByTimeAsync(200);
          expect(classifyScroll(el).followSurvives).toBe(false);
          expect(localStorage.getItem('k')).toBe('5200');
        } finally {
          done();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('does not let an older armed-and-parked record arm over a landing either', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', 'following:3200');
        const el = makeEl(4200, 20000);
        const done = tapNotificationInto(el);

        try {
          expect(followingLiveEdge.value).toBe(false);
          expect(el.scrollTop).toBe(5200);
          await vi.advanceTimersByTimeAsync(200);
          expect(localStorage.getItem('k')).toBe('5200');
        } finally {
          done();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('arms nothing from an older armed-and-parked record, for a link IN FLIGHT', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', 'following:3200');
        const el = makeEl(3200, 20000);
        captureObservers();
        stopFollowingBottom();
        scrollToEventAndPulse('never-renders');
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
          followsLiveEdge: true,
        });

        try {
          expect(followingLiveEdge.value).toBe(false);
          expect(el.scrollTop).toBe(3200);
          await vi.advanceTimersByTimeAsync(200);
          expect(localStorage.getItem('k')).toBe('following:3200');
        } finally {
          detach();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('does not let the follow SEED arm over a landing either', async () => {
      vi.useFakeTimers();
      try {
        setFollowLiveEdge(true);
        stopFollowingBottom();
        const el = makeEl(4200, 20000);
        const done = tapNotificationInto(el);

        try {
          expect(followingLiveEdge.value).toBe(false);
          expect(el.scrollTop).toBe(5200);
        } finally {
          done();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('keeps the ride for a link still IN FLIGHT, which is what the resume is for', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', LIVE_EDGE_VALUE);
        const el = makeEl(4200, 20000);
        captureObservers();
        stopFollowingBottom();
        scrollToEventAndPulse('never-renders');
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
          followsLiveEdge: true,
        });

        try {
          expect(followingLiveEdge.value).toBe(true);
          expect(el.scrollTop).toBe(4200);
        } finally {
          detach();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('arms nothing for a thread with no reading position when the seed is off', async () => {
      vi.useFakeTimers();
      try {
        setFollowLiveEdge(false);
        const el = makeEl(4200, 20000);
        const done = tapNotificationInto(el);

        try {
          expect(followingLiveEdge.value).toBe(false);
          expect(el.scrollTop).toBe(5200);
        } finally {
          done();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('never arms a container that cannot ride a live edge', async () => {
      vi.useFakeTimers();
      try {
        setFollowLiveEdge(true);
        stopFollowingBottom();
        const el = makeEl(4200, 20000);
        const restoreDom = withFindableTarget(el, { rectTop: 1000, reducedMotion: true });
        captureObservers();
        scrollToEventAndPulse('e1');
        const detach = attachScrollMemory(el, 'k', { live: transcript, resetOnEmpty: true });

        try {
          expect(followingLiveEdge.value).toBe(false);
        } finally {
          detach();
          restoreDom();
        }
      } finally {
        vi.useRealTimers();
      }
    });
  });

  function withDeepLink() {
    let claimHeld = true;
    return {
      live: () => ({ shouldRestore: () => !claimHeld }),
      release: () => { claimHeld = false; },
    };
  }

  function withUnfindableTarget() {
    const origQSA = (globalThis.document as any).querySelectorAll;
    (globalThis.document as any).querySelectorAll = () => [];
    return () => { (globalThis.document as any).querySelectorAll = origQSA; };
  }

  it('defers the top RESET to a deep-link too, then rescues a dead one', async () => {
    vi.useFakeTimers();
    try {
      const el = makeEl(4200, 20000);
      const link = withDeepLink();
      const detach = attachScrollMemory(el, 'k', { live: link.live, resetOnEmpty: true });

      expect(el.scrollTop).toBe(4200);

      await vi.advanceTimersByTimeAsync(4000);
      link.release();
      await vi.advanceTimersByTimeAsync(600);

      expect(el.scrollTop).toBe(0);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('waits out a link still holding its claim, then rescues once it lets go', async () => {
    vi.useFakeTimers();
    try {
      localStorage.setItem('k', '3200');
      const el = makeEl(4200, 20000);
      const link = withDeepLink();
      const detach = attachScrollMemory(el, 'k', { live: link.live, resetOnEmpty: true });

      const restoreDom = withUnfindableTarget();
      scrollToEventAndPulse('e-slow', { stillArriving: () => true });

      try {
        await vi.advanceTimersByTimeAsync(9000);
        expect(hasPendingEventScroll()).toBe(true);
        expect(el.scrollTop).toBe(4200);

        clearPendingEventScroll();
        link.release();
        await vi.advanceTimersByTimeAsync(5000);

        expect(el.scrollTop).toBe(3200);
      } finally {
        restoreDom();
        detach();
      }
    } finally {
      vi.useRealTimers();
    }
  });

  it('rescues a SAVED position the same way when the link turns out dead', async () => {
    vi.useFakeTimers();
    try {
      localStorage.setItem('k', '3200');
      const el = makeEl(4200, 20000);
      const link = withDeepLink();
      const detach = attachScrollMemory(el, 'k', { live: link.live, resetOnEmpty: true });

      expect(el.scrollTop).toBe(4200);

      await vi.advanceTimersByTimeAsync(4000);
      link.release();
      await vi.advanceTimersByTimeAsync(600);

      expect(el.scrollTop).toBe(3200);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('rescues to the TOP, never the live edge, when the saved offset is out of reach', async () => {
    vi.useFakeTimers();
    try {
      localStorage.setItem('k', '12000');
      const el = makeEl(4200, 5000);
      const link = withDeepLink();
      const detach = attachScrollMemory(el, 'k', { live: link.live, resetOnEmpty: true });

      await vi.advanceTimersByTimeAsync(4000);
      link.release();
      await vi.advanceTimersByTimeAsync(600);

      expect(el.scrollTop).toBe(0);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('leaves the rescue alone once anything has moved the container', async () => {
    vi.useFakeTimers();
    try {
      const el = makeEl(4200, 20000);
      const link = withDeepLink();
      const detach = attachScrollMemory(el, 'k', { live: link.live, resetOnEmpty: true });

      el.scrollTop = 8800;

      await vi.advanceTimersByTimeAsync(4000);
      link.release();
      await vi.advanceTimersByTimeAsync(600);

      expect(el.scrollTop).toBe(8800);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('stands down when a NEWER deep-link owns the open by the time it fires', async () => {
    vi.useFakeTimers();
    try {
      const el = makeEl(4200, 20000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({ shouldRestore: () => false }),
        resetOnEmpty: true,
      });

      await vi.advanceTimersByTimeAsync(4600);
      expect(el.scrollTop).toBe(4200);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('stands down when this key stopped being the current one', async () => {
    vi.useFakeTimers();
    try {
      const el = makeEl(4200, 20000);
      const link = withDeepLink();
      let current = true;
      const detach = attachScrollMemory(el, 'k', {
        live: link.live,
        resetOnEmpty: true,
        isCurrent: () => current,
      });

      current = false;
      await vi.advanceTimersByTimeAsync(4000);
      link.release();
      await vi.advanceTimersByTimeAsync(600);

      expect(el.scrollTop).toBe(4200);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('cancels the rescue when the thread is switched away inside the window', async () => {
    vi.useFakeTimers();
    try {
      const el = makeEl(4200, 20000);
      const link = withDeepLink();
      const detach = attachScrollMemory(el, 'k', { live: link.live, resetOnEmpty: true });

      detach();
      link.release();
      await vi.advanceTimersByTimeAsync(4600);
      expect(el.scrollTop).toBe(4200);
    } finally {
      vi.useRealTimers();
    }
  });

  it('detaches its scroll listener', () => {
    const el = makeEl(0);
    const detach = attachScrollMemory(el, 'k', { live: () => ({}) });
    expect(el.listenerCount()).toBe(1);
    detach();
    expect(el.listenerCount()).toBe(0);
  });

  describe('a standing follow survives leaving the thread', () => {
    function armViaToggle(el: any) {
      setActiveScrollElement(el);
      el.scrollTop = Math.max(0, el.scrollHeight - el.clientHeight);
      setFollowLiveEdge(true);
    }

    beforeEach(() => { stopFollowingBottom(); setTranscriptLive(true); setActiveScrollElement(null); });
    afterEach(() => { stopFollowingBottom(); setTranscriptLive(true); setActiveScrollElement(null); });


    it('arms a thread with NO reading position when the seed is on', () => {
      setFollowLiveEdge(true);
      stopFollowingBottom();

      const el = makeEl(300, 5000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });

      expect(classifyScroll(el).followSurvives).toBe(true);
      expect(el.scrollTop).toBe(5000 - 800);
      detach();
    });

    it('records the seeded arm, so the thread owns the answer from then on', () => {
      setFollowLiveEdge(true);
      stopFollowingBottom();

      const el = makeEl(4200, 5000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });
      detach();

      expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
    });

    it('leaves a thread with NO reading position alone when the seed is off', () => {
      setFollowLiveEdge(false);

      const el = makeEl(300, 5000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });

      expect(classifyScroll(el).followSurvives).toBe(false);
      expect(el.scrollTop).toBe(0);
      detach();
    });

    it('lets a RECORDED offset beat the seed', () => {
      setFollowLiveEdge(true);
      stopFollowingBottom();
      localStorage.setItem('k', '2000');

      const el = makeEl(300, 5000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });

      expect(classifyScroll(el).followSurvives).toBe(false);
      expect(el.scrollTop).toBe(2000);
      detach();
    });

    it('lets a RECORDED live edge beat the seed being off', () => {
      setFollowLiveEdge(false);
      localStorage.setItem('k', LIVE_EDGE_VALUE);

      const el = makeEl(300, 5000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });

      expect(classifyScroll(el).followSurvives).toBe(true);
      detach();
    });

    it('never seeds a container that is not the transcript', () => {
      setFollowLiveEdge(true);
      stopFollowingBottom();

      const el = makeEl(300, 5000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

      expect(classifyScroll(el).followSurvives).toBe(false);
      detach();
    });

    it('records the live edge rather than the offset the follow happened to reach', () => {
      const el = makeEl(100, 5000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), followsLiveEdge: true });

      armViaToggle(el);
      el.fireScroll();
      el.scrollHeight = 9000;
      el.scrollTop = 8200;
      detach();

      expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
    });

    it('records the request even when arming produced no scroll event at all', () => {
      const el = makeEl(4200, 5000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), followsLiveEdge: true });

      armViaToggle(el);
      detach();

      expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
    });

    it('records a bare offset when a gesture on a LIVE thread ends the ride', () => {
      const el = makeEl(100, 5000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), followsLiveEdge: true });

      armViaToggle(el);
      readerGestureForTest(el);
      el.scrollTop = 2000;
      el.fireScroll();
      detach();

      expect(localStorage.getItem('k')).toBe('2000');
    });

    it('records the same thing when the disarm gets there FIRST', () => {
      const el = makeEl(100, 5000);
      const observers = makeScrollObservers(el);
      el.addEventListener('scroll', observers.onScroll);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), followsLiveEdge: true });

      armViaToggle(el);
      readerGestureForTest(el);
      el.scrollTop = 2000;
      el.fireScroll();
      detach();
      observers.detachGestures();

      expect(followingLiveEdge.value).toBe(false);
      expect(localStorage.getItem('k')).toBe('2000');
    });

    it('records the PLACE when a gesture on a WAITING thread parks the ride', () => {
      setTranscriptLive(false);
      const el = makeEl(100, 5000);
      const observers = makeScrollObservers(el);
      el.addEventListener('scroll', observers.onScroll);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), followsLiveEdge: true });

      armViaToggle(el);
      readerGestureForTest(el);
      el.scrollTop = 2000;
      el.fireScroll();
      detach();
      observers.detachGestures();

      expect(followingLiveEdge.value).toBe(true);
      expect(localStorage.getItem('k')).toBe('2000');
    });

    it('restores an older armed-and-parked record as a place, holding no follow', () => {
      localStorage.setItem('k', 'following:2000');
      const el = makeEl(0, 5000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });

      expect(el.scrollTop).toBe(2000);
      expect(followingLiveEdge.value).toBe(false);
      detach();
    });

    it('keeps the live edge on the thread being LEFT when focusThread retires the follow', () => {
      const el = makeEl(100, 5000);
      let current = true;
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        followsLiveEdge: true,
        isCurrent: () => current,
      });

      armViaToggle(el);
      el.fireScroll();

      stopFollowingBottom();
      current = false;
      el.scrollTop = 120;
      el.fireScroll();
      detach();

      expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
    });

    it('does not record an arm made after this key stopped being the current one', () => {
      localStorage.setItem('k', '1800');
      const el = makeEl(1800, 5000);
      let current = true;
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        followsLiveEdge: true,
        isCurrent: () => current,
      });

      current = false;
      armViaToggle(el);
      detach();

      expect(localStorage.getItem('k')).toBe('1800');
    });

    it('returns the reader to TODAY\'s live edge, not the offset it was when they left', () => {
      localStorage.setItem('k', LIVE_EDGE_VALUE);
      const el = makeEl(120, 20000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });

      expect(el.scrollTop).toBe(19200);
      expect(classifyScroll(el).followSurvives).toBe(true);
      detach();
    });

    it('leaves an UNARMED reader at the bottom exactly where they were', () => {
      const el = makeEl(100, 5000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), followsLiveEdge: true });
      el.scrollTop = 4200;
      el.fireScroll();
      detach();
      expect(localStorage.getItem('k')).toBe('4200');

      const grown = makeEl(0, 20000);
      const detach2 = attachScrollMemory(grown, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });
      expect(grown.scrollTop).toBe(4200);
      expect(classifyScroll(grown).followSurvives).toBe(false);
      detach2();
    });

    it('never records or restores the live edge for a container that cannot ride one', () => {
      localStorage.setItem('k', LIVE_EDGE_VALUE);
      const el = makeEl(4200, 20000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

      expect(el.scrollTop).toBe(0);

      armViaToggle(el);
      el.scrollTop = 3000;
      el.fireScroll();
      detach();

      expect(localStorage.getItem('k')).toBe('3000');
    });

    it('rescues a dead deep-link into a followed thread at the live edge', async () => {
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', LIVE_EDGE_VALUE);
        const el = makeEl(4200, 20000);
        const link = withDeepLink();
        const detach = attachScrollMemory(el, 'k', {
          live: link.live,
          resetOnEmpty: true,
          followsLiveEdge: true,
        });

        expect(el.scrollTop).toBe(4200);

        await vi.advanceTimersByTimeAsync(4000);
        link.release();
        await vi.advanceTimersByTimeAsync(600);

        expect(el.scrollTop).toBe(19200);
        expect(classifyScroll(el).followSurvives).toBe(true);
        detach();
      } finally {
        vi.useRealTimers();
      }
    });

    it('hears the scroll class the engine, the bars and the indicator hear', () => {
      setFollowLiveEdge(true);
      stopFollowingBottom();
      const el = makeEl(300, 5000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });
      const { onScroll, detachGestures } = makeScrollObservers(el);
      const seen: Record<string, ScrollClass[]> = { engine: [], bars: [], indicator: [] };
      const stops = [
        onClassifiedScroll(el, (scroll) => seen.bars.push(scroll)),
        onClassifiedScrollAsEngine(el, (scroll) => { seen.engine.push(scroll); return onScroll(scroll); }),
        onClassifiedScroll(el, (scroll) => seen.indicator.push(scroll)),
      ];
      try {
        el.fireScroll();
        expect(seen.engine).toHaveLength(1);
        expect(seen.bars[0]).toBe(seen.engine[0]);
        expect(seen.indicator[0]).toBe(seen.engine[0]);
        expect(seen.engine[0].followSurvives).toBe(true);
        detach();
        expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
      } finally {
        for (const stop of stops) stop();
        detachGestures();
      }
    });

    it('tells the consumers of a carry that the app wrote it', () => {
      vi.useFakeTimers();
      setFollowLiveEdge(true);
      stopFollowingBottom();
      const el = makeEl(300, 5000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });
      const { onScroll, detachGestures } = makeScrollObservers(el);
      const told: ScrollClass[] = [];
      const raw: ScrollClass[] = [];
      const stops = [
        onClassifiedScrollAsEngine(el, onScroll),
        onClassifiedScroll(el, (scroll, event) => { told.push(scroll); raw.push(event); }),
      ];
      try {
        vi.advanceTimersByTime(1000);
        el.scrollTop = 3900;
        el.fireScroll();
        expect(el.scrollTop).toBe(5000 - 800);
        expect(told).toHaveLength(1);
        expect(raw[0].navigation).toBe(false);
        expect(told[0].navigation).toBe(true);
        expect(told[0].kind).toBe('carry');
        expect(told[0].onLastWrite).toBe(true);
        expect(told[0].followSurvives).toBe(true);
      } finally {
        vi.useRealTimers();
        for (const stop of stops) stop();
        detachGestures();
        detach();
      }
    });

    it('records the live edge for a parked reader who scrolls back onto it', () => {
      setTranscriptLive(false);
      setFollowLiveEdge(true);
      stopFollowingBottom();
      const el = makeEl(300, 5000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });
      const { onScroll, detachGestures } = makeScrollObservers(el);
      const told: ScrollClass[] = [];
      const stops = [
        onClassifiedScrollAsEngine(el, onScroll),
        onClassifiedScroll(el, (scroll) => told.push(scroll)),
      ];
      try {
        readerGestureForTest(el);
        el.scrollTop = 2000;
        el.fireScroll();
        expect(told[0].followSurvives).toBe(false);
        readerGestureForTest(el);
        el.scrollTop = 5000 - 800;
        el.fireScroll();
        expect(told[1].followSurvives).toBe(true);
        detach();
        expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
      } finally {
        for (const stop of stops) stop();
        detachGestures();
      }
    });
  });

  describe('a standing follow survives the app being backgrounded', () => {
    let page: ReturnType<typeof installFakePage>;

    beforeEach(() => {
      _resetPageVisitForTesting();
      page = installFakePage();
      stopFollowingBottom();
      setActiveScrollElement(null);
    });
    afterEach(() => {
      _resetPageVisitForTesting();
      page.restore();
      stopFollowingBottom();
      setActiveScrollElement(null);
    });

    function arriveFollowing(el: any, opts: Record<string, unknown> = {}) {
      localStorage.setItem('k', LIVE_EDGE_VALUE);
      return attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
        ...opts,
      });
    }

    it('lands on the live edge the thread reached WHILE the app was away', () => {
      const el = makeEl(0, 5000);
      const detach = arriveFollowing(el);
      expect(el.scrollTop).toBe(4200);

      page.background();
      el.scrollHeight = 20000;
      page.foreground();

      expect(el.scrollTop).toBe(19200);
      expect(classifyScroll(el).followSurvives).toBe(true);
      detach();
    });

    it('moves a reader who scrolled away BEFORE backgrounding zero pixels', () => {
      const el = makeEl(0, 20000);
      const detach = arriveFollowing(el);
      expect(el.scrollTop).toBe(19200);

      readerGestureForTest(el);
      el.scrollTop = 6000;
      el.fireScroll();
      expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);

      page.background();
      expect(localStorage.getItem('k')).toBe('6000');

      el.scrollHeight = 40000;
      page.foreground();

      expect(el.scrollTop).toBe(6000);
      expect(localStorage.getItem('k')).toBe('6000');
      detach();
    });

    it('reads what is STORED on wake, not what was saved when it attached', () => {
      localStorage.setItem('k', '400');
      const el = makeEl(400, 20000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });

      setActiveScrollElement(el);
      el.scrollTop = 19200;
      setFollowLiveEdge(true);
      page.background();
      expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);

      el.scrollHeight = 60000;
      page.foreground();

      expect(el.scrollTop).toBe(59200);
      detach();
    });

    it('keeps recording after a background, because a hide is not a teardown', () => {
      const el = makeEl(0, 20000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), followsLiveEdge: true });

      el.scrollTop = 3000;
      el.fireScroll();
      page.background();
      expect(localStorage.getItem('k')).toBe('3000');

      page.foreground();
      el.scrollTop = 7000;
      el.fireScroll();
      detach();

      expect(localStorage.getItem('k')).toBe('7000');
    });

    it('does not re-assert for a key that stopped being the current one', () => {
      const el = makeEl(0, 20000);
      let current = true;
      const detach = arriveFollowing(el, { isCurrent: () => current });
      expect(el.scrollTop).toBe(19200);

      current = false;
      el.scrollTop = 500;
      page.background();
      page.foreground();

      expect(el.scrollTop).toBe(500);
      detach();
    });

    it('never re-asserts a container that cannot ride a live edge', () => {
      localStorage.setItem('k', LIVE_EDGE_VALUE);
      const el = makeEl(300, 20000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });
      expect(el.scrollTop).toBe(0);

      page.background();
      page.foreground();

      expect(el.scrollTop).toBe(0);
      detach();
    });

    it('defers to a deep-link that owns the open, and keeps the ride anyway', () => {
      localStorage.setItem('k', LIVE_EDGE_VALUE);
      const el = makeEl(4200, 20000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({ shouldRestore: () => false }),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });
      expect(el.scrollTop).toBe(4200);
      expect(followingLiveEdge.value).toBe(true);

      page.background();
      page.foreground();

      expect(el.scrollTop).toBe(4200);
      expect(followingLiveEdge.value).toBe(true);
      detach();
    });

    it('rebuilds a ride the wake itself destroyed, still without writing', () => {
      localStorage.setItem('k', LIVE_EDGE_VALUE);
      const el = makeEl(4200, 20000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({ shouldRestore: () => false }),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });

      page.background();
      stopFollowingBottom();
      el.scrollHeight = 40000;
      page.foreground();

      expect(followingLiveEdge.value).toBe(true);
      expect(el.scrollTop).toBe(4200);
      detach();
    });
  });
});

describe('a reading position that names a turn', () => {
  const IDS = ['t0', 't1', 't2', 't3', 't4', 't5'];
  const TURN = 400;
  let origRO: unknown;
  let origMO: unknown;

  beforeEach(() => {
    localStorage.clear();
    origRO = (globalThis as any).ResizeObserver;
    origMO = (globalThis as any).MutationObserver;
  });
  afterEach(() => {
    (globalThis as any).ResizeObserver = origRO;
    (globalThis as any).MutationObserver = origMO;
  });

  function inertObservers() {
    class Inert {
      observe() {}
      disconnect() {}
      takeRecords() { return []; }
    }
    (globalThis as any).ResizeObserver = Inert;
    (globalThis as any).MutationObserver = Inert;
  }

  function liveObservers() {
    type Entry = { cb: () => void; content: boolean };
    const entries: Entry[] = [];
    class Capturing {
      entry: Entry;
      constructor(cb: () => void) { this.entry = { cb, content: false }; entries.push(this.entry); }
      observe(_target?: unknown, options?: MutationObserverInit) {
        if (options?.attributeFilter) this.entry.content = true;
      }
      disconnect() {
        const i = entries.indexOf(this.entry);
        if (i >= 0) entries.splice(i, 1);
      }
      takeRecords() { return []; }
    }
    (globalThis as any).ResizeObserver = Capturing;
    (globalThis as any).MutationObserver = Capturing;
    return {
      fire: () => { for (const e of [...entries]) e.cb(); },
      armed: () => entries.filter(e => !e.content).length,
      contentArmed: () => entries.filter(e => e.content).length,
    };
  }

  const opts = { live: () => ({}), resetOnEmpty: true, anchorsToContent: true };

  it('records the turn and its exact offset, not the scroll position', () => {
    inertObservers();
    const el = mockTranscript({ ids: IDS, turnHeight: TURN });
    const detach = attachScrollMemory(el, 'k', opts);

    el.scrollTop = 950;
    el.fireScroll();
    detach();

    expect(localStorage.getItem('k')).toBe('anchor:-150:t2');
    expect(parseSavedScroll(localStorage.getItem('k')))
      .toEqual({ kind: 'anchor', eventId: 't2', relTop: -150 });
  });

  it('opens on the same turn in a window too short to hold the old offset', () => {
    inertObservers();
    localStorage.setItem('k', 'anchor:-150:t2');
    const el = mockTranscript({ ids: IDS, renderFrom: 2, turnHeight: TURN, scrollTop: 0 });

    const detach = attachScrollMemory(el, 'k', opts);

    expect(el.scrollTop).toBe(150);
    detach();
  });

  it('opens on the same turn after the thread gained newer ones', () => {
    inertObservers();
    localStorage.setItem('k', 'anchor:-150:t2');
    const grown = mockTranscript({ ids: [...IDS, 't6', 't7'], turnHeight: TURN, scrollTop: 0 });

    const detach = attachScrollMemory(grown, 'k', opts);

    expect(grown.scrollTop).toBe(950);
    detach();
  });

  it('waits for the window to reach the turn, then lands on it', () => {
    const obs = liveObservers();
    localStorage.setItem('k', 'anchor:-150:t1');
    const el = mockTranscript({ ids: IDS, renderFrom: 4, turnHeight: TURN, scrollTop: 0 });

    const detach = attachScrollMemory(el, 'k', opts);
    expect(el.scrollTop).toBe(0);
    expect(obs.armed()).toBeGreaterThan(0);

    el.growWindowTo(2);
    obs.fire();
    expect(el.scrollTop).toBe(0);
    expect(obs.armed()).toBeGreaterThan(0);

    el.growWindowTo(1);
    obs.fire();
    expect(el.scrollTop).toBe(150);
    expect(obs.armed()).toBe(0);
    detach();
  });

  it('extends the wait across a SHRINK, not only across growth', async () => {
    vi.useFakeTimers();
    try {
      const obs = liveObservers();
      localStorage.setItem('k', 'anchor:-150:t1');
      const el = mockTranscript({ ids: IDS, renderFrom: 4, turnHeight: TURN, scrollTop: 0 });

      const detach = attachScrollMemory(el, 'k', opts);
      await vi.advanceTimersByTimeAsync(2000);
      el.setTurnHeight(200);
      obs.fire();
      await vi.advanceTimersByTimeAsync(2000);
      el.growWindowTo(0);
      obs.fire();

      expect(el.scrollTop).toBe(350);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('lands a RESOLVED anchor the content shrank under AT ONCE, and never moves again', async () => {
    vi.useFakeTimers();
    try {
      const obs = liveObservers();
      localStorage.setItem('k', 'anchor:0:t4');
      const el = mockTranscript({ ids: IDS, renderFrom: 0, turnHeight: TURN, clientHeight: 800 });
      el.setTurnHeightOf('t5', 100);

      const detach = attachScrollMemory(el, 'k', opts);

      expect(el.scrollHeight).toBe(2100);
      expect(el.scrollTop).toBe(1300);
      expect(obs.armed()).toBe(0);

      el.setTurnHeightOf('t5', 400);
      obs.fire();
      await vi.advanceTimersByTimeAsync(25_000);
      expect(el.scrollTop).toBe(1300);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('waits for a turn drawn with its HEAD CLAMPED, then lands on it exactly', () => {
    const obs = liveObservers();
    localStorage.setItem('k', 'anchor:-600:t4');
    const el = mockTranscript({ ids: IDS, renderFrom: 4, turnHeight: TURN, clientHeight: 400 });
    el.setTurnHeightOf('t4', 300);
    el.setHeadClamped('t4', true);

    const detach = attachScrollMemory(el, 'k', opts);
    expect(el.scrollTop).toBe(0);
    expect(obs.armed()).toBeGreaterThan(0);

    el.setTurnHeightOf('t4', 1200);
    el.setHeadClamped('t4', false);
    obs.fire();
    expect(el.scrollTop).toBe(600);
    expect(obs.armed()).toBe(0);
    detach();
  });

  it('lands a bottom-parked anchor at once when only ROUNDING puts it out of reach', async () => {
    vi.useFakeTimers();
    try {
      inertObservers();
      const el = mockTranscript({ ids: IDS, renderFrom: 0, turnHeight: TURN, clientHeight: 800 });
      localStorage.setItem('k', 'anchor:-1:t4');

      const detach = attachScrollMemory(el, 'k', opts);

      expect(el.scrollTop).toBe(1600);
      await vi.advanceTimersByTimeAsync(4000);
      expect(el.scrollTop).toBe(1600);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps the record when a scroll arrives on a transcript nobody can measure', async () => {
    vi.useFakeTimers();
    try {
      inertObservers();
      localStorage.setItem('k', 'anchor:-150:t2');
      const el = mockTranscript({ ids: IDS, renderFrom: 0, turnHeight: TURN, clientHeight: 800 });

      const detach = attachScrollMemory(el, 'k', opts);
      await vi.advanceTimersByTimeAsync(4000);

      el.collapse();
      el.fireScroll();
      await vi.advanceTimersByTimeAsync(1000);
      detach();

      expect(localStorage.getItem('k')).toBe('anchor:-150:t2');
    } finally {
      vi.useRealTimers();
    }
  });

  it('a deep-link stand-down does not SETTLE the restore; the rescue does', async () => {
    vi.useFakeTimers();
    try {
      inertObservers();
      localStorage.setItem('k', 'anchor:-150:t2');
      const el = mockTranscript({ ids: IDS, renderFrom: 0, turnHeight: TURN, scrollTop: 0 });
      let claimHeld = true;
      let settled = 0;

      const detach = attachScrollMemory(el, 'k', {
        live: () => ({ shouldRestore: () => !claimHeld, onRestoreSettled: () => { settled++; } }),
        resetOnEmpty: true,
        anchorsToContent: true,
      });

      expect(settled).toBe(0);

      claimHeld = false;
      await vi.advanceTimersByTimeAsync(5000);

      expect(settled).toBe(1);
      detach();
      expect(settled).toBe(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('holds the wait open while the pane has NO BOX, and lands when it is revealed', async () => {
    vi.useFakeTimers();
    try {
      const obs = liveObservers();
      localStorage.setItem('k', 'anchor:-150:t2');
      const el = mockTranscript({ ids: IDS, renderFrom: 0, turnHeight: TURN, clientHeight: 800 });
      el.collapse();

      const detach = attachScrollMemory(el, 'k', opts);
      await vi.advanceTimersByTimeAsync(9000);
      expect(obs.armed()).toBeGreaterThan(0);

      el.reveal(800);
      obs.fire();

      expect(el.scrollTop).toBe(950);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('stops re-arming at the CEILING, so a growing transcript cannot wait for ever', async () => {
    vi.useFakeTimers();
    try {
      const obs = liveObservers();
      localStorage.setItem('k', 'anchor:-150:gone');
      const el = mockTranscript({ ids: IDS, renderFrom: 4, turnHeight: TURN, scrollTop: 0 });

      const detach = attachScrollMemory(el, 'k', opts);
      for (let second = 0; second < 25; second++) {
        el.setTurnHeight(TURN + second);
        obs.fire();
        await vi.advanceTimersByTimeAsync(1000);
      }

      expect(obs.armed()).toBe(0);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('leaves the reader at the TOP, never the live edge, when the turn never renders', async () => {
    vi.useFakeTimers();
    try {
      inertObservers();
      localStorage.setItem('k', 'anchor:-150:gone');
      const el = mockTranscript({ ids: IDS, renderFrom: 4, turnHeight: TURN, scrollTop: 0 });

      const detach = attachScrollMemory(el, 'k', opts);
      await vi.advanceTimersByTimeAsync(4000);

      expect(el.scrollTop).toBe(0);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('a container that cannot name its children ignores an anchor record', () => {
    inertObservers();
    localStorage.setItem('k', 'anchor:-150:t2');
    const el = mockTranscript({ ids: IDS, turnHeight: TURN, scrollTop: 640 });

    const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

    expect(el.scrollTop).toBe(0);
    el.fireScroll();
    detach();
    expect(localStorage.getItem('k')).toBe('0');
  });

  it('an armed follow still records the live edge, never a turn', () => {
    inertObservers();
    osReducesMotion.value = true;
    const el = mockTranscript({ ids: IDS, turnHeight: TURN });
    setActiveScrollElement(el);
    const detach = attachScrollMemory(el, 'k', { ...opts, followsLiveEdge: true });
    setFollowLiveEdge(true);

    el.fireScroll();
    detach();

    expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
    setActiveScrollElement(null);
    osReducesMotion.value = false;
  });

  it('keeps the live edge when a SHRINK clamps the rider down', () => {
    inertObservers();
    osReducesMotion.value = true;
    const el = mockTranscript({ ids: IDS, turnHeight: TURN });
    setActiveScrollElement(el);
    const detach = attachScrollMemory(el, 'k', { ...opts, followsLiveEdge: true });
    setFollowLiveEdge(true);
    el.fireScroll();

    el.setTurnHeightOf('t5', 100);
    el.reclamp();
    el.fireScroll();
    detach();

    expect(followingLiveEdge.value).toBe(true);
    expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
    setActiveScrollElement(null);
    osReducesMotion.value = false;
  });

  it('records the TURN alone when the rider scrolls up, the ride ending there', () => {
    inertObservers();
    osReducesMotion.value = true;
    const el = mockTranscript({ ids: IDS, turnHeight: TURN });
    setActiveScrollElement(el);
    const detach = attachScrollMemory(el, 'k', { ...opts, followsLiveEdge: true });
    setFollowLiveEdge(true);
    el.fireScroll();

    readerGestureForTest(el);
    el.scrollTop = 950;
    el.fireScroll();
    detach();

    expect(localStorage.getItem('k')).toBe('anchor:-150:t2');
    setActiveScrollElement(null);
    osReducesMotion.value = false;
  });

  it('opens an older armed turn record on that turn, holding no follow', () => {
    inertObservers();
    localStorage.setItem('k', 'following:anchor:-150:t2');
    const el = mockTranscript({ ids: [...IDS, 't6', 't7'], turnHeight: TURN, scrollTop: 0 });

    const detach = attachScrollMemory(el, 'k', { ...opts, followsLiveEdge: true });

    expect(el.scrollTop).toBe(950);
    expect(followingLiveEdge.value).toBe(false);
    detach();
  });

  it('follows the turn at the top when the CONTENT changed under a still reader', async () => {
    vi.useFakeTimers();
    try {
      const obs = liveObservers();
      const el = mockTranscript({ ids: IDS, renderFrom: 2, turnHeight: TURN, scrollTop: 0 });
      const detach = attachScrollMemory(el, 'k', opts);
      expect(obs.contentArmed()).toBe(1);

      el.fireScroll();
      await vi.advanceTimersByTimeAsync(200);
      expect(localStorage.getItem('k')).toBe('anchor:0:t2');

      el.growWindowTo(0);
      obs.fire();
      await vi.advanceTimersByTimeAsync(400);

      expect(localStorage.getItem('k')).toBe('anchor:0:t0');
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('opens no record from content alone, so an untouched thread keeps none', async () => {
    vi.useFakeTimers();
    try {
      const obs = liveObservers();
      const el = mockTranscript({ ids: IDS, renderFrom: 2, turnHeight: TURN, scrollTop: 0 });
      const detach = attachScrollMemory(el, 'k', opts);

      el.growWindowTo(0);
      obs.fire();
      await vi.advanceTimersByTimeAsync(400);
      detach();

      expect(localStorage.getItem('k')).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it('a container recording a bare offset watches no content', () => {
    const obs = liveObservers();
    const el = mockTranscript({ ids: IDS, turnHeight: TURN, scrollTop: 0 });

    const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

    expect(obs.contentArmed()).toBe(0);
    detach();
  });

  it('a container that cannot ride drops an older marker and keeps the place', () => {
    inertObservers();
    localStorage.setItem('k', 'following:640');
    const el = mockTranscript({ ids: IDS, turnHeight: TURN, scrollTop: 0 });

    const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

    expect(el.scrollTop).toBe(640);
    expect(followingLiveEdge.value).toBe(false);
    detach();
  });
});

describe('a reading position that names a step row', () => {
  const IDS = ['t0', 't1', 't2'];
  const opts = { live: () => ({}), resetOnEmpty: true, anchorsToContent: true };
  let origRO: unknown;
  let origMO: unknown;

  beforeEach(() => {
    localStorage.clear();
    origRO = (globalThis as any).ResizeObserver;
    origMO = (globalThis as any).MutationObserver;
    class Inert { observe() {} disconnect() {} takeRecords() { return []; } }
    (globalThis as any).ResizeObserver = Inert;
    (globalThis as any).MutationObserver = Inert;
  });
  afterEach(() => {
    (globalThis as any).ResizeObserver = origRO;
    (globalThis as any).MutationObserver = origMO;
  });

  it('parses its stored form, an older armed one too, and nothing that only looks like it', () => {
    expect(parseSavedScroll('row:-10:call-7')).toEqual({ kind: 'row', rowEventId: 'call-7', relTop: -10 });
    expect(parseSavedScroll('following:row:0:call-7'))
      .toEqual({ kind: 'row', rowEventId: 'call-7', relTop: 0 });
    expect(parseSavedScroll('row:abc:call-7')).toBeNull();
    expect(parseSavedScroll('row:-10:')).toBeNull();
  });

  it('records the row at the line in preference to its turn', () => {
    const el = mockTranscript({ ids: IDS, turnHeight: 1000, rowsPerTurn: 40 });
    const detach = attachScrollMemory(el, 'k', opts);
    el.scrollTop = 1310;
    el.fireScroll();
    detach();
    expect(localStorage.getItem('k')).toBe('row:-10:t1-r12');
  });

  it('still records the turn where the turn has no rows', () => {
    const el = mockTranscript({ ids: IDS, turnHeight: 1000 });
    const detach = attachScrollMemory(el, 'k', opts);
    el.scrollTop = 1310;
    el.fireScroll();
    detach();
    expect(localStorage.getItem('k')).toBe('anchor:-310:t1');
  });

  it('opens on the row with the turn drawn whole, where it was saved on a clamped one', () => {
    localStorage.setItem('k', 'row:0:t0-r32');
    const el = mockTranscript({ ids: IDS, turnHeight: 1000, rowsPerTurn: 40, scrollTop: 0 });
    const detach = attachScrollMemory(el, 'k', opts);
    expect(el.scrollTop).toBe(800);
    detach();
  });

  it('waits while the row is clamped off, rather than landing anywhere', () => {
    localStorage.setItem('k', 'row:0:t0-r5');
    const el = mockTranscript({ ids: IDS, turnHeight: 1000, rowsPerTurn: 40, scrollTop: 0 });
    el.setRowsHidden('t0', 20);
    const detach = attachScrollMemory(el, 'k', opts);
    expect(el.scrollTop).toBe(0);
    detach();
  });

  it('a content pane or drawer never reads a row it could not have written', () => {
    localStorage.setItem('k', 'row:0:t0-r5');
    const el = mockTranscript({ ids: IDS, turnHeight: 1000, rowsPerTurn: 40, scrollTop: 700 });
    const detach = attachScrollMemory(el, 'k', { live: () => ({}) });
    expect(el.scrollTop).toBe(700);
    detach();
  });
});

describe('a restored reading position holds through content growth', () => {
  const IDS = ['t0', 't1', 't2', 't3', 't4', 't5'];
  const opts = { live: () => ({}), resetOnEmpty: true, anchorsToContent: true };
  let origRO: unknown;
  let origMO: unknown;

  beforeEach(() => {
    localStorage.clear();
    clearPendingEventScroll();
    origRO = (globalThis as any).ResizeObserver;
    origMO = (globalThis as any).MutationObserver;
    class Inert { observe() {} disconnect() {} takeRecords() { return []; } }
    (globalThis as any).ResizeObserver = Inert;
    (globalThis as any).MutationObserver = Inert;
  });
  afterEach(() => {
    clearPendingEventScroll();
    (globalThis as any).ResizeObserver = origRO;
    (globalThis as any).MutationObserver = origMO;
  });

  it('keeps a restored turn in place when a turn above it grows', () => {
    localStorage.setItem('k', 'anchor:-150:t2');
    const el = mockTranscript({ ids: IDS, turnHeight: 400, scrollTop: 0 });
    const { onResize } = makeScrollObservers(el);
    const detach = attachScrollMemory(el, 'k', opts);
    expect(el.scrollTop).toBe(950);

    el.setTurnHeightOf('t0', 900);
    onResize();

    expect(el.scrollTop).toBe(1450);
    detach();
  });

  it('keeps a restored row in place when a turn above it grows', () => {
    localStorage.setItem('k', 'row:-10:t1-r12');
    const el = mockTranscript({ ids: ['t0', 't1', 't2'], turnHeight: 1000, rowsPerTurn: 40, scrollTop: 0 });
    const { onResize } = makeScrollObservers(el);
    const detach = attachScrollMemory(el, 'k', opts);
    expect(el.scrollTop).toBe(1310);

    el.setTurnHeightOf('t0', 1600);
    onResize();

    expect(el.scrollTop).toBe(1910);
    detach();
  });

  it('moves nobody when content grows below the restored spot', () => {
    localStorage.setItem('k', 'anchor:-150:t1');
    const el = mockTranscript({ ids: IDS, turnHeight: 400, scrollTop: 0 });
    const { onResize } = makeScrollObservers(el);
    const detach = attachScrollMemory(el, 'k', opts);
    expect(el.scrollTop).toBe(550);

    el.setTurnHeightOf('t5', 2000);
    onResize();

    expect(el.scrollTop).toBe(550);
    detach();
  });

  it('keeps holding when the reader scrolled just before the restore', () => {
    localStorage.setItem('k', 'anchor:-150:t2');
    const el = mockTranscript({ ids: IDS, turnHeight: 400, scrollTop: 0 });
    const { onScroll, onResize } = makeScrollObservers(el);
    readerGestureForTest(el);
    const detach = attachScrollMemory(el, 'k', opts);
    onScroll();

    el.setTurnHeightOf('t0', 900);
    onResize();

    expect(el.scrollTop).toBe(1450);
    detach();
  });

  it('holds the new line once the reader scrolls away from the restored line', () => {
    localStorage.setItem('k', 'anchor:-150:t2');
    const el = mockTranscript({ ids: IDS, turnHeight: 400, scrollTop: 0 });
    const { onScroll, onResize } = makeScrollObservers(el);
    const detach = attachScrollMemory(el, 'k', opts);

    readerGestureForTest(el);
    el.scrollTop = 700;
    onScroll();
    el.setTurnHeightOf('t0', 900);
    onResize();

    expect(el.scrollTop).toBe(1200);

    el.setTurnHeightOf('t1', 900);
    onResize();
    expect(el.scrollTop).toBe(1200);
    detach();
  });
});

describe('a return puts back the line the reader was on, not only its turn', () => {
  const IDS = ['t0', 't1', 't2', 't3', 't4', 't5'];
  const opts = { live: () => ({}), resetOnEmpty: true, anchorsToContent: true };
  let origRO: unknown;
  let origMO: unknown;

  beforeEach(() => {
    localStorage.clear();
    clearPendingEventScroll();
    origRO = (globalThis as any).ResizeObserver;
    origMO = (globalThis as any).MutationObserver;
    class Inert { observe() {} disconnect() {} takeRecords() { return []; } }
    (globalThis as any).ResizeObserver = Inert;
    (globalThis as any).MutationObserver = Inert;
  });
  afterEach(() => {
    clearPendingEventScroll();
    (globalThis as any).ResizeObserver = origRO;
    (globalThis as any).MutationObserver = origMO;
  });

  function leaveAt(blocks: number[], scrollTop: number) {
    const el = mockTranscript({ ids: IDS, turnHeight: 400, blocks: { t2: blocks } });
    const detach = attachScrollMemory(el, 'k', opts);
    el.scrollTop = scrollTop;
    el.fireScroll();
    detach();
  }

  it('records the block the line runs through, inside its named turn', () => {
    leaveAt([20, 100, 1000, 200], 1320);
    expect(localStorage.getItem('k')).toMatch(/^line:-400:-520:t2:2~[0-9a-f]{8}$/);
  });

  it('records the turn over a head-clamped turn, whose block indices are not its own', () => {
    const el = mockTranscript({ ids: IDS, turnHeight: 400, blocks: { t2: [20, 100, 1000, 200] } });
    el.setHeadClamped('t2', true);
    const detach = attachScrollMemory(el, 'k', opts);
    el.scrollTop = 1320;
    el.fireScroll();
    detach();
    expect(localStorage.getItem('k')).toBe('anchor:-520:t2');
  });

  it('places the turn at once when the path does not resolve inside the rendered turn', () => {
    leaveAt([20, 100, 1000, 200], 1320);
    const el = mockTranscript({ ids: IDS, turnHeight: 400, blocks: { t2: [2000] } });
    const detach = attachScrollMemory(el, 'k', opts);
    expect(el.scrollTop).toBe(1320);
    detach();
  });

  it('holds the line when a block above it, inside the turn, grows after the return', () => {
    leaveAt([20, 100, 1000, 200], 1320);
    const el = mockTranscript({ ids: IDS, turnHeight: 400, blocks: { t2: [20, 100, 1000, 200] } });
    const { onResize } = makeScrollObservers(el);
    const detach = attachScrollMemory(el, 'k', opts);

    el.setBlockHeight('t2', 0, 300);
    onResize();

    expect(Math.abs(el.blockRelTop('t2', 2) - -400)).toBeLessThanOrEqual(2);
    detach();
  });

  it('holds the turn once a re-set reply replaced the block on the line', () => {
    leaveAt([20, 100, 1000, 200], 1320);
    const el = mockTranscript({ ids: IDS, turnHeight: 400, blocks: { t2: [20, 100, 1000, 200] } });
    const { onResize } = makeScrollObservers(el);
    const detach = attachScrollMemory(el, 'k', opts);

    el.replaceBlocks('t2');
    el.setTurnHeightOf('t1', 600);
    onResize();

    expect(Math.abs(el.blockRelTop('t2', 2) - -400)).toBeLessThanOrEqual(2);
    detach();
  });

  it('opens on the line when the turn re-wrapped taller since the reader left', () => {
    leaveAt([100, 100, 1000, 200], 1400);
    const el = mockTranscript({ ids: IDS, turnHeight: 400, blocks: { t2: [200, 200, 2000, 400] } });
    const detach = attachScrollMemory(el, 'k', opts);

    expect(Math.abs(el.blockRelTop('t2', 2) - -400)).toBeLessThanOrEqual(2);
    detach();
  });

  it('moves nobody when a block below the line grows during the hold', () => {
    leaveAt([100, 100, 1000, 200], 1400);
    const el = mockTranscript({ ids: IDS, turnHeight: 400, blocks: { t2: [100, 100, 1000, 200] } });
    const { onResize } = makeScrollObservers(el);
    const detach = attachScrollMemory(el, 'k', opts);
    const landed = el.scrollTop;

    el.setBlockHeight('t2', 3, 900);
    el.setTurnHeightOf('t5', 2000);
    onResize();

    expect(el.scrollTop).toBe(landed);
    detach();
  });

  it('lets go of the line once the reader scrolls', () => {
    leaveAt([100, 100, 1000, 200], 1400);
    const el = mockTranscript({ ids: IDS, turnHeight: 400, blocks: { t2: [100, 100, 1000, 200] } });
    const { onScroll, onResize } = makeScrollObservers(el);
    const detach = attachScrollMemory(el, 'k', opts);

    readerGestureForTest(el);
    el.scrollTop = 700;
    onScroll();
    el.setBlockHeight('t2', 0, 300);
    onResize();

    expect(el.scrollTop).toBe(700);
    detach();
  });
});
