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
  clearPendingEventScroll,
  followingLiveEdge,
  hasPendingEventScroll,
  isFollowScroll,
  setFollowLiveEdge,
  scrollToEventAndPulse,
  setActiveScrollElement,
  setThreadLive,
  stopFollowingBottom,
} from '../components/chat/scrollState';
import { _resetPageVisitForTesting } from '../utils/pageVisit';
import { USER_ACTION_EVENTS } from '../utils/userAction';
import { installFakePage } from '../utils/__tests__/fakePage';

/** Pin the *follow seed* off around every test in this file.
 *
 *  The toggle records a seed that outlives the thread AND the press: it is what
 *  a thread with NO reading position starts as. It ships ARMED, and a test that
 *  arms explicitly leaves it armed for the next one. Either way a fresh
 *  attachment gets seeded, which is how the two "unarmed reader" cases below
 *  started recording `live-edge`.
 *
 *  BEFORE as well as after, so the baseline does not depend on test ordering.
 *  `stopFollowingBottom()` cannot do this job, and must not: only a press writes
 *  the seed, precisely so a scroll cannot cancel a standing preference. Pressing
 *  it off is the honest reset, and it retires the follow on the way. */
beforeEach(() => setFollowLiveEdge(false));
afterEach(() => setFollowLiveEdge(false));

/** And un-say "the agent is running". `setThreadLive` is a module-global
 *  `ChatExchange` normally owns. A test that sets it true leaks a live thread
 *  into every later one. The follow would then carry a reader the test never
 *  meant to arm. */
afterEach(() => setThreadLive(false));

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
    // The second form a reading position takes. The reader had a standing
    // follow armed when they left. So the thread opens at whatever its bottom
    // is NOW, not at the pixel offset that bottom was back then.
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
// **A saved position is never retired for being old.** Nothing scrolls to the
// end on its own (ADR 0064). Retiring one would only convert "return the
// reader where they were" into "send them to the top of the window".
//
// One trace of the retired stamp survives on purpose, covered below: a browser
// whose localStorage still holds a stamped `"1500:12"` keeps its position.
// ---------------------------------------------------------------------------
describe('a stamped position written by an older build still parses', () => {
  it('reads the offset out and ignores the retired revision suffix', () => {
    expect(parseSavedScroll('1500:12')).toEqual({ kind: 'offset', top: 1500 });
    expect(parseSavedScroll('0:3')).toEqual({ kind: 'offset', top: 0 });
  });
});

