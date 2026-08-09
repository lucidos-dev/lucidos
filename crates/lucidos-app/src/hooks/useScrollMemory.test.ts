import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  parseSavedScroll,
  isFullyRestorable,
  resetContentScroll,
  contentScrollKey,
  attachScrollMemory,
  LIVE_EDGE_VALUE,
} from './useScrollMemory';
import {
  isFollowScroll,
  scrollToBottom,
  setActiveScrollElement,
  stopFollowingBottom,
} from '../components/chat/scrollState';
import { _resetPageVisitForTesting } from '../utils/pageVisit';
import { installFakePage } from '../utils/__tests__/fakePage';

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
    expect(parseSavedScroll('250')).toEqual({ kind: 'offset', top: 250 });
  });

  it('parses 0', () => {
    expect(parseSavedScroll('0')).toEqual({ kind: 'offset', top: 0 });
  });

  it('parses the live edge, which is a position and not an offset', () => {
    // The second form a reading position takes: the reader had a standing follow
    // armed when they left, so the thread opens at whatever its bottom is NOW
    // rather than at the pixel offset that bottom happened to be back then.
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
    // scrollTop is normally an integer, but be defensive on read
    expect(parseSavedScroll('250.7')).toEqual({ kind: 'offset', top: 250 });
  });

  it('returns null for NaN', () => {
    expect(parseSavedScroll('NaN')).toBeNull();
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
// A saved position is never retired for being old.
//
// It used to be, and three describes lived here for the stamp that made it
// answerable: `formatSavedScroll`'s `<offset>:<revision>` format,
// `savedScrollIsStale`, and `dropStaleSavedScroll`. The whole mechanism served
// the auto-scroll-to-bottom: once the thread had gained a turn, discarding the
// position made the open fall through to the end, where the reader presumably
// wanted to be. Nothing scrolls to the end on its own now, so retiring only
// converts "return the reader where they were" into "send them to the top of
// the window", which is the move the reader-owns-the-scroll rule exists to stop.
//
// One trace of it survives on purpose, covered below: a browser whose
// localStorage still holds a stamped `"1500:12"` must keep its position rather
// than lose it.
// ---------------------------------------------------------------------------
describe('a stamped position written by an older build still parses', () => {
  it('reads the offset out and ignores the retired revision suffix', () => {
    expect(parseSavedScroll('1500:12')).toEqual({ kind: 'offset', top: 1500 });
    expect(parseSavedScroll('0:3')).toEqual({ kind: 'offset', top: 0 });
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
      // `isElementVisible` needs a box and an ancestor chain to walk, so the
      // element can be registered as the active scroll target and reached by the
      // chevron. A null parent ends the walk immediately.
      parentElement: null,
      getBoundingClientRect: () => ({ width: 800, height: 800, top: 0, bottom: 800, left: 0, right: 800 }),
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

  it('writes the outgoing key with the offset it actually saw', () => {
    // The switch that used to corrupt it: parked at 1800 in one thread, then
    // tap another. The cleanup runs after that render, so the container already
    // shows the new thread by the time it fires.
    const el = makeEl(1800);
    const detach = attachScrollMemory(el, 'k', { live: () => ({}) });

    el.fireScroll();

    // ...the incoming thread's render lands before the cleanup does.
    el.scrollTop = 120;
    detach();

    expect(localStorage.getItem('k')).toBe('1800');
  });

  it('records a reader who ended up at the bottom, rather than forgetting them', () => {
    // The transcript used to pass `shouldSave: () => scrolledUp.value`, so a
    // reader at the live edge saved nothing: the auto-scroll-to-bottom on the
    // next open would put them back there anyway. Nothing does that now, and
    // declining to save would send someone who finished a thread to the TOP of
    // it on re-entry, which is the app moving them rather than returning them.
    const el = makeEl(4200); // 4200 + 800 clientHeight == the 5000 bottom
    const detach = attachScrollMemory(el, 'k', { live: () => ({}) });

    el.fireScroll();
    detach();

    expect(localStorage.getItem('k')).toBe('4200');
  });

  it('records a reader who scrolled to the very top, distinctly from no save at all', () => {
    // "0" has to persist as a real position, so the open path RESTORES the top
    // the reader chose instead of taking the `resetOnEmpty` branch. Both land
    // in the same place today, but they mean different things and only one of
    // them stands down for a live deep-link.
    const el = makeEl(0, 5000);
    const detach = attachScrollMemory(el, 'k', { live: () => ({}) });

    el.fireScroll();
    detach();

    expect(localStorage.getItem('k')).toBe('0');
    expect(parseSavedScroll(localStorage.getItem('k'))).toEqual({ kind: 'offset', top: 0 });
  });

  it('ignores a scroll that lands after this key stopped being the current one', () => {
    // Leaving a thread you were parked in used to lose your place in it, by two
    // routes into the same window: the teardown is deferred past the render that
    // changed the key, so the listener is still attached while the shared
    // transcript already belongs to the next thread. Either the incoming
    // thread's open-at-the-top reset moved it (it had no position of its own) or
    // swapping in its content clamped it, and the scroll event carried the
    // INCOMING thread's offset.
    localStorage.setItem('k', '5000');
    // Tall enough that the restore completes on attach: until it does, the save
    // listener is gated behind `restoring` and the assertion would be vacuous.
    const el = makeEl(5000, 20000);
    let current = true;
    const detach = attachScrollMemory(el, 'k', {
      live: () => ({}),
      isCurrent: () => current,
    });

    current = false;   // the render moved on to the next thread
    el.scrollTop = 0;  // its shorter content clamped the shared container
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
    // The guard is about WHOSE key the scroll belongs to, never about what
    // caused it. The down chevron is the reader moving, so where it lands them
    // becomes their new position and the old parked one must not survive it.
    localStorage.setItem('k', '5000');
    const el = makeEl(5000, 20000);
    const detach = attachScrollMemory(el, 'k', {
      live: () => ({}),
      isCurrent: () => true,
    });

    el.scrollTop = 19200; // the chevron's write
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

  // -------------------------------------------------------------------------
  // Opening position. With no auto-scroll-to-bottom left anywhere, this hook is
  // the ONLY thing that decides where a thread starts: restore a saved
  // position, else open at the top of what is rendered.
  // -------------------------------------------------------------------------

  it('opens at the top when there is no saved position', () => {
    // `.thread-content` is ONE element reused across threads, so without the
    // reset a fresh thread inherits the offset of the one before it. This is
    // what "open at the TOP of what is rendered, the way a document opens" is
    // made of.
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

  it('defers the RESTORE to a deep-link that owns the open', () => {
    // A notification tap into a thread the reader has a saved position in. The
    // restore is an observer that keeps retrying until the content is tall
    // enough, so it can fire long after the deep-link has landed and snap the
    // reader off the event they were sent to. It stands down for the whole
    // resolve window.
    localStorage.setItem('k', '3200');
    const el = makeEl(4200, 20000);
    const detach = attachScrollMemory(el, 'k', {
      live: () => ({ shouldRestore: () => false }),
      resetOnEmpty: true,
    });

    expect(el.scrollTop).toBe(4200); // untouched
    detach();
  });

  // A deep-link owning the open, modelled the way the real one behaves: the
  // claim is held while `scrollToEventAndPulse` waits for its target, and
  // released at its own deadline (EVENT_RESOLVE_DEADLINE_MS) whether or not the
  // target ever showed up.
  function withDeepLink() {
    let claimHeld = true;
    return {
      live: () => ({ shouldRestore: () => !claimHeld }),
      release: () => { claimHeld = false; },
    };
  }

  it('defers the top RESET to a deep-link too, then rescues a dead one', async () => {
    // The reset stands down for the same reason the restore does: the attach
    // cannot be assumed to precede the landing. It is parked on `paused` until
    // the events load, and `eventsLoaded` arrives in the same store write as
    // the rendered exchanges, so the deep-link's MutationObserver (a microtask
    // on that commit) resolves before Preact's deferred effect attaches. Under
    // reduced motion the landing is one synchronous write with nothing to
    // re-assert it, so an ungated reset simply overwrote it.
    //
    // Standing down alone would strand a DEAD link on the outgoing thread's
    // offset forever, so the attach waits out the deep-link's budget and then
    // positions, but only if the container has not moved at all.
    vi.useFakeTimers();
    try {
      const el = makeEl(4200, 20000); // the outgoing thread's offset
      const link = withDeepLink();
      const detach = attachScrollMemory(el, 'k', { live: link.live, resetOnEmpty: true });

      expect(el.scrollTop).toBe(4200); // untouched while the link may still land

      await vi.advanceTimersByTimeAsync(4000);
      link.release(); // its deadline passed with nothing found
      await vi.advanceTimersByTimeAsync(600);

      expect(el.scrollTop).toBe(0); // the link was dead, so the open is ours after all
      detach();
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

      expect(el.scrollTop).toBe(3200); // where the reader left off, not the top
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('leaves the rescue alone once anything has moved the container', async () => {
    // A landing moved it, or the reader scrolled. Either way there is a real
    // position here now and it is not ours to overwrite. This is the whole
    // safety of the rescue: it acts only on a container nothing touched.
    vi.useFakeTimers();
    try {
      const el = makeEl(4200, 20000);
      const link = withDeepLink();
      const detach = attachScrollMemory(el, 'k', { live: link.live, resetOnEmpty: true });

      el.scrollTop = 8800; // the deep-link lands on its event

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
      // Never released: a second notification tapped mid-wait re-claims it.
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
    // The teardown is deferred past the render that changed `key`, so the timer
    // can outlive this attachment's relevance even without a detach.
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

      current = false; // the render moved on to the next thread
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

      detach(); // the reader moves on before the deep-link's budget runs out
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

  // -------------------------------------------------------------------------
  // The second form a reading position takes. A standing follow is the reader
  // asking to ride the live edge until they say otherwise, and it used to end
  // the moment they looked at another thread: the flag is one global that
  // `focusThread` retires on every open, so coming back landed on the pixel
  // offset the transcript had when they walked away, with everything the agent
  // produced meanwhile below them and nothing following. Recording the request
  // here is what gives it the same lifetime the offset already had.
  // -------------------------------------------------------------------------
  describe('a standing follow survives leaving the thread', () => {
    /** Arm the follow the way the down chevron does. Not a test-only hatch: this
     *  is one of the three real arming points, reached through the same
     *  active-element registration ThreadView performs. */
    function armViaChevron(el: any) {
      setActiveScrollElement(el);
      scrollToBottom();
    }

    beforeEach(() => { stopFollowingBottom(); setActiveScrollElement(null); });
    afterEach(() => { stopFollowingBottom(); setActiveScrollElement(null); });

    it('records the live edge rather than the offset the follow happened to reach', () => {
      // Every growth round writes scrollTop, so recording the number would
      // overwrite the request with a pixel value on the next token and re-entry
      // would land wherever the stream had got to.
      const el = makeEl(100, 5000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), followsLiveEdge: true });

      armViaChevron(el);
      el.fireScroll();
      el.scrollHeight = 9000; // the reply keeps arriving
      el.scrollTop = 8200;
      detach();

      expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
    });

    it('records the request even when arming produced no scroll event at all', () => {
      // The ordinary idle case, not an exotic one: a reader already at the live
      // edge who presses the chevron gets a write the browser clamps to where
      // they already are, and an idle thread then grows nothing. Nothing ever
      // fires, so a save driven only by scroll events would lose the request.
      const el = makeEl(4200, 5000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), followsLiveEdge: true });

      armViaChevron(el);
      detach();

      expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
    });

    it('records the offset the reader landed on when they scroll away, follow still armed', () => {
      // The listener-order case. `.thread-content` carries two scroll listeners:
      // the disarm lives in `makeScrollObservers` and this save lives here. No
      // observers are wired in this test, so the flag is STILL armed when the
      // save runs, which is exactly the order that would break a save asking
      // "is the follow armed". Asking where the container is instead answers the
      // same in either order.
      const el = makeEl(100, 5000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), followsLiveEdge: true });

      armViaChevron(el);
      el.scrollTop = 2000; // a wheel, a drag, a flick
      el.fireScroll();
      detach();

      expect(localStorage.getItem('k')).toBe('2000');
    });

    it('keeps the live edge on the thread being LEFT when focusThread retires the follow', () => {
      // `focusThread` retires the follow so the thread being OPENED does not
      // inherit it, and that retire must cost the outgoing thread nothing. It
      // can only be free because the retirement is not broadcast: there is no
      // save path for it to reach.
      const el = makeEl(100, 5000);
      let current = true;
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        followsLiveEdge: true,
        isCurrent: () => current,
      });

      armViaChevron(el);
      el.fireScroll();

      stopFollowingBottom(); // what focusThread does on the way in
      current = false;       // the render moved on to the incoming thread
      el.scrollTop = 120;    // whose shorter content clamps the shared container
      el.fireScroll();
      detach();

      expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
    });

    it('does not record an arm made after this key stopped being the current one', () => {
      // The superseded attachment is still subscribed until its deferred
      // teardown, and a follow armed in the thread now on screen is not this
      // key's request.
      localStorage.setItem('k', '1800');
      const el = makeEl(1800, 5000);
      let current = true;
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        followsLiveEdge: true,
        isCurrent: () => current,
      });

      current = false;
      armViaChevron(el);
      detach();

      expect(localStorage.getItem('k')).toBe('1800');
    });

    it('returns the reader to TODAY\'s live edge, not the offset it was when they left', () => {
      // The whole report. The thread grew from 5000 to 20000 while they were
      // away; the old behaviour restored 4200 and stranded them 15000px above
      // the work they came back to watch.
      localStorage.setItem('k', LIVE_EDGE_VALUE);
      const el = makeEl(120, 20000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });

      expect(el.scrollTop).toBe(19200);
      expect(isFollowScroll(el)).toBe(true); // and still following, not a one-shot landing
      detach();
    });

    it('leaves an UNARMED reader at the bottom exactly where they were', () => {
      // The distinction the whole feature rests on: a position is not a request.
      // This reader is at the identical place as the one above and is recorded
      // as an offset, so re-entry returns them to it and follows nothing.
      const el = makeEl(100, 5000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), followsLiveEdge: true });
      el.scrollTop = 4200; // the live edge, reached by hand
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
      expect(isFollowScroll(grown)).toBe(false);
      detach2();
    });

    it('never records or restores the live edge for a container that cannot ride one', () => {
      // The content pane and the thread drawer share this hook, and the follow
      // is one global: without the gate, arming in the transcript would stamp
      // the live edge onto whatever they were showing.
      localStorage.setItem('k', LIVE_EDGE_VALUE);
      const el = makeEl(4200, 20000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

      expect(el.scrollTop).toBe(0); // the sentinel reads as no saved position

      armViaChevron(el);
      el.scrollTop = 3000;
      el.fireScroll();
      detach();

      expect(localStorage.getItem('k')).toBe('3000');
    });

    it('rescues a dead deep-link into a followed thread at the live edge', async () => {
      // Same rescue the offset form gets: the deep-link owns the open, and when
      // it turns out dead the thread is positioned rather than left showing the
      // outgoing thread's offset.
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

        expect(el.scrollTop).toBe(4200); // untouched while the link may still land

        await vi.advanceTimersByTimeAsync(4000);
        link.release();
        await vi.advanceTimersByTimeAsync(600);

        expect(el.scrollTop).toBe(19200);
        expect(isFollowScroll(el)).toBe(true);
        detach();
      } finally {
        vi.useRealTimers();
      }
    });
  });

  // -------------------------------------------------------------------------
  // Being BACKGROUNDED is the third way to leave a thread, after switching to
  // another one and reloading. It is the one that runs no teardown and no
  // attach: the same DOM comes back, so nothing re-reads the reading position
  // on its own and nothing commits what was pending when the app went away.
  // -------------------------------------------------------------------------
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

    /** Arrive at a thread recorded at the live edge, the way a reader does when
     *  they open one they were following. */
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
      // The report. Nothing remounts across a background, so without a wake
      // signal the reader comes back to the offset the transcript had when the
      // app was frozen, with the whole batch the resync just delivered below
      // them and nothing following.
      const el = makeEl(0, 5000);
      const detach = arriveFollowing(el);
      expect(el.scrollTop).toBe(4200);

      page.background();
      el.scrollHeight = 20000; // the turns that landed while they were away
      page.foreground();

      expect(el.scrollTop).toBe(19200);
      expect(isFollowScroll(el)).toBe(true);
      detach();
    });

    it('moves a reader who scrolled away BEFORE backgrounding zero pixels', () => {
      // The direction that does damage, and the reason the wake reads the
      // record rather than the follow flag. Note what has NOT happened here:
      // the 150ms debounce never fired, so the offset reached storage only
      // because the hide flushed it, and without that flush the stale live edge
      // would still be there for the wake to act on.
      const el = makeEl(0, 20000);
      const detach = arriveFollowing(el);
      expect(el.scrollTop).toBe(19200);

      el.scrollTop = 6000; // a wheel, a drag, a flick: the disarm
      el.fireScroll();
      expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE); // debounce still pending

      page.background();
      expect(localStorage.getItem('k')).toBe('6000'); // the hide committed it

      el.scrollHeight = 40000;
      page.foreground();

      expect(el.scrollTop).toBe(6000);
      expect(isFollowScroll(el)).toBe(false);
      detach();
    });

    it('reads what is STORED on wake, not what was saved when it attached', () => {
      // The mirror of the case above, and the other half of why `saved` cannot
      // be reused: here the attach-time record was an OFFSET and the reader
      // armed a follow afterwards. A wake keyed on the attach-time snapshot
      // would leave them behind exactly when they had asked not to be.
      localStorage.setItem('k', '400');
      const el = makeEl(400, 20000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });

      setActiveScrollElement(el);
      scrollToBottom(); // the reader arms it, and the arm records the live edge
      page.background();
      expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);

      el.scrollHeight = 60000;
      page.foreground();

      expect(el.scrollTop).toBe(59200);
      detach();
    });

    it('keeps recording after a background, because a hide is not a teardown', () => {
      // The same attachment carries on: the flush commits, it does not detach.
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
      // A background can land inside the window where a superseded attachment
      // is still subscribed, and the container already belongs to the next
      // thread by then.
      const el = makeEl(0, 20000);
      let current = true;
      const detach = arriveFollowing(el, { isCurrent: () => current });
      expect(el.scrollTop).toBe(19200);

      current = false;
      el.scrollTop = 500; // the incoming thread's content clamped the container
      page.background();
      page.foreground();

      expect(el.scrollTop).toBe(500);
      detach();
    });

    it('never re-asserts a container that cannot ride a live edge', () => {
      // The content pane and the thread drawer share this hook and share the
      // wake, so the gate has to hold on this path too.
      localStorage.setItem('k', LIVE_EDGE_VALUE);
      const el = makeEl(300, 20000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });
      expect(el.scrollTop).toBe(0); // the sentinel reads as no saved position

      page.background();
      page.foreground();

      expect(el.scrollTop).toBe(0);
      detach();
    });

    it('defers to a deep-link that owns the open', () => {
      // A push notification can resume the app and resolve a deep-link in one
      // breath. The event the reader was sent to wins over the live edge.
      localStorage.setItem('k', LIVE_EDGE_VALUE);
      const el = makeEl(4200, 20000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({ shouldRestore: () => false }),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });
      expect(el.scrollTop).toBe(4200); // stood down at attach

      page.background();
      page.foreground();

      expect(el.scrollTop).toBe(4200); // and still stood down
      detach();
    });
  });
});
