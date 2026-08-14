import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

if (typeof (globalThis as any).HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class {};
}

import {
  followingLiveEdge,
  makeScrollObservers,
  readerGestureForTest,
  setActiveScrollElement,
  setFollowLiveEdge,
  setThreadLive,
  stopFollowingBottom,
} from '../scrollState';

/**
 * **What the listeners themselves make of each input.**
 *
 * The suite next door (`scroll-follow-the-live-edge.test.ts`) drives the
 * DECISION through `readerGestureForTest`, which states the fact the listeners
 * record without going near them. That is the right shape for asking what the
 * follow does with a gesture, and it leaves the other half unasked: whether a
 * given input produces one. The distinction is not academic. Every bug this
 * mechanism has had so far lived on this side of the line, in which event
 * counts as the reader:
 *
 *   - arming on `pointerdown` put a window over every press inside the
 *     transcript, and a press is how the reader answers a question or grants a
 *     permission (both of which must KEEP the follow);
 *   - reading `offsetX` off a bubbled press measured the wrong box, so an
 *     ordinary tap low in a tall turn read as a press in the scrollbar gutter;
 *   - `Cmd+ArrowDown` is turn stepping, not a scroll key, and stamping it
 *     defeated the one case `stepThreadTurn` keeps the ride for.
 *
 * So this file drives real events at the real listeners, through a container
 * double that records what was registered on it.
 */