// ---------------------------------------------------------------------------
// The teardown flush must write what THIS key observed, never what the next
// render is holding. `attachScrollMemory` is the hook's whole body, extracted
// so the lifecycle can be driven with a fake element. The defect here is about
// WHEN a value is read, which no assertion over the hook could reach.
// ---------------------------------------------------------------------------
describe('attachScrollMemory teardown', () => {
  function makeEl(scrollTop: number, scrollHeight = 5000) {
    const listeners: Array<() => void> = [];
    return {
      scrollTop,
      scrollHeight,
      clientHeight: 800,
      // `isElementVisible` needs a box and an ancestor chain to walk. The
      // element can then register as the active scroll target and be reached by
      // the chevron. A null parent ends the walk immediately.
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

  /** The reader doing something, through the one definition the app shares
   *  (`utils/userAction.ts`): a real input event on `document`, which is what
   *  the restore window stands down for. Dispatched rather than faked, so the
   *  test exercises the same listener the hook installs. */
  function userAction(type: (typeof USER_ACTION_EVENTS)[number] = 'wheel') {
    document.dispatchEvent(new Event(type));
  }

  /** Swap in observers that hand the test their callbacks and drop them on
   *  disconnect. A retired restore is then observable, rather than merely not
   *  firing. Used by the restore-window tests below and by the deep-link
   *  describe further down, which is why it sits out here. */
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
    // Nothing scrolls to the bottom on its own (ADR 0064). Declining to save at
    // the live edge would send someone who finished a thread to the TOP of it
    // on re-entry. That is the app moving them, not returning them.
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
    // Two routes reach the same window. The teardown is deferred past the
    // render that changed the key. The listener is therefore still attached
    // while the shared transcript belongs to the next thread. Either the
    // incoming thread's open-at-the-top reset moved it, or swapping in its
    // content clamped it. Either way the scroll carries the INCOMING offset.
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

  // -------------------------------------------------------------------------
  // The restore WINDOW. A saved offset the container is not yet tall enough to
  // hold is retried until the content grows, and then given up on. Both ends of
  // that window belong to the reader: nothing here may put them somewhere they
  // did not ask to be, seconds after they arrived and settled.
  // -------------------------------------------------------------------------

  it('never lands the reader on the live edge when the saved offset is out of reach', async () => {
    // Clamping to `Math.min(saved.top, max)` at the deadline is the live edge
    // three seconds late, since the deadline only runs when the offset is
    // UNREACHABLE. The transcript renders a TAIL sized by
    // `threadWindow.seedRenderCount`. A position recorded against a taller
    // render is therefore out of reach on the next open.
    vi.useFakeTimers();
    try {
      localStorage.setItem('k', '12000'); // recorded when far more was rendered
      const el = makeEl(0, 5000);         // today's tail: its bottom is 4200
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

      await vi.advanceTimersByTimeAsync(3000);

      expect(el.scrollTop).toBe(0);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  it('parks the reader at the top while it waits, not on the outgoing offset', () => {
    // `.thread-content` is one element. A thread whose saved offset is not yet
    // reachable would otherwise spend the wait on the PREVIOUS thread's
    // position. A wait that never pays out leaves them there for good. Arriving
    // in a shorter thread clamps that borrowed number to this thread's live
    // edge, and the save listener then persists it as this thread's own.
    localStorage.setItem('k', '12000');
    const el = makeEl(4200, 5000); // the outgoing thread's offset, clamped in
    captureObservers();
    const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

    expect(el.scrollTop).toBe(0);
    detach();
  });

  it('does not park a reader whose offset is reachable right now', () => {
    // The common revisit: the transcript is already tall enough, so the retry
    // is about to land the position. Writing the top first would only be a
    // frame of flash on the way there.
    localStorage.setItem('k', '3200');
    const el = makeEl(4200, 20000);
    const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

    expect(el.scrollTop).toBe(3200);
    detach();
  });

  // The deadline's LAST LOOK. Isolating it needs nothing but inert observers.
  // The attach's own attempt is synchronous, so any growth arranged after it is
  // growth only the deadline can answer for, which is the decoded-image case.

  it('takes one last look at the deadline, for growth neither observer saw', async () => {
    // What the deadline is FOR. An image decoding grows `scrollHeight` without
    // mutating the DOM, and the container's own box is unchanged, being a flex
    // child of a fixed parent. Neither observer fires, so a now-reachable
    // offset would otherwise be dropped.
    vi.useFakeTimers();
    try {
      localStorage.setItem('k', '3200');
      const el = makeEl(500, 1000); // arrived carrying the outgoing thread's 500
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

      el.scrollHeight = 20000; // the image lands; nothing announces it
      await vi.advanceTimersByTimeAsync(3000);

      expect(el.scrollTop).toBe(3200);
      detach();
    } finally {
      vi.useRealTimers();
    }
  });

  // ── Whose window is it ────────────────────────────────────────────────────
  //
  // The wait runs for three seconds, long enough for the reader to have settled
  // in, so the first thing they DO retires it. Asked as a gesture and never as
  // a change in `scrollTop`. The app writes `scrollTop` all through this
  // window, and reading one of those as the reader abandons their position.

  it('retires the whole wait on the reader\'s first gesture', () => {
    localStorage.setItem('k', '3200');
    const el = makeEl(0, 3000);
    const observers = captureObservers();
    const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

    userAction('wheel');     // they start reading before the content is ready
    el.scrollTop = 900;
    el.scrollHeight = 20000; // and then it grows tall enough to hold 3200
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
    // Every mechanism that moves this container without the reader. The iOS
    // compositor nudge (`utils/webkitRepaint.ts`). A clamp when shorter content
    // swaps into the shared element. `restoreAfterReflow`'s correction across a
    // pane resize. The render window's compensation for prepended height. None
    // of them emits an input event, which is why the question is asked so.
    localStorage.setItem('k', '3200');
    const el = makeEl(0, 3000);
    const observers = captureObservers();
    const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

    el.scrollTop = 2200;     // whichever of them it was
    el.scrollHeight = 20000; // and then the growth the restore was waiting for
    observers.fire();

    expect(el.scrollTop).toBe(3200);
    detach();
  });

  it('defers the RESTORE to a deep-link that owns the open', () => {
    // A notification tap into a thread the reader has a saved position in. The
    // restore is an observer retrying until the content is tall enough. It can
    // fire long after the deep-link landed, snapping the reader off the event.
    // So it stands down for the whole resolve window.
    localStorage.setItem('k', '3200');
    const el = makeEl(4200, 20000);
    const detach = attachScrollMemory(el, 'k', {
      live: () => ({ shouldRestore: () => false }),
      resetOnEmpty: true,
    });

    expect(el.scrollTop).toBe(4200); // untouched
    detach();
  });

  // ── A restore still ARMED when a deep-link claims the open ─────────────────
  //
  // The stand-down at attach reads the claim once, and answers only for the
  // ordering where the claim is already in place. Two ordinary orderings are
  // not. A deep-link into the thread the reader is ALREADY in re-attaches
  // nothing. A thread whose events arrive while the tap resolves attaches
  // BEFORE the claim. In both the restore is mid-flight, waiting for the
  // transcript to grow, and the claim's own render-all is that growth.
  // Re-asking is not enough either: the claim releases within a second of a
  // synchronous landing, while the restore stays armed for three.
  //
  // These drive the REAL deep-link, not a stand-in predicate, so the wiring
  // between the two modules is what is pinned. That needs the observers to be
  // reachable rather than inert, which is what `captureObservers` (above) is
  // for. Both the restore's observers and the deep-link's own land in it, hence
  // the empty record list every callback is given.
  describe('a deep-link claiming the open mid-restore', () => {
    let origCSS: unknown;
    beforeEach(() => {
      // scrollToEventAndPulse builds an attribute selector; the env has no CSS.
      origCSS = (globalThis as any).CSS;
      (globalThis as any).CSS = { escape: (s: string) => s };
    });
    afterEach(() => {
      (globalThis as any).CSS = origCSS;
      clearPendingEventScroll();
    });

    /** The transcript's own gate, verbatim from ThreadView. */
    const transcript = () => ({ shouldRestore: () => !hasPendingEventScroll() });

    it('retires a restore armed before the claim, so the landing stands', () => {
      localStorage.setItem('k', '3200');
      const el = makeEl(0, 1000); // too short to hold 3200, so the restore waits
      const observers = captureObservers();
      const detach = attachScrollMemory(el, 'k', {
        live: transcript,
        resetOnEmpty: true,
      });
      expect(el.scrollTop).toBe(0); // nothing tall enough to restore onto yet
      const armedBefore = observers.armed();
      expect(armedBefore).toBeGreaterThan(0);

      scrollToEventAndPulse('e1'); // the tap claims the open
      expect(observers.armed()).toBeLessThan(armedBefore); // retired at the claim

      el.scrollHeight = 20000; // render-all grows the transcript
      el.scrollTop = 8800;     // and the landing puts the reader on the event
      observers.fire();

      expect(el.scrollTop).toBe(8800);
      detach();
    });

    it('retires the restore DEADLINE with it', async () => {
      // The other half of an armed restore: at RESTORE_DEADLINE_MS it takes one
      // last look, and the claim's own render-all is the growth that makes the
      // saved offset reachable. Here the link is still resolving and has moved
      // nobody, so the container sits where the open found it. An un-retired
      // deadline would write 3200 over a landing that has not happened.
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
      // Standing down is two obligations, not one. Retiring the restore alone
      // would leave a dead link with nothing positioning the thread, and
      // `.thread-content` is one element reused across threads: it keeps
      // showing the OUTGOING thread's offset, which the save listener then
      // persists as this thread's remembered position. A claim arriving mid
      // restore has to arm the same rescue an attach-time claim does.
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', '3200');
        const el = makeEl(4200, 1000); // the outgoing thread's offset
        captureObservers();
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
        });

        scrollToEventAndPulse('e1');
        el.scrollHeight = 20000; // render-all grows it, but nothing lands

        await vi.advanceTimersByTimeAsync(4000); // the deep-link's own deadline
        clearPendingEventScroll();               // it gave up and released
        await vi.advanceTimersByTimeAsync(600);

        expect(el.scrollTop).toBe(3200); // where the reader left off
        detach();
      } finally {
        vi.useRealTimers();
      }
    });

    /** Claim the open BEFORE the attach, with a target that never renders.
     *
     *  The other ordering cannot reach a thread with NO record. A claim
     *  broadcast only stands down over an armed restore, and a no-record open
     *  arms none: it decides on the spot and stops restoring. So the attach-time
     *  branch is the only route the seed's arm and the rescue ever share. */
    function deadLinkOverNoRecord(el: any) {
      captureObservers();
      stopFollowingBottom();
      setThreadLive(false);
      scrollToEventAndPulse('never-renders');
      return attachScrollMemory(el, 'k', {
        live: transcript,
        resetOnEmpty: true,
        // The transcript, which is the one container that records a live edge
        // and therefore the only one the seed can arm.
        followsLiveEdge: true,
      });
    }

    it('does not reset an armed reader to the top when the link turns out dead', async () => {
      // The rescue answers for the open it is rescuing, so it re-reads. The
      // stand-down can ARM this open through the *follow seed*, on a thread whose
      // record was empty when the attachment read it. Held against that snapshot
      // the rescue took its reset branch. It hauled an armed reader to the top,
      // recording the offset over the request the arm had just made.
      vi.useFakeTimers();
      try {
        setFollowLiveEdge(true); // the press that records the seed
        const el = makeEl(4200, 20000); // the outgoing thread's offset
        const detach = deadLinkOverNoRecord(el);

        expect(followingLiveEdge.value).toBe(true); // the seed armed, in place
        await vi.advanceTimersByTimeAsync(4000); // the deep-link's own deadline
        clearPendingEventScroll();               // it gave up and released
        await vi.advanceTimersByTimeAsync(600);

        expect(el.scrollTop).toBe(19200); // today's live edge, not the top
        expect(followingLiveEdge.value).toBe(true);
        detach();
      } finally {
        vi.useRealTimers();
      }
    });

    it('still opens an UNARMED reader at the top when the link turns out dead', async () => {
      // The other half of the re-read, and the reason it is not a blanket
      // resume. With the seed off there is no request and no record, so the top
      // is where this thread opens. `.thread-content` is one shared element, so
      // leaving it is leaving the reader on the outgoing thread's offset.
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
      // The first link's rescue expires while the second claim is still held,
      // so it declines and leaves nothing behind: a dead second link would
      // strand the reader on the borrowed offset with no recovery. A newer
      // claim is a newer request, so it re-arms on its own budget.
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
        scrollToEventAndPulse('e2'); // a second notification, tapped mid-window
        await vi.advanceTimersByTimeAsync(4600); // its deadline, then the slack

        expect(el.scrollTop).toBe(3200);
        detach();
      } finally {
        vi.useRealTimers();
      }
    });

    it('does not rescue a second link away from the first link\'s landing', async () => {
      // The rescue's reference point is captured from the FIRST claim and never
      // re-read. Re-reading it would make a successful landing the new baseline
      // for "nothing has moved here". A dead SECOND link would then haul the
      // reader off the event the first one took them to.
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', '3200');
        // Out of reach of what has rendered, so the open parks and WAITS. That
        // is what leaves a restore armed for the claim to stand down, which is
        // what arms the rescue at all: an offset the open can honour on the
        // spot leaves nothing for either link to take over.
        const el = makeEl(4200, 1000);
        captureObservers();
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
        });

        scrollToEventAndPulse('e1');
        el.scrollHeight = 20000; // the claim's render-all
        el.scrollTop = 8800;     // and the first link lands on its event
        await vi.advanceTimersByTimeAsync(1000);
        scrollToEventAndPulse('e2'); // a second, which turns out dead
        await vi.advanceTimersByTimeAsync(4600);

        expect(el.scrollTop).toBe(8800);
        detach();
      } finally {
        vi.useRealTimers();
      }
    });

    /** A findable deep-link target. Its rect top DEFAULTS to the container's, so
     *  `smoothScrollToElement` computes a target equal to the current scrollTop
     *  and every tween frame writes it back unchanged: a link that RESOLVES and
     *  moves nobody, which is what a thread clamped to its bottom does to a link
     *  aimed at its last turn.
     *
     *  `opts.rectTop` puts it somewhere else, for the cases that need a landing
     *  that MOVES: the container ends at `el.scrollTop + rectTop`. That target
     *  CHASES the container, so pair it with `opts.reducedMotion`, which makes
     *  the landing a synchronous write rather than a tween. Doing so also lets
     *  a test see the value at the instant the landing is announced.
     *
     *  `opts.absTop` is the other way round: it pins the target at one offset
     *  in the transcript, by reporting a rect the container's own scrolling
     *  moves. A tween therefore has a STABLE destination and settles on it,
     *  which is what a test of an asynchronous landing needs. Returns the
     *  teardown. */
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
      const origMM = (globalThis as any).matchMedia;
      (globalThis.document as any).querySelectorAll = (sel: string) =>
        (sel.startsWith('[data-event-id') ? [target] : []);
      (globalThis as any).getComputedStyle = () => ({ scrollMarginTop: '0px' });
      if (opts.reducedMotion) (globalThis as any).matchMedia = () => ({ matches: true });
      setActiveScrollElement(el);
      return () => {
        setActiveScrollElement(null);
        (globalThis.document as any).querySelectorAll = origQSA;
        (globalThis as any).getComputedStyle = origCS;
        (globalThis as any).matchMedia = origMM;
      };
    }

    it('records where the landing ARRIVED, not where it set off from', async () => {
      // The announcement is made after the scroll it describes, and under
      // reduced motion that scroll IS the whole landing: one synchronous write.
      // Announced first, the recorder would be handed the offset the reader was
      // leaving. With no tween frames behind it, nothing would correct that.
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
          // The open restored 200 on the spot (the transcript is already tall
          // enough), so that is where the landing sets off FROM.
          expect(el.scrollTop).toBe(200);
          scrollToEventAndPulse('e1');
          expect(el.scrollTop).toBe(1200); // 200 + the target's 1000 offset
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
      // The ordinary tap into a thread the reader is NOT in, the one ordering a
      // broadcast cannot serve. The target resolves on the microtask checkpoint
      // of the commit that rendered it, while Preact defers the subscribing
      // effect past that checkpoint. So the attach has to ASK what it missed.
      // Arriving in a shorter thread clamps the shared container to its bottom,
      // 500 here, and the link to its last turn resolves exactly there. The
      // has-anything-moved test then says dead about a live link.
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', '200');
        const el = makeEl(500, 1300); // clamped on arrival: max scroll IS 500
        const restoreDom = withFindableTarget(el);
        captureObservers();

        scrollToEventAndPulse('e1'); // resolves before anything attaches
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
        });

        try {
          await vi.advanceTimersByTimeAsync(4600);
          expect(el.scrollTop).toBe(500); // still on the event, not back at 200
        } finally {
          detach();
          restoreDom();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('does not rescue a link that RESOLVED without moving the container', async () => {
      // The rescue's has-anything-moved test reads a landing with nowhere to
      // move as a dead link. A link resolving to where the reader already sits
      // is ordinary. Arriving in a shorter thread clamps the shared container
      // to its bottom, and a link to that thread's last turn is right there.
      // Here the open restored the saved position and the target is in it. So
      // the inference says dead about a live link, and the resolve is told.
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', '3200');
        // Out of reach at attach, so the open parks at the top and waits. That
        // leaves a restore for the claim to stand down, and therefore a rescue
        // to arm. The growth below is the claim's own render-all, which makes
        // the rescue's write OBSERVABLE: without the resolve cancelling it,
        // 3200 would be reachable by the time it fires.
        const el = makeEl(4200, 1000);
        const restoreDom = withFindableTarget(el);
        captureObservers();
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
        });

        try {
          expect(el.scrollTop).toBe(0); // parked for the wait
          scrollToEventAndPulse('e1');  // resolves at once, and moves nobody
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

    // ── Going to a link SETS the memory ────────────────────────────────────
    //
    // The landing is a reading position. The reader asked to be at that event.
    // Coming back must return them there rather than to whatever they parked on
    // before following the link. The scroll listener cannot be left to notice
    // it: neither of these two landings writes a scroll event.

    it('records a landing that moved nobody as this thread\'s position', async () => {
      // Nothing to move means no scroll event, so without this the thread keeps
      // the stale position and the next open undoes the navigation. Here the
      // saved offset is out of reach of what has rendered. The open parks the
      // reader at the top and waits, and the link finds its target there.
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
          expect(el.scrollTop).toBe(0); // parked for the wait
          scrollToEventAndPulse('e1');
          await vi.advanceTimersByTimeAsync(200); // the debounced save lands
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
      // A position is an offset OR the live edge, and the recorder must answer
      // the same way the scroll listener would. A link naming a place other than
      // the live edge ends the ride: the reader asked to be at ONE place, and
      // the transcript must stop moving under them there. So coming back returns
      // them to the event they went to, not to a live edge they stopped riding.
      //
      // A same-thread deep-link is the way into this case, `focusThread`
      // retiring the follow only for a DIFFERENT thread. The recorded ride puts
      // the reader on the bottom as the thread opens, and the link then takes
      // them 15000px back up it.
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', LIVE_EDGE_VALUE);
        const el = makeEl(19200, 20000); // opening on its own bottom
        const restoreDom = withFindableTarget(el, { rectTop: -15000, reducedMotion: true });
        captureObservers();
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
          followsLiveEdge: true,
        });
        setFollowLiveEdge(true); // the reader armed the follow here
        setThreadLive(true);     // and the agent is working
        expect(isFollowScroll(el)).toBe(true);

        try {
          scrollToEventAndPulse('e1');
          await vi.advanceTimersByTimeAsync(200);
          // Both halves, because the record follows from the retirement rather
          // than standing on its own: asserting the offset alone would pass just
          // as well against a recorder that had stopped asking which form a
          // position takes.
          expect(isFollowScroll(el)).toBe(false);
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
      // The reader's own case, from the recording side. A link to the bottom of
      // the thread asks for the place the ride already holds. The two agree, so
      // there is nothing to end.
      //
      // The record has to follow, or the ride is lost on the way back in: an
      // offset written here opens the thread parked, with the toggle dark. Both
      // halves again, the record following from the stamp.
      //
      // The thread is LIVE here and IDLE in the test below. The pair pins that
      // the answer does not depend on which.
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', LIVE_EDGE_VALUE);
        const el = makeEl(4200, 5000); // 4200 + 800 clientHeight IS the bottom
        const restoreDom = withFindableTarget(el);
        captureObservers();
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
          followsLiveEdge: true,
        });
        setFollowLiveEdge(true); // the reader armed the follow here
        setThreadLive(true);     // and the agent is working

        try {
          scrollToEventAndPulse('e1');
          await vi.advanceTimersByTimeAsync(200);
          expect(followingLiveEdge.value).toBe(true);
          expect(isFollowScroll(el)).toBe(true);
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
      // The mirror of the test above, and it agrees with it. Where the landing
      // rests is what decides, so the agent decides nothing here.
      //
      // A quiet thread is routinely one about to run, a question card being
      // quiescent by `isRenderedThreadIdle`. This reader is parked on such a
      // card, and answering it is what wakes the thread. Riding on from there is
      // the whole of what they asked for.
      //
      // The recorder still asks the POSITION rather than whether a link landed.
      // The reason is listener ORDER inside `.thread-content`'s two scroll
      // handlers, not deep links (see `currentPosition`).
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', LIVE_EDGE_VALUE);
        const el = makeEl(4200, 5000); // already AT the bottom, as above
        const restoreDom = withFindableTarget(el);
        captureObservers();
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
          followsLiveEdge: true,
        });
        setFollowLiveEdge(true); // armed, and `setThreadLive` deliberately unset

        try {
          scrollToEventAndPulse('e1');
          await vi.advanceTimersByTimeAsync(200);
          expect(followingLiveEdge.value).toBe(true);
          expect(isFollowScroll(el)).toBe(true);
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
      // The resolve-first ordering, and it is the ordinary one for a cached
      // thread: the target renders on the commit's microtask checkpoint, while
      // Preact defers this attach past it. So the stand-down ASKS what it
      // missed, and the question is not "did it land" but "did it land
      // somewhere the ride disagrees with".
      //
      // Asking the first would make the toggle's state after a notification tap
      // depend on whether the thread's events happened to be cached.
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', LIVE_EDGE_VALUE);
        const el = makeEl(4200, 5000); // arriving clamped to its own bottom
        const restoreDom = withFindableTarget(el);
        captureObservers();

        scrollToEventAndPulse('e1'); // resolves before anything attaches
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
          followsLiveEdge: true,
        });

        try {
          expect(followingLiveEdge.value).toBe(true);
          expect(el.scrollTop).toBe(4200); // and the link still owns the place
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
      // The ordinary cross-thread tap. `focusThread` retired the ride before
      // the link resolved, so the landing ran as a plain element tween, and
      // `.thread-content` arrives holding the OUTGOING thread's offset.
      //
      // Arming beside that tween is not enough. Its frames mark plain
      // navigation, so every one records an offset. The ride the reader never
      // ended is then lost on the way back in. The resume takes the motion
      // over instead.
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', LIVE_EDGE_VALUE);
        const el = makeEl(2000, 20000); // arriving on the outgoing thread's offset
        const restoreDom = withFindableTarget(el, { absTop: 19200 }); // the newest turn
        captureObservers();

        scrollToEventAndPulse('e1'); // resolves before anything attaches
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
          followsLiveEdge: true,
        });

        try {
          expect(followingLiveEdge.value).toBe(true);
          await vi.advanceTimersByTimeAsync(1500); // the glide settles
          expect(el.scrollTop).toBe(19200);
          el.fireScroll();                         // its trailing scroll event
          await vi.advanceTimersByTimeAsync(200);
          expect(isFollowScroll(el)).toBe(true);
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
      // The ordinary cross-thread tap, and under reduced motion the whole
      // landing is one synchronous write inside it. The attachment missed the
      // announcement, so it asks at setup instead.
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', '200');
        const el = makeEl(500, 1300);
        const restoreDom = withFindableTarget(el);
        captureObservers();

        scrollToEventAndPulse('e1'); // resolves before anything attaches
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
      // A superseded attachment is still subscribed until its deferred teardown,
      // and the landing it hears belongs to the thread now on screen. Writing it
      // to the outgoing key is the corruption `observed` exists to prevent.
      vi.useFakeTimers();
      try {
        localStorage.setItem('outgoing', '3200');
        const el = makeEl(4200, 20000);
        const restoreDom = withFindableTarget(el);
        captureObservers();
        const detach = attachScrollMemory(el, 'outgoing', {
          live: transcript,
          resetOnEmpty: true,
          isCurrent: () => false, // the render already moved to the next thread
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
      // The content pane and the thread drawer share this hook. They pass no
      // `shouldRestore`. A transcript deep-link must therefore not retire their
      // restores: they answer "ours" and wait for their own content.
      localStorage.setItem('k', '3200');
      const el = makeEl(0, 1000);
      const observers = captureObservers();
      const detach = attachScrollMemory(el, 'k', { live: () => ({}) });

      scrollToEventAndPulse('e1');

      el.scrollHeight = 20000;
      observers.fire();
      expect(el.scrollTop).toBe(3200); // restored, as it always would have been
      detach();
    });

    it('records no landing for a container the claim is not about', async () => {
      // The same scoping, on the writing side: a transcript deep-link must not
      // stamp the content pane's or the thread drawer's own offset over what
      // they had saved. The container sits somewhere other than its record, so
      // a mis-scoped record is visible as the wrong value rather than a no-op.
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

    // ── The link owns the POSITION, not the REQUEST ────────────────────────
    //
    // A *standing follow* is one global flag `focusThread` retires on every
    // open, and the resume answering it lives in the positioning branch this
    // stand-down replaces. A deep-linked open is therefore the one open with a
    // retire and no resume. It is resumed here instead, writing nothing,
    // because both answers hold: the link decides where the reader looks, the
    // record decides whether they are riding.

    /** Arrive at a thread the way a NOTIFICATION TAP does. `focusThread` retires
     *  the global follow on the way in. The target renders and the link resolves
     *  on the microtask checkpoint of that commit, and only then does Preact run
     *  the effect that attaches this. `rectTop` is what makes the landing MOVE
     *  the reader, which is the ordinary case. */
    function tapNotificationInto(el: any, opts: { live?: boolean } = {}) {
      const restoreDom = withFindableTarget(el, { rectTop: 1000, reducedMotion: true });
      captureObservers();
      stopFollowingBottom();          // what focusThread does on the way in
      setThreadLive(!!opts.live);     // what ChatExchange publishes for the thread arrived at
      scrollToEventAndPulse('e1');    // resolves before anything attaches
      const detach = attachScrollMemory(el, 'k', {
        live: transcript,
        resetOnEmpty: true,
        followsLiveEdge: true,
      });
      return () => { detach(); restoreDom(); };
    }

    it('ends the ride on an IDLE thread too, because the LANDING answered it', async () => {
      // The case a liveness gate gets wrong, and the COMMON one rather than a
      // corner. A "needs your answer" notification points at a thread parked on
      // a question card by construction, and that thread is quiescent:
      // `isRenderedThreadIdle` counts `waiting_for_user_answer` as idle. A gated
      // landing would decline to retire and this resume would re-arm over it.
      // The reader answers, the thread wakes, and `honourWake` writes them to
      // the live edge, off the event the notification existed to show them.
      //
      // An idle thread is not a finished one. The ride ends here.
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', LIVE_EDGE_VALUE);
        const el = makeEl(4200, 20000);
        const done = tapNotificationInto(el);

        try {
          expect(followingLiveEdge.value).toBe(false);
          expect(el.scrollTop).toBe(5200); // the landing, and nothing written over it
          // And the landing is recorded as an OFFSET: the reader asked to be at
          // one place, so coming back returns them to it.
          await vi.advanceTimersByTimeAsync(200);
          expect(isFollowScroll(el)).toBe(false);
          expect(localStorage.getItem('k')).toBe('5200');
        } finally {
          done();
        }
      } finally {
        vi.useRealTimers();
      }
    });

    it('ends it on a LIVE thread as well, which is the unchanged half', async () => {
      // A link into a streaming thread is a request to be at ONE place, and the
      // next token would carry the reader off it. Same answer as the idle case
      // above, which is the point: the landing decides, not the agent.
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', LIVE_EDGE_VALUE);
        const el = makeEl(4200, 20000);
        const done = tapNotificationInto(el, { live: true });

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

    it('does not let the follow SEED arm over a landing either', async () => {
      // The same bug by its other route. The seed answers the no-record case,
      // and a thread reached by a link has none on a first open. An ungated
      // seed would re-arm exactly where the recorded request would have. A link
      // that LANDED is the reader naming one place, which outranks a standing
      // preference about where threads start.
      vi.useFakeTimers();
      try {
        setFollowLiveEdge(true); // the press that records the seed
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
      // The half that survives, and the reason the guard asks about the LANDING
      // rather than deleting the in-place resume. `focusThread` retires the
      // follow on the way in, so a deep-linked open is the one open with a
      // retire and no resume. While the link resolves nobody has positioned the
      // reader, so the ride is held open. A DEAD link costs them nothing, and a
      // landing takes the ride away.
      vi.useFakeTimers();
      try {
        localStorage.setItem('k', LIVE_EDGE_VALUE);
        const el = makeEl(4200, 20000);
        // Claim a link whose target never renders: no `withFindableTarget`, so
        // nothing resolves and `deepLinkHasResolved()` stays false.
        captureObservers();
        stopFollowingBottom();
        setThreadLive(false);
        scrollToEventAndPulse('never-renders');
        const detach = attachScrollMemory(el, 'k', {
          live: transcript,
          resetOnEmpty: true,
          followsLiveEdge: true,
        });

        try {
          expect(followingLiveEdge.value).toBe(true);
          expect(el.scrollTop).toBe(4200); // and nothing written over them
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
      // The content pane and the thread drawer share this hook, and the follow
      // is one global. The gate must hold on this path too: a deep link in the
      // transcript must not arm a follow because a file preview was scrolled.
      vi.useFakeTimers();
      try {
        setFollowLiveEdge(true); // the seed is on, and must not reach this container
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
    // the events load. `eventsLoaded` arrives in the same store write as the
    // rendered exchanges, so the deep-link's MutationObserver resolves before
    // Preact's deferred effect attaches. Under reduced motion the landing is
    // one synchronous write, so an ungated reset would overwrite it.
    //
    // Standing down alone would strand a DEAD link on the outgoing thread's
    // offset. So the attach waits out the deep-link's budget and then
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

  it('rescues to the TOP, never the live edge, when the saved offset is out of reach', async () => {
    // The rescue owes the reader a position. A dead link leaves nobody else to
    // give them one, and the shared container still shows the outgoing thread's
    // offset. What it owes them is not the bottom. Clamping an unreachable
    // offset to `max` is scrolling to the live edge, late (ADR 0064). A position
    // that cannot be honoured opens the thread where a thread with no position
    // at all opens, at the top of what is rendered.
    vi.useFakeTimers();
    try {
      localStorage.setItem('k', '12000');
      const el = makeEl(4200, 5000); // the outgoing thread's offset, in a short tail
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
  // asking to ride the live edge until they say otherwise. The flag itself is
  // one global `focusThread` retires on every open. Without a record, the ride
  // would end the moment they looked at another thread. Recording the request
  // here gives it the same lifetime the offset already had.
  // -------------------------------------------------------------------------
  describe('a standing follow survives leaving the thread', () => {
    /** Arm the follow the way the FOLLOW TOGGLE does. Not a test-only hatch:
     *  it is the only arming point there is (the resume this file exercises
     *  aside), reached through the same active-element registration ThreadView
     *  performs. The toggle glides rather than jumping, so its arrival has to be
     *  waited out where the position matters. */
    function armViaToggle(el: any) {
      setActiveScrollElement(el);
      // Park on the live edge FIRST. The toggle glides there when the reader is
      // not on it. This environment's `requestAnimationFrame` stub hands every
      // frame the same timestamp, so a tween never reaches t=1. These tests are
      // about what the arm RECORDS, not how it travels. The travelling is
      // `scroll-follow-the-live-edge.test.ts`.
      el.scrollTop = Math.max(0, el.scrollHeight - el.clientHeight);
      setFollowLiveEdge(true);
    }

    beforeEach(() => { stopFollowingBottom(); setActiveScrollElement(null); });
    afterEach(() => { stopFollowingBottom(); setActiveScrollElement(null); });

    /* ── The follow SEED ─────────────────────────────────────────────────────
     *  What a thread with NO record starts as. The per-thread record stays
     *  authoritative wherever it exists, being the reader's own last act on that
     *  thread. The seed answers the one case the record cannot. */

    it('arms a thread with NO reading position when the seed is on', () => {
      // The brand-new thread, and the whole point of the toggle being reachable
      // from the compose view. Arming has to WRITE the live edge as well, since
      // `.thread-content` is one element reused across threads and arrives
      // holding the outgoing thread's offset.
      setFollowLiveEdge(true);   // the reader's last press, on some other thread
      stopFollowingBottom();     // what focusThread does on the way into this one

      const el = makeEl(300, 5000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });

      expect(isFollowScroll(el)).toBe(true);
      expect(el.scrollTop).toBe(5000 - 800); // this fake's live edge
      detach();
    });

    it('records the seeded arm, so the thread owns the answer from then on', () => {
      // The seed decides a thread's FIRST open and no more. From then on the
      // thread has a record like any other. Turning the seed off changes what
      // NEW threads do, not what this one does.
      //
      // It has to be recorded from the ARM rather than from a scroll. Whether
      // arming moves the container is incidental: a shared `.thread-content`
      // arriving on the outgoing thread's offset moves, a thread already at its
      // live edge does not. Recording off the scroll would give those two
      // readers different persistence for the same act.
      setFollowLiveEdge(true);
      stopFollowingBottom();

      const el = makeEl(4200, 5000); // already AT the live edge: arming writes nothing
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

      expect(isFollowScroll(el)).toBe(false);
      expect(el.scrollTop).toBe(0); // the shared-container reset, as before
      detach();
    });

    it('lets a RECORDED offset beat the seed', () => {
      // The direction that matters most: the reader parked here deliberately, and
      // a standing preference must not overrule what they did on this thread.
      setFollowLiveEdge(true);
      stopFollowingBottom();
      localStorage.setItem('k', '2000');

      const el = makeEl(300, 5000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });

      expect(isFollowScroll(el)).toBe(false);
      expect(el.scrollTop).toBe(2000);
      detach();
    });

    it('lets a RECORDED live edge beat the seed being off', () => {
      // And the mirror. The reader armed the follow here; turning the toggle off
      // somewhere else since is not them changing their mind about this thread.
      setFollowLiveEdge(false);
      localStorage.setItem('k', LIVE_EDGE_VALUE);

      const el = makeEl(300, 5000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({}),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });

      expect(isFollowScroll(el)).toBe(true);
      detach();
    });

    it('never seeds a container that is not the transcript', () => {
      // The follow is one global and this hook serves three containers. The
      // seed is read inside the same `followsLiveEdge` opt-in the record's own
      // live-edge form is. Without it, opening a file preview arms the follow.
      setFollowLiveEdge(true);
      stopFollowingBottom();

      const el = makeEl(300, 5000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), resetOnEmpty: true });

      expect(isFollowScroll(el)).toBe(false);
      detach();
    });

    it('records the live edge rather than the offset the follow happened to reach', () => {
      // Every growth round writes scrollTop. Recording the number would
      // overwrite the request on the next token, and re-entry would land
      // wherever the stream got to.
      const el = makeEl(100, 5000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), followsLiveEdge: true });

      armViaToggle(el);
      el.fireScroll();
      el.scrollHeight = 9000; // the reply keeps arriving
      el.scrollTop = 8200;
      detach();

      expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
    });

    it('records the request even when arming produced no scroll event at all', () => {
      // The ordinary idle case, not an exotic one. A reader already at the live
      // edge who presses the chevron gets a write the browser clamps. An idle
      // thread grows nothing. Nothing fires, so a save driven only by scroll
      // events would lose the request.
      const el = makeEl(4200, 5000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), followsLiveEdge: true });

      armViaToggle(el);
      detach();

      expect(localStorage.getItem('k')).toBe(LIVE_EDGE_VALUE);
    });

    it('records the offset the reader landed on when they scroll away, follow still armed', () => {
      // The listener-order case. `.thread-content` carries two scroll
      // listeners: the disarm in `makeScrollObservers` and this save. No
      // observers are wired here, so the flag is STILL armed when the save
      // runs. That is the order that breaks a save asking whether the follow is
      // armed. Asking where the container is answers the same in either order.
      const el = makeEl(100, 5000);
      const detach = attachScrollMemory(el, 'k', { live: () => ({}), followsLiveEdge: true });

      armViaToggle(el);
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

      armViaToggle(el);
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
      armViaToggle(el);
      detach();

      expect(localStorage.getItem('k')).toBe('1800');
    });

    it('returns the reader to TODAY\'s live edge, not the offset it was when they left', () => {
      // The thread grew from 5000 to 20000 while they were away. Restoring the
      // offset would strand them 15000px above the work they came back for.
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
      // The distinction the whole feature rests on: a position is not a
      // request. This reader is at the identical place as the one above, yet is
      // recorded as an offset. Re-entry returns them to it and follows nothing.
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

      armViaToggle(el);
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
      // Nothing remounts across a background. Without a wake signal the reader
      // comes back to the offset the transcript had when the app froze. The
      // whole batch the resync delivered then sits below them.
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
      // record rather than the follow flag. Note what has NOT happened here.
      // The 150ms debounce never fired, so the offset reached storage only
      // because the hide flushed it. Without that flush the stale live edge
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
      el.scrollTop = 19200;    // on the live edge, so the arm runs no tween
      setFollowLiveEdge(true); // the reader arms it, and the arm records the live edge
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

    it('defers to a deep-link that owns the open, and keeps the ride anyway', () => {
      // A push notification can resume the app and resolve a deep-link in one
      // breath, which is the MOBILE shape of the whole report. The event the
      // reader was sent to wins over the live edge, and the request survives:
      // the link owns the position, not what this thread asked for. The wake
      // used to decline outright, which is right about the write and wrong about
      // the request.
      localStorage.setItem('k', LIVE_EDGE_VALUE);
      const el = makeEl(4200, 20000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({ shouldRestore: () => false }),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });
      expect(el.scrollTop).toBe(4200); // stood down at attach
      expect(followingLiveEdge.value).toBe(true); // and resumed in place

      page.background();
      page.foreground();

      expect(el.scrollTop).toBe(4200); // and still stood down
      expect(followingLiveEdge.value).toBe(true);
      detach();
    });

    it('rebuilds a ride the wake itself destroyed, still without writing', () => {
      // The hazard the wake reads the RECORD for: a bfcache scroll restore fires
      // an event shaped exactly like the disarm, so the flag can be gone by the
      // time this runs. With a deep-link owning the open the answer is the same
      // one attach gives, the request back without the live edge over the event.
      localStorage.setItem('k', LIVE_EDGE_VALUE);
      const el = makeEl(4200, 20000);
      const detach = attachScrollMemory(el, 'k', {
        live: () => ({ shouldRestore: () => false }),
        resetOnEmpty: true,
        followsLiveEdge: true,
      });

      page.background();
      stopFollowingBottom();
      el.scrollHeight = 40000; // the turns that landed while they were away
      page.foreground();

      expect(followingLiveEdge.value).toBe(true);
      expect(el.scrollTop).toBe(4200); // the event, not the new live edge
      detach();
    });
  });
});