describe('what the reader-gesture listeners count as a scroll', () => {
  /** A container that records its listeners, plus `window`'s, so a test can
   *  fire either and the teardown can be checked for both. */
  function makeContainer() {
    const own: Record<string, Function[]> = {};
    const el: any = {
      parentElement: null,
      children: [],
      clientWidth: 800,
      clientHeight: 500,
      scrollHeight: 3000,
      _scrollTop: 2500,
      get scrollTop() { return this._scrollTop; },
      set scrollTop(v: number) {
        this._scrollTop = Math.min(Math.max(0, v), Math.max(0, this.scrollHeight - this.clientHeight));
      },
      getBoundingClientRect: () => ({ width: 800, height: 500, top: 0, bottom: 500, left: 0, right: 800 }),
      querySelectorAll: () => [],
      listeners: own,
      addEventListener(type: string, fn: Function) { (own[type] ??= []).push(fn); },
      removeEventListener(type: string, fn: Function) {
        own[type] = (own[type] ?? []).filter(f => f !== fn);
      },
      /** Fire an event at the container's own listeners. `target` defaults to
       *  the container, which is what a press on its scrollbar reports. */
      fire(type: string, event: Record<string, unknown> = {}) {
        for (const fn of own[type] ?? []) fn({ target: el, ...event });
      },
    };
    return el;
  }

  let windowListeners: Record<string, Function[]>;
  let realWindow: any;

  beforeEach(() => {
    windowListeners = {};
    realWindow = (globalThis as any).window;
    (globalThis as any).window = {
      addEventListener(type: string, fn: Function) { (windowListeners[type] ??= []).push(fn); },
      removeEventListener(type: string, fn: Function) {
        windowListeners[type] = (windowListeners[type] ?? []).filter(f => f !== fn);
      },
    };
    stopFollowingBottom();
    setActiveScrollElement(null);
    setThreadLive(true);
    readerGestureForTest(null, false);
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    (globalThis as any).window = realWindow;
    stopFollowingBottom();
    setActiveScrollElement(null);
    readerGestureForTest(null, false);
  });

  /** An armed reader on a live thread, with the listeners really attached. */
  function riding() {
    const el = makeContainer();
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    expect(followingLiveEdge.value).toBe(true);
    return { el, ...observers };
  }

  /** Move the container the way the platform does: no input event at all. */
  function platformScrollsTo(el: any, top: number, onScroll: () => void) {
    el.scrollTop = top;
    onScroll();
  }

  /** `riding()`, plus the round that records the reader ON the live edge.
   *
   *  The tests above ask only what the listeners make of an input, and the
   *  follow's flag answers that without any snapshot. The two focus cases ask
   *  what the transcript then DOES, and the platform-scroll correction is gated
   *  on having measured the reader at the edge beforehand (`anchorAtLiveEdge`,
   *  taken at the end of every scroll and resize round). Without this round
   *  both of them would pass by writing nothing, for the wrong reason. */
  function ridingAndAnchored() {
    const r = riding();
    r.onScroll();
    return r;
  }

  it('attaches to the container, and the releases to window', () => {
    const el = makeContainer();
    const { detachGestures } = makeScrollObservers(el);

    expect(Object.keys(el.listeners).sort()).toEqual(
      ['focusin', 'keydown', 'pointerdown', 'pointermove', 'touchmove', 'wheel'],
    );
    // The release goes on `window`: a drag that ends with the pointer outside
    // the transcript would otherwise leave the press recorded forever.
    expect(Object.keys(windowListeners).sort()).toEqual(
      ['pointercancel', 'pointerup', 'touchcancel', 'touchend'],
    );

    detachGestures();
    for (const fns of Object.values(el.listeners)) expect(fns).toEqual([]);
    for (const fns of Object.values(windowListeners)) expect(fns).toEqual([]);
  });

  it('counts a wheel notch', () => {
    const { el, onScroll } = riding();
    el.fire('wheel');
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('counts a finger travelling', () => {
    const { el, onScroll } = riding();
    el.fire('touchmove');
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('counts a scroll key', () => {
    const { el, onScroll } = riding();
    el.fire('keydown', { key: 'PageDown' });
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('does NOT count a chord, which is a shortcut rather than a scroll key', () => {
    // Cmd+Arrow is turn stepping. Stamping it would retire the ride from
    // `onScroll` mid-glide, defeating the one case `stepThreadTurn` keeps it
    // for: a step onto the last turn, which lands at the live edge anyway.
    const { el, onScroll } = riding();
    el.fire('keydown', { key: 'ArrowDown', metaKey: true });
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('does NOT count a key that scrolls nothing', () => {
    const { el, onScroll } = riding();
    el.fire('keydown', { key: 'a' });
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('does NOT count a scroll key pressed on a control INSIDE the transcript', () => {
    // `keydown` bubbles, and the transcript is full of controls that take these
    // exact keys and scroll nothing: Space on a focused button is how the
    // reader ANSWERS a question card, Home/End and the arrows move a caret in a
    // text field. Each changes content, so stamping them would put a window
    // over the very interactions the press rule refuses.
    const { el, onScroll } = riding();
    const answerButton = { nodeName: 'BUTTON' };

    el.fire('keydown', { key: ' ', target: answerButton });
    platformScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(true);
  });

  it('but LEAVES the reader where that key scrolled them, since it is still theirs', () => {
    // The other half, and the one the platform-scroll correction made visible.
    // A scroll key the focused control does not consume still scrolls the
    // transcript, because the browser scrolls the nearest scrollable ancestor,
    // and the choice-card seeding parks focus on a button INSIDE the transcript
    // by design. So a reader answering a question and then paging back through
    // the reply is in exactly this state. Not a gesture (the ride survives,
    // asserted above), but not the platform either: the correction has to stand
    // down or keyboard scrolling is undone the instant it happens. Codex named
    // it P1 in `/harden`, 2026-08-13.
    const { el, onScroll } = ridingAndAnchored();

    el.fire('keydown', { key: 'PageUp', target: { nodeName: 'BUTTON' } });
    platformScrollsTo(el, 900, onScroll);

    expect(el.scrollTop).toBe(900);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('and answers the platform again once that keypress is four frames old', () => {
    // A window, like every other answer to "was that scroll ours". A PageUp four
    // frames ago says nothing about the keyboard adjusting the offset now.
    const { el, onScroll } = ridingAndAnchored();

    el.fire('keydown', { key: 'PageUp', target: { nodeName: 'BUTTON' } });
    vi.advanceTimersByTime(200);
    platformScrollsTo(el, 900, onScroll);

    expect(el.scrollTop).toBe(2500);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('does NOT count FOCUS landing inside it, and keeps the reader where it went', () => {
    // The other way the container scrolls with nobody writing `scrollTop`: the
    // browser reveals a focused control that is off screen, for Tab, Shift+Tab,
    // a screen reader moving the cursor, or any `focus()` without
    // `preventScroll`. It is a NAVIGATION rather than a gesture, so the reader
    // keeps the lit toggle AND the place the browser took them to.
    //
    // Both halves are asserted, because the ride surviving is worth nothing if
    // the correction then writes them back: that is exactly what happened
    // before the `focusin` stamp, and Tab appeared to do nothing while the
    // control it moved to sat off screen with an invisible ring. Found by the
    // Codex reviewer in `/harden`, 2026-08-13.
    const { el, onScroll } = ridingAndAnchored();
    const buttonInAnOlderTurn = { nodeName: 'BUTTON' };

    el.fire('focusin', { target: buttonInAnOlderTurn });
    platformScrollsTo(el, 400, onScroll);

    expect(followingLiveEdge.value).toBe(true);
    expect(el.scrollTop).toBe(400);
  });

  it('but the armed follow still carries them back on the next GROWTH round', () => {
    // The limit of what a reveal buys, stated so nobody reads the two cases
    // above as more than they are. The correction stands down for a reveal;
    // `honourGrowth` does not, because ARMED AND LIVE is the whole of what
    // riding the live edge means and only a GESTURE takes it away. So on a
    // streaming thread a Tab reveal survives its own scroll event and the next
    // token carries the reader back, exactly as it did before any of this.
    //
    // Deliberately not "fixed" by retiring the follow on a focus, which was the
    // obvious symmetry with the up chevron and turn stepping. Focus is not
    // always the reader's: `seedChoiceCardFocus` moves it onto an arriving
    // card's default choice, and a card can arrive inside a submit's live
    // claim, so retiring here would take the ride away from a reader who
    // touched nothing. That is the exact class the gesture term exists to
    // refuse. The way off a ride while streaming stays what it has always
    // been: scroll, or press the toggle.
    const { el, onScroll, onResize } = ridingAndAnchored();

    el.fire('focusin', { target: { nodeName: 'BUTTON' } });
    platformScrollsTo(el, 400, onScroll);
    expect(el.scrollTop).toBe(400);

    el.scrollHeight = 3100;   // the next token
    onResize();

    expect(el.scrollTop).toBe(2600);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('and leaves them on the reveal for as long as the thread is IDLE', () => {
    // The other side of the same line, and the one that matters for the case
    // the reveal marking was added for: nothing is streaming, so nothing writes,
    // and the reader stays on the control they tabbed to for as long as they
    // like. Growth on an idle thread is the transcript finishing its own
    // rendering (`followIsCarrying`), so it carries nobody.
    const { el, onScroll, onResize } = ridingAndAnchored();
    setThreadLive(false);

    el.fire('focusin', { target: { nodeName: 'BUTTON' } });
    platformScrollsTo(el, 400, onScroll);

    el.scrollHeight = 3100;   // a late image decoding, a card mounting
    onResize();

    expect(el.scrollTop).toBe(400);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('and answers the platform again once the focus reveal is four frames old', () => {
    // The stamp is a WINDOW, like every other answer to "was that scroll ours":
    // a focus move four frames ago says nothing about a scroll now, and if it
    // did, one Tab would exempt the rest of the thread from the correction.
    const { el, onScroll } = ridingAndAnchored();

    el.fire('focusin', { target: { nodeName: 'BUTTON' } });
    vi.advanceTimersByTime(200);
    platformScrollsTo(el, 400, onScroll);

    expect(el.scrollTop).toBe(2500);   // carried back to the live edge
    expect(followingLiveEdge.value).toBe(true);
  });

  it('counts a scrollbar DRAG for as long as the thumb is held', () => {
    // The press stamps once; a slow haul down the bar can outlast the window,
    // so the moves under it keep the signal fresh.
    const { el, onScroll } = riding();
    el.fire('pointerdown', { offsetX: el.clientWidth + 6, offsetY: 200 });
    vi.advanceTimersByTime(1000);
    el.fire('pointermove', { buttons: 1 });
    vi.advanceTimersByTime(1000);
    el.fire('pointermove', { buttons: 1 });
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('does NOT count a pointer merely crossing the transcript', () => {
    // On a desktop this fires constantly. Without the press gate the signal
    // would be permanently in flight and every platform scroll would retire.
    const { el, onScroll } = riding();
    el.fire('pointermove', { buttons: 0 });
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('does NOT count the jitter every real click on a control carries', () => {
    // A press on content never arms the drag path at all, so the sub-pixel
    // movement between pressing a question card's button and releasing it
    // cannot stamp. Recording content presses is what made that reachable.
    const { el, onScroll } = riding();
    const answerButton = { nodeName: 'BUTTON' };

    el.fire('pointerdown', { target: answerButton, offsetX: 40, offsetY: 12 });
    el.fire('pointermove', { buttons: 1 }); // the finger settling on the button
    el.fire('pointermove', { buttons: 1 });
    platformScrollsTo(el, 900, onScroll);   // and the card's reflow

    expect(followingLiveEdge.value).toBe(true);
  });

  it('counts a press in the scrollbar GUTTER, which scrolls with no movement', () => {
    // Clicking the track pages the transcript in one jump, so there is no move
    // to wait for. The gutter is outside the client box, which is what makes
    // this the one press that cannot be a content control.
    const { el, onScroll } = riding();
    el.fire('pointerdown', { offsetX: el.clientWidth + 6, offsetY: 200 });
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('does NOT count an ordinary press, wherever in the transcript it lands', () => {
    // Answering a question, granting a permission, expanding a turn. The
    // `offsetY` case is the one that shipped broken: `offsetX`/`offsetY` are
    // measured from the TARGET's box and `pointerdown` bubbles, so a tap low in
    // a turn taller than the viewport reported an offset past the container's
    // client box and read as the gutter.
    const { el, onScroll } = riding();
    const tallTurn = { nodeName: 'DIV' };

    el.fire('pointerdown', { target: tallTurn, offsetX: 900, offsetY: 4000 });
    platformScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(true);
  });

  it('forgets a scrollbar press released where it could not see it', () => {
    // A release over a nested iframe, or one that happens while the PWA is
    // backgrounded, never reaches the window listener. The next move with no
    // button held is what clears it, so the press cannot stick and turn an
    // ordinary hover into a drag.
    const { el, onScroll } = riding();

    el.fire('pointerdown', { offsetX: el.clientWidth + 6, offsetY: 200 });
    el.fire('pointermove', { buttons: 0 }); // the release we never saw
    vi.advanceTimersByTime(2000);           // and the press's own stamp lapses
    el.fire('pointermove', { buttons: 0 }); // now just a hover
    platformScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(true);
  });

  it('ends the press on a release anywhere, including outside the transcript', () => {
    const { el, onScroll } = riding();

    el.fire('pointerdown', { offsetX: el.clientWidth + 6, offsetY: 200 });
    for (const fn of windowListeners['pointerup'] ?? []) fn({});
    vi.advanceTimersByTime(2000);           // the press's own stamp lapses
    el.fire('pointermove', { buttons: 1 }); // a drag that belongs to something else
    platformScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(true);
  });
});
