import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readdirSync, readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, join, resolve } from 'node:path';

// Stub HTMLElement before importing modules that reference it
if (typeof globalThis.HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class HTMLElement {};
}

import {
  awayFromBottom,
  followAnsweredQuestion,
  followContinuedThread,
  followResolvedPermission,
  followSentMessage,
  followLiveEdgeSeed,
  followingLiveEdge,
  honourAnchoredMutation,
  isFollowScroll,
  isNavigationScroll,
  markNavigationScroll,
  markRevealScroll,
  makeScrollObservers,
  readerGestureForTest,
  resumeFollowingBottom,
  scrollToBottom,
  scrollToBottomAnimated,
  scrollToTop,
  setActiveScrollElement,
  setFollowLiveEdge,
  setThreadLive,
  stopFollowingBottom,
} from '../scrollState';

/** The follow toggle is a STANDING ask: take me to the live edge and keep me
 *  there until I say otherwise. This file pins its duration (growth honours it,
 *  and only the reader's own scroll retires it) and, against it, the one-shot
 *  reaction every submit gets. A submit arms nothing.
 *
 *  Its mirror is `scroll-resize-never-follows.test.ts`: growth moves an UNARMED
 *  reader zero pixels, including one sitting exactly at the live edge. A
 *  position is not a request, and that is the whole point of both files.
 *  Rationale: ADR 0064. */

/** How tall the agent status line is in this fake. No expected offset is derived
 *  from it: it stands for the row that must end up IN VIEW under the landed
 *  turn. */
const STATUS_LINE_HEIGHT = 28;

/** The landing line: how far below the transcript's top a landed turn's top
 *  comes to rest. `turnLandingClearancePx` reads `.chat-exchange`'s computed
 *  `scroll-margin-top` and falls back to this with no layout to ask, which is
 *  every test here except the one mocking `getComputedStyle`.
 *
 *  The line is only REACHABLE when the container can scroll that far, so each
 *  test sets `scrollHeight` by hand for the case it is about: headroom past the
 *  turn for the line, none for the reveal branch. */
const LANDING_CLEARANCE = 8;

/** A `.thread-content` stand-in that clamps `scrollTop` the way a browser does,
 *  counts writes, and holds the panels and cards a landing measures. Counting
 *  writes is what fails a jump landing where the reader already was.
 *
 *  `panels` are given in SCROLL coordinates (`top` from the top of the content).
 *  Their rect is derived from the live `scrollTop` on every read, so a tween
 *  measuring per frame sees a moving target, as in a browser. */
function makeEl(opts: {
  scrollTop: number;
  scrollHeight: number;
  clientHeight?: number;
  panels?: Array<{ top: number; height: number }>;
  /** How many `.chat-exchange` turns the transcript holds. Defaults to ONE, the
   *  ordinary transcript every follow test assumes. `0` is the brand-new
   *  thread: a compose view showing the welcome message, or a promoted thread
   *  whose first optimistic row has not rendered. */
  turns?: number;
}) {
  const panels: any[] = [];
  const questionCards: any[] = [];
  const permissionCards: any[] = [];
  const turns: any[] = [];
  /** An inline style declaration the module may write to, kept so a test can
   *  assert that it does not. Nothing publishes a custom property onto the
   *  transcript: the tail-room guard below is what keeps it that way. */
  const styleProps = new Map<string, string>();
  const el: any = {
    parentElement: null,
    children: [],
    style: {
      setProperty: (name: string, value: string) => { styleProps.set(name, value); },
      getPropertyValue: (name: string) => styleProps.get(name) ?? '',
    },
    /** The turns array is in DOM order, so its tail is the newest turn. */
    get lastElementChild() { return turns[turns.length - 1] ?? null; },
    clientWidth: 800,
    clientHeight: opts.clientHeight ?? 500,
    scrollHeight: opts.scrollHeight,
    writes: 0,
    _scrollTop: opts.scrollTop,
    get scrollTop() { return this._scrollTop; },
    set scrollTop(v: number) {
      this.writes++;
      this._scrollTop = Math.min(Math.max(0, v), Math.max(0, this.scrollHeight - this.clientHeight));
    },
    getBoundingClientRect: () => ({
      width: 800, height: el.clientHeight, top: 0, bottom: el.clientHeight, left: 0, right: 800,
    }),
    querySelectorAll: (selector: string) => (
      selector === '.initiator-panel-user' ? panels
        : selector === '.question-body' ? questionCards
          : selector === '.permission-body' ? permissionCards
            : selector === '.chat-exchange' ? turns
              : []
    ),
    /** Render one more user message, the way an optimistic send row arrives.
     *  `visible: false` models a panel with no box: a queued follow-up folded
     *  into its closed disclosure group.
     *
     *  It adds a TURN too, since a user message renders inside a
     *  `.chat-exchange`. The turn is given the ROW's own rect, which is the
     *  DOM's answer. A turn renders as `[initiator panel, response panel?]`, so
     *  the exchange's top IS the row's top. A turn parked at 0 while its row sat
     *  lower would let a landing aimed at the wrong element pass.
     *
     *  The turn arrives WITH its agent status line, as the app commits the row
     *  and its "Requesting" panel together. `status: false` is the turn that
     *  never gets one: the queued follow-up, and the frame before the response
     *  panel mounts. */
    addUserMessage(p: { top: number; height: number; visible?: boolean; status?: boolean }) {
      const turn = makeTurn(p.top, p.height);
      const panel: any = {
        parentElement: null,
        isConnected: true,
        turn,
        closest: (sel: string) => (sel === '.chat-exchange' ? turn : null),
        getBoundingClientRect: () => (p.visible === false
          ? { width: 0, height: 0, top: 0, bottom: 0, left: 0, right: 0 }
          : {
              width: 800,
              height: p.height,
              top: p.top - el.scrollTop,
              bottom: p.top + p.height - el.scrollTop,
              left: 0,
              right: 800,
            }),
      };
      if (p.visible !== false && p.status !== false) mountStatusLine(turn, p.top + p.height);
      panels.push(panel);
      return panel;
    },
    /** Mount the `.response-header` of an already-rendered turn: the response
     *  panel arriving a frame after the row it belongs to. */
    mountStatusLine(panel: any) {
      const rect = panel.getBoundingClientRect();
      return mountStatusLine(panel.turn, rect.bottom + el.scrollTop);
    },
    /** Render a question card: a `.question-body` carrying the card's tool-use
     *  id inside the `.initiator-panel` that is the answer's landing target. */
    addQuestionCard(p: { toolUseId: string; top: number; height: number; status?: boolean }) {
      return addCard(questionCards, 'data-tool-use-id', p.toolUseId, p);
    },
    /** Render a permission-shaped card: a `.permission-body` carrying the card's
     *  REQUEST id, inside the same `.initiator-panel` a question card sits in.
     *  One shape covers all three, since the coding-agent tool permission, the
     *  command guard and the MCP tool consent share `PermissionBodyShell`. */
    addPermissionCard(p: { requestId: string; top: number; height: number; status?: boolean }) {
      return addCard(permissionCards, 'data-request-id', p.requestId, p);
    },
    /** Render the turn a Continue produces: a fresh `ContinuationStarted`
     *  exchange, which carries an agent status line and NO user message row (the
     *  reader submitted a button press, not content). Continue's landing anchors
     *  on the exchange itself for exactly that reason. */
    addContinuationTurn(p: { top: number; height: number; status?: boolean }) {
      const turn = makeTurn(p.top, p.height);
      if (p.status !== false) mountStatusLine(turn, p.top + p.height);
      return turn;
    },
    /** The live to answered swap, as Preact performs it. `QuestionBody` returns
     *  a DIFFERENT component once the answer lands, so the body node is
     *  remounted while the `.initiator-panel` around it is REUSED. The panel
     *  object is therefore carried over and only the body is replaced. */
    answerQuestionCard(card: { body: any; panel: any }) {
      card.body.isConnected = false;
      questionCards.splice(questionCards.indexOf(card.body), 1);
      const answered = {
        ...card.body,
        isConnected: true,
        closest: (sel: string) => (sel === '.initiator-panel' ? card.panel : null),
      };
      questionCards.push(answered);
      return { body: answered, panel: card.panel };
    },
    /** Give the container a child for the reflow anchor to hold onto. `top` is
     *  its viewport top, which a width change moves. */
    addAnchorChild(top: number) {
      const child = { isConnected: true, _top: top, getBoundingClientRect() { return { width: 800, height: 200, top: this._top, bottom: this._top + 200, left: 0, right: 800 }; } };
      el.children.push(child);
      return child;
    },
  };
  /** One `.chat-exchange`, with the slot its agent status line mounts into. It
   *  carries a rect and a `closest` of its own because Continue's landing anchors
   *  on the EXCHANGE rather than on a panel inside it: `lastTurn` measures it to
   *  reject an invisible one, and `landOnOwnTurn` walks `closest` from it (which
   *  answers with the element itself, as the DOM's does). */
  function makeTurn(top = 0, height = 100) {
    const turn: any = {
      parentElement: null,
      isConnected: true,
      statusLine: null,
      closest: (sel: string) => (sel === '.chat-exchange' ? turn : null),
      matches: (sel: string) => sel === '.chat-exchange',
      querySelector: (sel: string) => (sel === '.response-header' ? turn.statusLine : null),
      getBoundingClientRect: () => ({
        width: 800, height, top: top - el.scrollTop, bottom: top + height - el.scrollTop, left: 0, right: 800,
      }),
    };
    turns.push(turn);
    return turn;
  }

  /** The shared body of `addQuestionCard` / `addPermissionCard`: a card body
   *  carrying `value` under `attr`, inside the `.initiator-panel` that is the
   *  landing target. Body and panel are given DIFFERENT rects, so a test landing
   *  on the panel cannot pass by measuring the body instead. */
  function addCard(
    into: any[],
    attr: string,
    value: string,
    p: { top: number; height: number; status?: boolean },
  ) {
    const turn = makeTurn(p.top, p.height);
    const rect = (top: number, height: number) => () => ({
      width: 800, height, top: top - el.scrollTop, bottom: top + height - el.scrollTop, left: 0, right: 800,
    });
    const panel = {
      parentElement: null,
      isConnected: true,
      turn,
      closest: (sel: string) => (sel === '.chat-exchange' ? turn : null),
      getBoundingClientRect: rect(p.top, p.height),
    };
    const body = {
      parentElement: null,
      isConnected: true,
      getAttribute: (name: string) => (name === attr ? value : null),
      closest: (sel: string) => (sel === '.initiator-panel' ? panel : null),
      getBoundingClientRect: rect(p.top + 20, Math.max(0, p.height - 40)),
    };
    if (p.status !== false) mountStatusLine(turn, p.top + p.height);
    into.push(body);
    return { body, panel };
  }

  /** The turn's `.response-header`: the row carrying the executor's name and the
   *  live Requesting / Working label, sitting directly under what the reader
   *  produced. It is what a submit's landing aims at, so its rect follows the
   *  scroll position exactly as the panels' do. */
  function mountStatusLine(turn: any, top: number) {
    turn.statusLine = {
      parentElement: null,
      isConnected: true,
      getBoundingClientRect: () => ({
        width: 800,
        height: STATUS_LINE_HEIGHT,
        top: top - el.scrollTop,
        bottom: top + STATUS_LINE_HEIGHT - el.scrollTop,
        left: 0,
        right: 800,
      }),
    };
    return turn.statusLine;
  }

  for (let i = 0; i < (opts.turns ?? 1); i++) makeTurn();
  for (const p of opts.panels ?? []) el.addUserMessage(p);
  return el;
}

/** Park the reader exactly at the live edge without counting it as a write. */
function atBottom(el: any) {
  el._scrollTop = Math.max(0, el.scrollHeight - el.clientHeight);
  return el._scrollTop;
}

/** The follow is module state, so a test that armed it would leak into the next
 *  one. Retiring it is the call `focusThread` makes on opening another thread,
 *  not a test-only hatch.
 *
 *  It also puts the thread in its LIVE state, which is what every test here
 *  except the idle-scroll pair is about. The disarm asks whether the agent is
 *  live, so a file-wide default of `false` would make every disarm test pass
 *  for the wrong reason. Tests that care about idle say so explicitly. */
function resetFollow() {
  stopFollowingBottom();
  setActiveScrollElement(null);
  setThreadLive(true);
  awayFromBottom.value = false;
  readerGestureForTest(null, false);
}

/** THE READER'S OWN SCROLL: their hand on the transcript, the position it left
 *  it at, and the event that follows a frame later.
 *
 *  Both halves are required. A scroll retires the follow only when a GESTURE is
 *  behind it: the position alone cannot tell a flick from the iOS keyboard
 *  resizing the container (ADR 0064). So writing `scrollTop` and calling
 *  `onScroll` WITHOUT this helper models the platform moving the container,
 *  which deliberately retires nothing.
 *
 *  Every kind of reader scroll is one call: a wheel notch, a scrollbar drag, a
 *  touch flick and its momentum, a scroll key. The signal does not distinguish
 *  them, and neither does the follow. */
function readerScrollsTo(el: { scrollTop: number }, top: number, onScroll: () => void) {
  readerGestureForTest(el as unknown as HTMLElement);
  el.scrollTop = top;
  onScroll();
}

describe('the follow toggle arms a standing follow', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('keeps the reader at the live edge as the reply keeps growing, not just once', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500); // the toggle GLIDES to the live edge, as the chevron does
    expect(el.scrollTop).toBe(2500);

    for (const [height, expected] of [[3400, 2900], [4000, 3500], [9000, 8500]]) {
      el.scrollHeight = height;
      onResize();
      expect(el.scrollTop).toBe(expected);
    }
    expect(awayFromBottom.value).toBe(false);
  });

  it('leaves a reader who never pressed it alone under exactly the same growth', () => {
    // The crux: the identical position and the identical growth as the test
    // above, and not a pixel of movement. Following is an explicit request, not
    // a proximity to the bottom.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const parked = atBottom(el);
    el.writes = 0;

    el.scrollHeight = 3400;
    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
    expect(awayFromBottom.value).toBe(true);
  });

  it('is not retired by its own trailing scroll event, even once content has grown past it', () => {
    // A scrollTop write fires its scroll event a frame LATER. By then a
    // streaming thread has often grown, so the container reads as off the live
    // edge. Position, not proximity, tells the follow's own write from the
    // reader's gesture: growth changes scrollHeight and never scrollTop.
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    el.scrollHeight = 3400;       // a chunk lands before the scroll event does
    onScroll();                   // the arm's own event, arriving late

    el.scrollHeight = 3800;
    onResize();
    expect(el.scrollTop).toBe(3300); // still following
  });

  it('is retired by a scroll up, and later growth then leaves the reader alone', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    setFollowLiveEdge(true);
    readerScrollsTo(el, 2000, onScroll); // wheel, drag, flick, momentum or a keypress

    el.scrollHeight = 3400;
    onResize();
    expect(el.scrollTop).toBe(2000);

    el.scrollHeight = 9000;
    onResize();
    expect(el.scrollTop).toBe(2000);
  });

  it('is retired by the up chevron, which is the reader asking to be elsewhere', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    setFollowLiveEdge(true);
    scrollToTop();
    vi.advanceTimersByTime(1500);
    onScroll();
    expect(el.scrollTop).toBe(0);

    el.scrollHeight = 3400;
    onResize();
    expect(el.scrollTop).toBe(0); // not yanked back down by the next token
  });

  it('survives everything that is not a scroll: a card resolving, expanding a turn', () => {
    // Only a scroll retires the follow. A card resolving swaps content under
    // the reader and a disclosure grows it, and neither is the reader saying
    // they want to be somewhere else. What this pins is that the resolution's
    // reflow does not retire a follow already armed, however it was armed.
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);

    el.scrollHeight = 2800;  // the question card is replaced by the answer
    onResize();
    onScroll();              // the browser clamping the reader down fires one
    expect(el.scrollTop).toBe(2300);

    el.scrollHeight = 3600;  // and a turn is expanded
    onResize();
    expect(el.scrollTop).toBe(3100);
  });
});

describe('the chevron navigates and arms nothing', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** The chevron is a navigation like the up chevron and turn stepping: it
   *  takes the reader to the bottom and stops. The follow has a button of its
   *  own. Why the chevron must not arm one: ADR 0064. */

  it('takes the reader to the live edge, and the next token leaves them there', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    scrollToBottom();
    expect(el.scrollTop).toBe(2500);
    el.writes = 0;

    el.scrollHeight = 3400;
    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2500);
    expect(awayFromBottom.value).toBe(true); // and it is their way back down
  });

  it('the animated form arms nothing either, once its glide lands', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    scrollToBottomAnimated();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2500);
    el.writes = 0;

    el.scrollHeight = 3400;
    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2500);
  });
});

describe('the follow toggle is the whole of the follow\'s controls', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('ON glides to the live edge from wherever the reader is, and arms', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    setFollowLiveEdge(true);
    expect(el.scrollTop).toBe(100); // a glide, not a jump
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2500);
    expect(followingLiveEdge.value).toBe(true);

    el.scrollHeight = 3400;
    onResize();
    expect(el.scrollTop).toBe(2900);
  });

  it('ON writes no scroll for a reader already at the live edge', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const parked = atBottom(el);
    el.writes = 0;

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
    expect(followingLiveEdge.value).toBe(true);
    expect(awayFromBottom.value).toBe(false);

    el.scrollHeight = 4000; // and it really did arm
    onResize();
    expect(el.scrollTop).toBe(3500);
  });

  it('OFF disarms and writes NO scroll, leaving the reader where they read', () => {
    // Turning a mode off is not a request to be moved.
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    el.scrollHeight = 4000;
    onResize();
    expect(el.scrollTop).toBe(3500);
    el.writes = 0;

    setFollowLiveEdge(false);
    vi.advanceTimersByTime(1500);
    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(3500);
    expect(followingLiveEdge.value).toBe(false);

    el.scrollHeight = 9000;
    onResize();
    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(3500);
  });

  it('OFF stops a glide of its own that is still in flight', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(60);
    const midGlide = el.scrollTop;
    expect(midGlide).toBeGreaterThan(100);

    setFollowLiveEdge(false);
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(midGlide);
  });

  it('goes off by itself when the reader scrolls away from a live reply', () => {
    // The button RENDERS the follow rather than owning it, which is why the off
    // tap is a convenience: the reader's own scroll is the mechanism, and the
    // signal the button reads is the one the disarm writes.
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    expect(followingLiveEdge.value).toBe(true);

    readerScrollsTo(el, 1200, onScroll); // a wheel, a flick, a keypress

    expect(followingLiveEdge.value).toBe(false);
  });
});

/**
 * **Only the READER retires a follow, and only with their hand on the
 * transcript or on a chevron.**
 *
 * The question is asked of the INPUT, never of the position. Three things move
 * the container with no gesture behind them:
 *
 *   - the soft keyboard opening or closing, which rewrites `--app-height` and
 *     lets WebKit adjust the offset through its ~350ms animation;
 *   - an app backgrounded and resumed onto an offset nobody wrote;
 *   - the full response / steps toggle, whose anchor correction is a write of
 *     ours made from a position we could not stamp in advance.
 *
 * Why the position alone cannot answer it: ADR 0064. These tests are the two
 * directions: the platform moves the container and the ride survives, the
 * reader moves it and the ride ends.
 */
describe('a scroll retires the follow only when the READER made it', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** An armed reader riding a live thread, which is the state all of these are
   *  about. Returns the observers so each case can move the container its own
   *  way. */
  function riding() {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000, clientHeight: 500 });
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    setThreadLive(true);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    expect(followingLiveEdge.value).toBe(true);
    return { el, ...observers };
  }

  it('survives the iOS keyboard opening under a streaming reply', () => {
    // The keyboard takes ~300px of viewport, so the transcript shrinks and the
    // reader is left well above the live edge. WebKit then adjusts the offset
    // itself. No finger has touched the transcript in any of it.
    const { el, onScroll, onResize } = riding();

    el.clientHeight = 200;
    onResize();
    el.scrollTop = 1400;
    onScroll();

    expect(followingLiveEdge.value).toBe(true);
  });

  it('survives the keyboard closing again', () => {
    // Each step is the resize AND an offset WebKit adjusted on its own, off the
    // live edge. The offset is what makes this test say anything. A resize
    // alone runs `honourGrowth`, which re-stamps the hold, so `atEdge` and
    // `tookOver` would answer "not the reader" before the gesture term is
    // consulted.
    const { el, onScroll, onResize } = riding();

    for (const clientHeight of [200, 260, 340, 420, 500]) {
      el.clientHeight = clientHeight;
      onResize();
      el.scrollTop = Math.max(0, el.scrollTop - 200);
      onScroll();
    }

    expect(followingLiveEdge.value).toBe(true);
  });

  it('survives an app backgrounded and resumed onto a restored offset', () => {
    // Nothing observes the transcript while the app is away, so the resume
    // arrives as one scroll event at a position nobody here wrote.
    const { el, onScroll } = riding();

    el.scrollTop = 0;
    onScroll();

    expect(followingLiveEdge.value).toBe(true);
  });

  it('survives the platform scrolling the container for any other reason', () => {
    // A focus ring brought into view, a restored session, a UA doing what UAs
    // do. The rule is about the ABSENCE of a gesture, not about enumerating the
    // platform's reasons.
    const { el, onScroll } = riding();

    el.scrollTop = 700;
    onScroll();
    el.scrollTop = 1900;
    onScroll();

    expect(followingLiveEdge.value).toBe(true);
  });

  it('ends on the reader flicking, and on every other way they scroll', () => {
    // One signal covers the lot: a wheel notch, a scrollbar drag, a touch flick
    // and its momentum, a scroll key. The follow does not distinguish them, so
    // neither does this.
    const { el, onScroll } = riding();

    readerScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(false);
  });

  it('ends on the coast after the finger lifts, not just under it', () => {
    // iOS momentum fires its scroll events AFTER `touchend`, so the window has
    // to outlive the gesture. A flick that only crosses the live-edge threshold
    // during the coast would otherwise read as the platform's.
    const { el, onScroll } = riding();

    readerGestureForTest(el);     // touchstart … touchend, finger gone
    vi.advanceTimersByTime(200);  // and the coast begins
    el.scrollTop = 900;
    onScroll();

    expect(followingLiveEdge.value).toBe(false);
  });

  it('ends on the up chevron, which is a press rather than a gesture', () => {
    // The tap lands on the BUTTON, so no gesture reaches the transcript and the
    // press has to retire the ride itself.
    riding();

    scrollToTop();

    expect(followingLiveEdge.value).toBe(false);
  });

  it('keeps the ride when the up chevron is pressed on an IDLE thread', () => {
    // Same rule as the scroll disarm: re-reading a thread nothing is writing to
    // is browsing, and the reader's next submit should still carry them.
    riding();
    setThreadLive(false);

    scrollToTop();

    expect(followingLiveEdge.value).toBe(true);
  });

  it('keeps the ride when the platform scrolls an IDLE thread', () => {
    const { el, onScroll } = riding();
    setThreadLive(false);

    el.scrollTop = 0;
    onScroll();

    expect(followingLiveEdge.value).toBe(true);
  });

  it('lets the coast lapse, so a scroll long after the flick is the platform again', () => {
    // The half that keeps it a WINDOW rather than a latch. A flick five seconds
    // ago says nothing about a scroll now. If it did, the keyboard opening after
    // any earlier scroll would retire the follow all over again.
    const { el, onScroll } = riding();

    readerGestureForTest(el);
    vi.advanceTimersByTime(5000);
    el.scrollTop = 900;
    onScroll();

    expect(followingLiveEdge.value).toBe(true);
  });

  it('does not read a PRESS inside the transcript as a scroll', () => {
    // Answering a question, granting a permission, expanding a turn: each is a
    // press on a control INSIDE the transcript, and each changes content. That
    // combination keeps the follow. Arming attribution on the press would put a
    // 1.2s window over every one of them, so only MOVEMENT arms it. Modelled as
    // the listeners see it: a press records no movement, so the signal stays
    // cold and the content change is attributed to the app.
    const { el, onScroll } = riding();

    readerGestureForTest(el, false); // pressed, and never moved
    el.scrollTop = 900;              // the card resolving reflows the transcript
    onScroll();

    expect(followingLiveEdge.value).toBe(true);
  });

  it('forgets a gesture made on a DIFFERENT transcript', () => {
    // One thread's flick may not speak for another's. The panes are mounted
    // together on mobile, so this is a real arrangement rather than a
    // hypothetical one.
    const { el, onScroll } = riding();
    const other = makeEl({ scrollTop: 0, scrollHeight: 3000 });

    readerGestureForTest(other);
    el.scrollTop = 900;
    onScroll();

    expect(followingLiveEdge.value).toBe(true);
  });
});

/**
 * **And the follow PUTS THEM BACK, which is the other half of that rule.**
 *
 * The block above pins that a platform scroll does not RETIRE the ride. That
 * alone is half a rule (ADR 0064). Every write answering something the reader
 * did NOT do runs off the ResizeObserver, and a platform scroll resizes
 * nothing. WebKit's keyboard adjust lands AFTER the last resize round by
 * construction, so the reader waited for the next GROWTH.
 *
 * These are the same movements as the block above, asserting the POSITION its
 * cases say nothing about. Three states must NOT be corrected: a gesture, an
 * armed reader parked in history, and a live-thread flick, which retires the
 * ride instead.
 */
describe('the follow puts the reader back when the PLATFORM moves them', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** An armed reader riding a live thread, WITH the anchor snapshot taken.
   *
   *  `riding()` above stops one event short of the app. Each of the glide's
   *  `scrollTop` writes fires a scroll event a frame later, and that event
   *  records the reader on the live edge. The correction is gated on that
   *  snapshot, so a test that never delivers the event tests the wrong
   *  state. */
  function ridingAndAnchored() {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000, clientHeight: 500 });
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    setThreadLive(true);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    observers.onScroll();          // the glide's own trailing event
    expect(el.scrollTop).toBe(2500);
    expect(followingLiveEdge.value).toBe(true);
    el.writes = 0;
    return { el, ...observers };
  }

  /** THE PLATFORM'S OWN SCROLL: the container somewhere nobody here wrote it,
   *  and the event that follows.
   *
   *  The wait is the load-bearing half. A scroll arriving within
   *  `NAV_SCROLL_EVENT_WINDOW_MS` of one of the app's own writes is that write's
   *  own event (`isNavigationScroll`), and the correction stands down for it.
   *  This models the opposite case. WebKit adjusts the offset through the
   *  keyboard's ~350ms animation and an app resume arrives after minutes away,
   *  neither within four frames of anything we did. */
  function platformScrollsTo(el: { scrollTop: number }, top: number, onScroll: () => void) {
    vi.advanceTimersByTime(200);
    el.scrollTop = top;
    onScroll();
  }

  it('puts them back after the iOS keyboard adjusts the offset on its own', () => {
    // The keyboard takes ~300px of viewport, which IS a resize and is handled.
    // WebKit then adjusts the offset through its ~350ms animation, with no
    // resize behind it and nothing after it.
    const { el, onScroll, onResize } = ridingAndAnchored();

    el.clientHeight = 200;
    onResize();
    expect(el.scrollTop).toBe(2800);  // the resize round still carries them

    platformScrollsTo(el, 1400, onScroll);  // and then WebKit moves them, later

    expect(el.scrollTop).toBe(2800);
    expect(followingLiveEdge.value).toBe(true);
    expect(awayFromBottom.value).toBe(false);
  });

  it('puts them back after an app resume restores an offset nobody wrote', () => {
    // Nothing observes the transcript while the app is away. The resume arrives
    // as one scroll event at a position nobody here wrote, with no resize after
    // it at all.
    const { el, onScroll } = ridingAndAnchored();

    platformScrollsTo(el, 0, onScroll);

    expect(el.scrollTop).toBe(2500);
    expect(awayFromBottom.value).toBe(false);
  });

  it('does it on an IDLE thread too, because the platform moves them either way', () => {
    // No liveness term, exactly as the box-change branch takes none. A finished
    // reply is the most likely thing to be read with the keyboard raised and
    // dismissed. Waiting for the next turn is no answer while the toggle says
    // the reader is being kept at the bottom.
    const { el, onScroll } = ridingAndAnchored();
    setThreadLive(false);

    platformScrollsTo(el, 0, onScroll);

    expect(el.scrollTop).toBe(2500);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('leaves the reader where their own GESTURE put them on an idle thread', () => {
    // The term that makes this the complement of the disarm rather than a fight
    // with it. On an idle thread a flick keeps the ride deliberately, and the
    // reader is browsing: writing them back would undo the scroll they just made
    // and make the lit toggle unusable on a finished thread.
    const { el, onScroll } = ridingAndAnchored();
    setThreadLive(false);

    readerScrollsTo(el, 900, onScroll);

    expect(el.scrollTop).toBe(900);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('keeps leaving them there when the platform moves them again afterwards', () => {
    // The at-the-edge term, which is what stops the correction reaching an armed
    // reader parked up in history. Their own scroll recorded them OFF the edge,
    // so the next platform scroll has no edge to restore them to.
    const { el, onScroll } = ridingAndAnchored();
    setThreadLive(false);
    readerScrollsTo(el, 900, onScroll);

    readerGestureForTest(null, false);  // the coast lapses: this one is the platform
    platformScrollsTo(el, 400, onScroll);

    expect(el.scrollTop).toBe(400);
  });

  it('does not undo the reader flicking away from a LIVE reply', () => {
    // The disarm wins that one, and having won it there is no follow left for
    // the correction to act on. Both halves are asserted, because a correction
    // that ran before the disarm would read identically at the flag.
    const { el, onScroll } = ridingAndAnchored();

    readerScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(false);
    expect(el.scrollTop).toBe(900);
  });

  it('writes once, and its own trailing scroll event writes nothing', () => {
    // `markHeldScroll` stamps the position read back AFTER the write. The event
    // this fires a frame later therefore reads as unmoved, and the correction
    // cannot chase itself.
    const { el, onScroll } = ridingAndAnchored();

    platformScrollsTo(el, 0, onScroll);   // the fake counts the platform's move as a write too
    expect(el.writes).toBe(2);

    onScroll();         // the correction's own event, arriving a frame later

    expect(el.writes).toBe(2);
    expect(el.scrollTop).toBe(2500);
  });

  it('moves an UNARMED reader nowhere, however the platform scrolls them', () => {
    // A position is not a request, here as everywhere else. This reader was
    // sitting exactly at the live edge and never pressed anything.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000, clientHeight: 500 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);
    atBottom(el);
    onScroll();          // records them on the edge, as a growth round would
    el.writes = 0;

    platformScrollsTo(el, 900, onScroll);

    expect(el.scrollTop).toBe(900);
    expect(el.writes).toBe(1);   // only the platform's own move
  });

  it("leaves a NAVIGATION of the app's own where it put the reader", () => {
    // The fourth term. Every navigation writes through `markNavigationScroll`:
    // `useScrollMemory` restoring a thread on open, its reset to the top for a
    // position that cannot be honoured, the up chevron, turn stepping. Each is
    // the app putting the reader somewhere, which is neither a gesture nor the
    // platform. Without the term the correction undoes them on the write's own
    // trailing event, and the navigation becomes a no-op.
    const { el, onScroll } = ridingAndAnchored();

    markNavigationScroll(el, 0);
    onScroll();

    expect(el.scrollTop).toBe(0);
    expect(followingLiveEdge.value).toBe(true);
  });

  it("leaves a REVEAL of the app's own alone, though it writes no scrollTop", () => {
    // The navigation shape that stamps nothing by itself: `choiceCardNav`'s
    // arrow-key step reveals the next option with `scrollIntoView`, so the
    // platform picks the offset and no `scrollTop` write exists to mark. The
    // keydown lands on the choice BUTTON too, so no gesture is recorded either.
    // Left unmarked it is indistinguishable from the keyboard adjusting the
    // offset, and the correction takes the stepped-to option back off screen.
    // `markRevealScroll` is what makes it a navigation like the rest.
    const { el, onScroll } = ridingAndAnchored();

    vi.advanceTimersByTime(200);   // long past any write of ours
    el.scrollTop = 900;            // the platform's reveal, offset its own choice
    markRevealScroll(el);          // and the app saying it asked for one
    onScroll();

    expect(el.scrollTop).toBe(900);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('and stops deferring to it once the window has passed', () => {
    // What keeps it a WINDOW rather than a latch. A navigation four frames ago
    // says nothing about a scroll now. If it did, the keyboard adjusting after
    // any earlier chevron tap would go uncorrected for the rest of the thread.
    const { el, onScroll } = ridingAndAnchored();

    markNavigationScroll(el, 0);
    platformScrollsTo(el, 0, onScroll);

    expect(el.scrollTop).toBe(2500);
  });
});

describe('scrolling an IDLE thread keeps the follow', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** Two acts produce an identical scroll event, and the disarm has to tell
   *  them apart. Scrolling away from a reply IN FLIGHT means "stop dragging
   *  me". Scrolling on an IDLE thread is browsing: nothing is dragging anybody,
   *  and re-reading a turn before writing the next message is no decision about
   *  how the next reply should behave. */

  function armedThenScrolledUp(live: boolean) {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    setThreadLive(live);
    // The reader goes back to re-read something, with their own hand: the
    // whole question here is what a READER's scroll means on a live thread
    // versus an idle one, so it has to be one.
    readerScrollsTo(el, 800, observers.onScroll);
    return { el, ...observers };
  }

  it('keeps the follow armed when the agent is idle', () => {
    const { el, onResize } = armedThenScrolledUp(false);

    expect(followingLiveEdge.value).toBe(true);

    setThreadLive(true);     // and the next reply carries them, as they asked
    el.scrollHeight = 4000;
    onResize();
    expect(el.scrollTop).toBe(3500);
  });

  it('retires it when the agent is LIVE, which is the unchanged half', () => {
    const { el, onResize } = armedThenScrolledUp(true);

    expect(followingLiveEdge.value).toBe(false);
    el.writes = 0;

    el.scrollHeight = 4000;
    onResize();
    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(800);
  });

  it('a submit after an idle scroll goes to the LIVE EDGE, arming still standing', () => {
    // The reader's whole case for the rule: keep the follow, browse between
    // messages, and have the next message behave as the follow says.
    const { el } = armedThenScrolledUp(false);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followAnsweredQuestion('q1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2500);       // 3000 - 500, the live edge
    expect(el.scrollTop).not.toBe(2392);   // and NOT the landing on the card
  });

  it('a submit after a LIVE scroll gets the landing instead', () => {
    const { el } = armedThenScrolledUp(true);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followAnsweredQuestion('q1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2392);       // the card's turn top, minus the clearance
  });

  it('a SUBMIT makes the thread live, so scrolling away after it still disarms', () => {
    // The gap the status cannot cover. Answering a card leaves the last turn on
    // `awaiting-answer`, which is NOT active, until the resumed status arrives
    // over SSE a round trip later. The thread reads as idle for that whole
    // window, so a reader who answered and then fled would keep their follow.
    // A submit is an act the agent is expected to answer, so it marks the
    // thread live itself.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    atBottom(el);
    setFollowLiveEdge(true);
    setThreadLive(false); // the card is parked on the reader: nothing is running

    followAnsweredQuestion('q1');
    readerScrollsTo(el, 900, onScroll); // and they scroll away before the agent picks it up

    expect(followingLiveEdge.value).toBe(false);
    el.writes = 0;

    el.scrollHeight = 9000; // the reply resumes, and does not haul them back
    onResize();
    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(900);
  });

  it('keeps that claim while the THREAD PROJECTION is still catching up', () => {
    // The live term needs the projection to agree, and the projection is the
    // slow half. The client's `meta.status` only advances when a per-event
    // aggregate carrying `running` arrives, seconds after the send. So the
    // render right after a submit writes `false` while the agent is on its way.
    // A `setThreadLive` clearing the claim on any write would destroy it in the
    // one window it exists for.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    atBottom(el);
    setFollowLiveEdge(true);
    setThreadLive(false);

    followAnsweredQuestion('q1');
    setThreadLive(false); // the lagging projection, re-rendered after the submit

    readerScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(false);
  });

  it('a submit that is never answered LETS GO of that claim', () => {
    // The claim has to expire rather than stand until contradicted, because the
    // contradiction may never come. A Continue whose POST fails, or a decision
    // the engine never answers, leaves the last turn's status as it was. So
    // `ChatExchange`'s effect never re-runs and never writes `false`. Left
    // standing, the claim would cost the reader their follow the next time they
    // browsed this idle thread.
    const nowSpy = vi.spyOn(performance, 'now');
    let clock = 1_000_000;
    nowSpy.mockImplementation(() => clock);
    try {
      const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
      const { onScroll } = makeScrollObservers(el);
      setActiveScrollElement(el);
      atBottom(el);
      setFollowLiveEdge(true);
      setThreadLive(false);   // nothing running, and nothing ever will be

      followContinuedThread(); // the POST then fails, so no status ever changes
      clock += 30_000;         // long past the claim

      readerScrollsTo(el, 900, onScroll); // the reader browses the still-idle thread

      expect(followingLiveEdge.value).toBe(true);
    } finally {
      nowSpy.mockRestore();
    }
  });
});

describe('an IDLE thread moves an armed reader nowhere', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** The other half of the block above: the WRITE asks whether the agent is
   *  live, exactly as the disarm does. Without it, a reader who scrolled an idle
   *  thread keeps the ride, then is written back to the live edge by the next
   *  height change. On an idle thread that is the transcript finishing its own
   *  rendering: markdown settling, an image decoding, a card mounting.
   *
   *  ARMED and CARRYING are the two states this pins apart (ADR 0064). Nothing
   *  here retires anything: the toggle stays lit and the ride resumes on its
   *  own. */

  function armedThenScrolledUpOnAnIdleThread() {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);   // the toggle's glide settles on the live edge
    setThreadLive(false);           // the reply finishes
    readerScrollsTo(el, 800, observers.onScroll); // and the reader goes back to re-read it
    expect(followingLiveEdge.value).toBe(true);   // the ride survives, per the block above
    el.writes = 0;
    return { el, ...observers };
  }

  it('leaves the reader where they scrolled when the transcript grows', () => {
    const { el, onResize } = armedThenScrolledUpOnAnIdleThread();

    el.scrollHeight = 4000;
    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(800);
  });

  it('leaves them alone across round after round of it', () => {
    // One round could pass on a stale stamp or a coincidence. A finished reply
    // settles over several frames, and every one of them must answer the same.
    const { el, onResize } = armedThenScrolledUpOnAnIdleThread();

    for (const height of [3400, 4000, 9000]) {
      el.scrollHeight = height;
      onResize();
      expect(el.scrollTop).toBe(800);
    }
    expect(el.writes).toBe(0);
  });

  it('still shows them the chevron, so they can go back themselves', () => {
    // Standing down from the WRITE is not standing down from the signals. The
    // reader is off the live edge and has to be told, or their one way back
    // goes with the ride.
    const { el, onResize } = armedThenScrolledUpOnAnIdleThread();

    el.scrollHeight = 4000;
    onResize();

    expect(awayFromBottom.value).toBe(true);
  });

  it('carries them again the moment the agent starts', () => {
    // Armed is armed. The idle spell suspends the writing, it does not end the
    // request, and no second press is needed.
    const { el, onResize } = armedThenScrolledUpOnAnIdleThread();

    el.scrollHeight = 4000;
    onResize();
    expect(el.scrollTop).toBe(800);

    setThreadLive(true);
    el.scrollHeight = 5000;
    onResize();
    expect(el.scrollTop).toBe(4500);
  });

  it('carries them on the WAKE itself, without waiting for a second resize', () => {
    // The order the two signals arrive in. The mutation that wakes a thread
    // fires the ResizeObserver inside its own frame. `ChatExchange` publishes
    // the new status from a Preact effect, deferred to a task AFTER that frame.
    // So `honourGrowth` sees the waking resize while this module still reads
    // idle.
    //
    // Modelled that way: the growth and its resize land FIRST, the liveness
    // after. A streaming reply hides this by resizing again a moment later. A
    // coding-agent turn resuming its subprocess does not, and sits on its
    // mounted row for fifteen to twenty seconds.
    const { el, onResize } = armedThenScrolledUpOnAnIdleThread();

    el.scrollHeight = 4000;  // the waking turn's row mounts
    onResize();              // and its resize is delivered while we still read idle
    expect(el.scrollTop).toBe(800);

    setThreadLive(true);     // the effect lands a task later, with no resize behind it

    expect(el.scrollTop).toBe(3500);
  });

  it('acts on the EDGE only, so a repeated live signal writes nothing', () => {
    // Only a WAKE describes new content. `ChatExchange` re-runs its effect
    // whenever its derived liveness changes, and a `true` repeating what the
    // module already knew is not a second turn arriving. Pinned from the armed
    // side, where both answers are visible. The first `true` carries the
    // reader. A second must move them zero pixels, or every later render
    // re-asserts the live edge over wherever they had got to.
    const { el, onScroll } = armedThenScrolledUpOnAnIdleThread();

    setThreadLive(true);        // the wake: the round the observer missed
    expect(el.scrollTop).toBe(2500);

    // Now put the container somewhere the follow did not, with NO gesture
    // behind it: the iOS keyboard / app-resume case the follow survives. The
    // setup's own scroll is retired first, or its coast would still count as
    // the reader's. Armed, live, and off the stamp is the state a re-asserting
    // signal would trample.
    readerGestureForTest(null, false);
    el.scrollTop = 1200;
    onScroll();
    expect(followingLiveEdge.value).toBe(true);
    el.writes = 0;

    setThreadLive(true);        // the same answer again, from a later render

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(1200);
  });

  it('carries them for a SUBMIT before any status says the thread is live', () => {
    // The gap the submit's own claim covers, from this side too. The write runs
    // through the seconds before the projection catches up, or the reader
    // watches the reply grow in below them.
    const { el, onResize } = armedThenScrolledUpOnAnIdleThread();

    followContinuedThread();  // marks the thread live by itself
    vi.advanceTimersByTime(1500);
    el.scrollHeight = 4000;
    onResize();

    expect(el.scrollTop).toBe(3500);
  });
});

describe('an IDLE thread keeps an armed reader who never left the edge', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** The block above's other half. Those tests are all one reader: an armed one
   *  who SCROLLED UP on a quiet thread and must be left where they parked. This
   *  block is the reader who never moved, and a rider who has not moved is
   *  exactly who the lit toggle is for.
   *
   *  A thread PARKS the instant the agent asks a question:
   *  `waiting_for_user_answer` is quiescent, so `exchangeMarksThreadLive` says
   *  idle. The card therefore arrives on a thread this module reads as doing
   *  nothing. Standing the write down for every armed reader mounts that card
   *  UNDER a rider on the live edge, with its options below the fold.
   *
   *  WHERE THE READER IS decides, never what kind of resize it was (ADR 0064).
   *  See `keepTheLiveEdge`, whose box-change and platform-scroll twins are in
   *  `scroll-reflow-anchor.test.ts`. */

  function armedAtTheEdgeOnAnIdleThread() {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    atBottom(el);              // 2500
    observers.onScroll();      // the transcript records them ON the edge
    setFollowLiveEdge(true);   // arming from the edge writes nothing, runs no tween
    setThreadLive(false);      // and the thread parks on the question it just asked
    expect(followingLiveEdge.value).toBe(true);
    el.writes = 0;
    return { el, ...observers };
  }

  it('keeps them on the edge when a question card mounts under them', () => {
    const { el, onResize } = armedAtTheEdgeOnAnIdleThread();

    el.scrollHeight = 3400;  // the card, its options and their descriptions
    onResize();

    expect(el.scrollTop).toBe(2900);
    expect(awayFromBottom.value).toBe(false);
  });

  it('keeps them there across round after round of it', () => {
    // A card does not arrive in one round: the panel mounts, the options lay
    // out, the composer swaps into answer mode. Every round has to answer the
    // same, or the reader ends up short of the edge by whichever one did not.
    const { el, onResize } = armedAtTheEdgeOnAnIdleThread();

    for (const [height, edge] of [[3400, 2900], [3600, 3100], [4000, 3500]]) {
      el.scrollHeight = height;
      onResize();
      expect(el.scrollTop).toBe(edge);
    }
    expect(awayFromBottom.value).toBe(false);
  });

  it('does it in the resize round itself, so nothing is painted short of the edge', () => {
    // Why this is not merely a position assertion. The ResizeObserver runs
    // after layout and before paint, so a write from `onResize` is invisible.
    // A rescue running from a Preact effect a task later is a painted frame at
    // the wrong place followed by a jump. Nothing may be needed after the
    // resize returns.
    const { el, onResize } = armedAtTheEdgeOnAnIdleThread();

    el.scrollHeight = 3400;
    onResize();

    expect(el.scrollTop).toBe(el.scrollHeight - el.clientHeight);
    expect(el.writes).toBe(1);
  });

  it('needs no wake, and a later wake then moves them nowhere', () => {
    // Answering is what wakes the thread, and by then the reader is already on
    // the edge, so `honourWake`'s replay has nothing to do. It must not be the
    // thing that gets them there: that replay is a task late by construction.
    const { el, onResize } = armedAtTheEdgeOnAnIdleThread();

    el.scrollHeight = 3400;
    onResize();
    expect(el.scrollTop).toBe(2900);
    el.writes = 0;

    setThreadLive(true);

    expect(el.scrollTop).toBe(2900);
  });

  it('lets go the moment they scroll off the edge, and keeps the ride', () => {
    // The line between this block and the one above it, walked in one test. The
    // same reader, one flick apart: on the edge they are held there, off it they
    // are left alone. Neither loses the lit toggle.
    const { el, onScroll, onResize } = armedAtTheEdgeOnAnIdleThread();

    el.scrollHeight = 3400;
    onResize();
    expect(el.scrollTop).toBe(2900);

    readerScrollsTo(el, 1500, onScroll);
    el.writes = 0;

    el.scrollHeight = 4000;
    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(1500);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('defers to an anchor correction that moved them off the edge first', () => {
    // A REVEAL is growth too, so the growth branch runs for one and must not
    // undo `withScrollAnchor`'s correction. Expanding a turn's steps grows every
    // turn ABOVE the reader. The correction holds them on the content they were
    // reading, which takes them off the live edge.
    //
    // Nothing here knows about reveals. The correction WRITES the container, and
    // its scroll event is dispatched before the resize one in the same frame.
    // So `recordAnchor` has already answered "off the edge" when growth asks.
    //
    // The correction ANNOUNCES itself through `honourAnchoredMutation`, which
    // carries the held stamp. Without that, a gesture-less scroll reads as the
    // platform moving the reader, and `keepTheLiveEdge` writes them straight
    // back to the edge. Modelled with no gesture on purpose: the ride has to
    // survive a correction the app made.
    const { el, onScroll, onResize } = armedAtTheEdgeOnAnIdleThread();

    el.scrollHeight = 5000;   // the steps unfold, above the reader and below
    el.scrollTop = 3700;      // and the correction holds them on their own turn
    honourAnchoredMutation(el);
    onScroll();
    el.writes = 0;

    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(3700);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('moves an UNARMED reader on the edge zero pixels under the same growth', () => {
    // Being at the bottom is a position, not a request. The whole of
    // `scroll-resize-never-follows.test.ts` says so; it is repeated here because
    // this block is the one that could take it away.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    atBottom(el);
    onScroll();
    setThreadLive(false);
    el.writes = 0;

    el.scrollHeight = 3400;
    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2500);
    expect(awayFromBottom.value).toBe(true);
  });
});

describe('the toggle remembers the last press as a seed', () => {
  /** The seed is what a thread with NO reading position starts as, and the only
   *  state the toggle can show in the transcript-less compose view. It must
   *  never mirror the follow. A scroll retiring the ride on THIS thread is not
   *  the reader revising a standing preference. One accidental flick would
   *  otherwise turn it off for every future thread. */
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); setFollowLiveEdge(false); });

  it('records both edges of the press', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);

    setFollowLiveEdge(true);
    expect(followLiveEdgeSeed.value).toBe(true);

    setFollowLiveEdge(false);
    expect(followLiveEdgeSeed.value).toBe(false);
  });

  it('records the press even where there is no transcript to arm', () => {
    // The compose view. `setFollowLiveEdge` finds no container and arms nothing,
    // and the press still has to be remembered: that is the whole point of the
    // button being there.
    setActiveScrollElement(null);

    setFollowLiveEdge(true);

    expect(followLiveEdgeSeed.value).toBe(true);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('is NOT touched when the reader scrolls the follow away', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);

    readerScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(false); // the ride ended here
    expect(followLiveEdgeSeed.value).toBe(true); // the preference did not
  });

  it('is NOT touched by the retire a thread switch makes', () => {
    // `focusThread` retires the follow on every open. Reading that as the reader
    // changing their mind would clear the seed on the way into the very thread
    // it is meant to seed.
    setActiveScrollElement(null);
    setFollowLiveEdge(true);

    stopFollowingBottom();

    expect(followLiveEdgeSeed.value).toBe(true);
  });
});

describe('a reveal inside the transcript never retires the follow', () => {
  /** ONLY A SCROLL MAY DISARM. A collapse, an unfold and the two turn controls
   *  are clicks on a control. The reader who made one asked for more of the
   *  turn, not for less of the ride.
   *
   *  They move the container all the same. `withScrollAnchor` pins the turn the
   *  click was on and writes the correction itself. The turn controls are
   *  transcript-wide, so every turn BELOW the anchored one grows too. The
   *  correction then lands the reader short of the live edge, which reads
   *  exactly like the reader scrolling away.
   *
   *  The fake reproduces the shape rather than the DOM. `honourAnchoredMutation`
   *  is the entry point `withScrollAnchor` calls right after its own write, and
   *  that write is modelled by moving `_scrollTop` without counting it. */
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** Arm the follow, park at the live edge, then reveal: the transcript grows by
   *  `above + below` and the anchor correction writes `above` of it. */
  function armAndReveal(el: any, above: number, below: number) {
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    el.scrollHeight += above + below;
    el._scrollTop += above;
    honourAnchoredMutation(el);
  }

  it('keeps the follow armed when a reveal leaves the reader short of the live edge', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);

    armAndReveal(el, 500, 1500);
    onScroll(); // the correction's own scroll event, a frame later

    expect(followingLiveEdge.value).toBe(true);
  });

  it('puts an armed reader ON the live edge at once, not over a tween', () => {
    // The write lands in the same frame the caller unfreezes, before the paint
    // the mutation causes, so the newest content stays exactly where it was.
    // See `honourAnchoredMutation`. The rejected tween is in ADR 0064.
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);

    armAndReveal(el, 500, 1500);
    expect(el.scrollTop).toBe(4500); // the new live edge, with no frame in between
    const writesOnLanding = el.writes;

    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(4500);
    expect(el.writes).toBe(writesOnLanding); // and nothing eased in behind it
  });

  it('moves an UNARMED reader zero pixels, whatever the reveal did', () => {
    // The mirror, and the reason the glide is gated on the follow: a toggle is a
    // disclosure, so a reader who never asked to ride is left where the anchor
    // correction already put them.
    const el = makeEl({ scrollTop: 1000, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);

    el.scrollHeight = 5000;
    const writesBefore = el.writes;
    honourAnchoredMutation(el);
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(writesBefore);
    expect(el.scrollTop).toBe(1000);
  });

  it('still lets the reader scroll away afterwards', () => {
    // The carry must not blanket-disable the disarm: it says THIS write was ours,
    // not that nothing can retire the follow any more.
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);

    armAndReveal(el, 500, 1500);  // which lands them on the live edge
    vi.advanceTimersByTime(1500);

    readerScrollsTo(el, 900, onScroll); // and then they flick up

    expect(followingLiveEdge.value).toBe(false);
  });
});

describe('coming back to a thread resumes the follow it was left with', () => {
  /** The follow is one global that `focusThread` retires on every open. So the
   *  request is recorded per thread as a reading position
   *  (`hooks/useScrollMemory.ts`), and this is the entry point that resumes it.
   *  That file pins the RECORDING. This pins that resuming produces a real
   *  follow rather than a one-shot landing at the bottom. */
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('lands on the live edge the thread has NOW, and keeps riding it', () => {
    // `.thread-content` is one element reused across threads, so on arrival it
    // holds the OUTGOING thread's offset. Arming without writing would leave
    // the reader sitting there until the next growth round.
    const el = makeEl({ scrollTop: 300, scrollHeight: 20000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    resumeFollowingBottom(el);
    expect(el.scrollTop).toBe(19500);

    for (const [height, expected] of [[21000, 20500], [26000, 25500]]) {
      el.scrollHeight = height;
      onResize();
      expect(el.scrollTop).toBe(expected);
    }
    expect(awayFromBottom.value).toBe(false);
  });

  it('writes as the app, so the mobile header and the render window stand down', () => {
    // Opening a thread at the top of the rendered slice is inside the window
    // expansion's margin by construction. So is any write the reader did not
    // make with their finger. The restore goes through `markFollowScroll`:
    // `markNavigationScroll` plus the follow's own position stamp.
    const el = makeEl({ scrollTop: 300, scrollHeight: 20000 });
    setActiveScrollElement(el);

    resumeFollowingBottom(el);

    expect(isNavigationScroll(el)).toBe(true);
    expect(isFollowScroll(el)).toBe(true);
  });

  it('is retired by the reader exactly as a freshly armed one is', () => {
    // A resumed follow is the same request, so it gets no special protection:
    // the first gesture that takes the container away from it ends it.
    const el = makeEl({ scrollTop: 300, scrollHeight: 20000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    resumeFollowingBottom(el);
    readerScrollsTo(el, 8000, onScroll);

    el.scrollHeight = 26000;
    onResize();
    expect(el.scrollTop).toBe(8000);
  });
});

describe('resuming IN PLACE, when a deep link owns the position', () => {
  /** A deep link owns where the reader is looking on the open it caused; it does
   *  not own what the thread asked for. The two answers coexist, so the resume
   *  has a second placement that arms and writes nothing. Which OPENS take it is
   *  `hooks/useScrollMemory.ts`'s question, pinned there; what this pins is the
   *  placement itself. */
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('arms without moving the reader a pixel, and rides the next growth', () => {
    // The reader stays on the event the link took them to. The toggle is lit,
    // so the thread waking up carries them.
    const el = makeEl({ scrollTop: 6000, scrollHeight: 20000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    setThreadLive(false); // the thread the link landed on is parked on a question
    el.writes = 0;

    resumeFollowingBottom(el, 'in-place');

    expect(followingLiveEdge.value).toBe(true);
    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(6000);

    // Idle growth (markdown settling, the card mounting) still moves nobody.
    el.scrollHeight = 21000;
    onResize();
    expect(el.scrollTop).toBe(6000);

    // The thread wakes: the same armed request picks them back up.
    setThreadLive(true);
    expect(el.scrollTop).toBe(20500);
  });

  it('is what a LIVE thread gets too: the agent decides nothing here', () => {
    // A landing off the live edge does end the ride, but liveness is the wrong
    // way to ask whether that happened. The guard is
    // `deepLinkLandedOffLiveEdge()`, so the branch reads no `_threadLive` at
    // all: same call, live thread, same result as the idle one above.
    //
    // The landed-link guard itself is pinned in `hooks/useScrollMemory.test.ts`
    // ("a deep-link claiming the open mid-restore"), which has the DOM harness
    // to resolve a real link. This file is DOM-free, so faking a resolve here
    // would need a seam letting a test claim a landing no link made.
    const el = makeEl({ scrollTop: 6000, scrollHeight: 20000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    setThreadLive(true);

    resumeFollowingBottom(el, 'in-place');

    expect(followingLiveEdge.value).toBe(true);
    expect(el.scrollTop).toBe(6000);
    // And being live, the very next growth carries them: the ride is real, not
    // a lit toggle over nothing.
    el.scrollHeight = 21000;
    onResize();
    expect(el.scrollTop).toBe(20500);
  });

  it('leaves an ALREADY ARMED follow and its held stamp alone', () => {
    // A link into the thread the reader is already in retires nothing, so there
    // is no request to resume. Re-arming would clear the stamp `isFollowScroll`
    // reads. That stamp decides whether this thread's reading position is
    // recorded as the live edge or as an offset.
    const el = makeEl({ scrollTop: 100, scrollHeight: 20000 });
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    expect(isFollowScroll(el)).toBe(true);
    setThreadLive(false);

    resumeFollowingBottom(el, 'in-place');

    expect(followingLiveEdge.value).toBe(true);
    expect(isFollowScroll(el)).toBe(true);
  });

  it('takes no stamp of its own, so the landing is recorded as an offset', () => {
    // The reader is wherever the LINK put them, which is not a position the
    // follow wrote. Recording it as the live edge would send them to the bottom
    // on re-entry instead of back to the event they went to.
    const el = makeEl({ scrollTop: 6000, scrollHeight: 20000 });
    setActiveScrollElement(el);
    setThreadLive(false);

    resumeFollowingBottom(el, 'in-place');

    expect(followingLiveEdge.value).toBe(true);
    expect(isFollowScroll(el)).toBe(false);
  });
});

describe('sending a message lands the turn at the top of the viewport', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('lands the reader who sent FROM the live edge, once their turn renders', () => {
    // The ordinary case: most sends are made from the bottom. Being at the live
    // edge WHEN THE SUBMIT IS MADE says nothing about where the turn will sit,
    // because the submit is what appends it.
    //
    // The fixture's ordering is the whole point: park at the live edge, submit,
    // and only THEN grow the transcript with the turn.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const parked = atBottom(el);
    el.writes = 0;

    followSentMessage();
    expect(el.writes).toBe(0);      // nothing yet: the turn does not exist
    expect(el.scrollTop).toBe(parked);

    el.addUserMessage({ top: 2900, height: 120 }); // the row and its status line
    el.scrollHeight = 3400;                        // with a screenful under it
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2892); // 2900 (the row's top) - the clearance
    expect(el.scrollTop).toBeGreaterThan(parked);
  });

  it('still writes nothing when that turn is already ON the landing line', () => {
    // A reader whose new turn already sits at the top of the viewport has
    // nowhere to go. Reached by measuring the target rather than predicting it,
    // which is the difference that makes the case above work.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const parked = atBottom(el);
    el.writes = 0;

    followSentMessage();
    // Exactly on the line: parked at 2500, so a row at 2508 is `LANDING_CLEARANCE`
    // below the container top, which is where the landing would put it.
    el.addUserMessage({ top: parked + LANDING_CLEARANCE, height: 20 });
    el.scrollHeight = 3000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
  });

  it('arms nothing, so the reply streaming in leaves that reader where they are', () => {
    // Riding is the follow toggle's request, not the send's (ADR 0064). The
    // reader who sent from the live edge is left at the bottom of what they can
    // see, and the chevron is their way down.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const parked = atBottom(el);

    followSentMessage();
    el.writes = 0;

    el.scrollHeight = 4000;
    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
    expect(awayFromBottom.value).toBe(true);
  });

  it('moves nobody while the just-sent message has not rendered yet', () => {
    // The composer collapsing fires a resize of its own before the optimistic
    // row arrives. Jumping to the bottom on it would be the blind jump the
    // landing exists to avoid.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.writes = 0;

    el.clientHeight = 520; // the composer gave the transcript its height back
    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(500);
  });

  it('glides so the message is the TOP thing in the viewport', () => {
    // What the whole screen under the message is for: the agent status line one
    // row down says the turn was taken, and the reply then streams into the
    // space rather than into the fold.
    //
    // Anchored on an ELEMENT. A scrollHeight target would land at 2900 here,
    // past the whole turn, hiding the very thing they wrote.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();

    el.addUserMessage({ top: 2900, height: 120 }); // the optimistic row renders
    el.scrollHeight = 3400;                        // with the working indicator under it
    onResize();
    expect(el.scrollTop).toBe(500); // a glide, not a jump: nothing lands this frame

    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2892);                      // 2900 (the row's top) - the clearance
    expect(el.scrollTop).not.toBe(2548);                  // NOT the old status-line-on-the-bottom answer
    expect(2900 - el.scrollTop).toBe(LANDING_CLEARANCE);  // the row is the top thing on screen
    expect(3048).toBeLessThanOrEqual(el.scrollTop + el.clientHeight); // the status line is in view under it
  });

  it('lands as soon as the ROW renders, without waiting for the response panel', () => {
    // The response panel can mount a commit or two after the row it belongs to.
    // A landing that waited for the status line stopped one row short.
    //
    // The target is the turn's own TOP, fixed the instant the row renders, so
    // there is nothing to wait for. The status line arrives into the viewport
    // underneath, wherever in the sequence it turns up.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();

    const sent = el.addUserMessage({ top: 2900, height: 120, status: false });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2892); // landed on the row alone

    el.mountStatusLine(sent);        // the response panel follows a commit later
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2892); // and does not move the reader again
    expect(3048).toBeLessThanOrEqual(el.scrollTop + el.clientHeight);
  });

  it('a QUEUED follow-up is only REVEALED, never chased to the live edge', () => {
    // A second message fired while the first reply runs lands as the newest
    // turn. Nothing sits under it but the transcript's own bottom padding, so
    // the landing line is out of reach.
    //
    // The ask changes rather than being clamped. Asking for the line anyway
    // drags the reader to the live edge for a turn that cannot reach the top,
    // gaining nothing. The modest ask is to SHOW the turn, which stops short of
    // the live edge by the trailing room the transcript reserves.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();

    el.addUserMessage({ top: 2900, height: 120, status: false }); // the queued bubble
    // Its own bottom is 3020: no room reserved under a queued turn, just the
    // transcript's own bottom padding past it.
    el.scrollHeight = 3080;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2520);                    // 3020 (the bubble's bottom) - the viewport
    expect(el.scrollTop).toBeLessThan(2892);            // short of the landing line, out of reach
    expect(el.scrollTop).toBeLessThan(el.scrollHeight - el.clientHeight); // and short of the live edge
    expect(3020).toBe(el.scrollTop + el.clientHeight);  // the bubble is fully on screen
    expect(el.scrollTop).toBeLessThan(2900);            // with the running reply above it
  });

  it('still lands on the line when the container falls a rounded pixel short', () => {
    // At the BOUNDARY a turn is within a rounding error of a screenful. Both
    // sides of "can the container reach the line" are then the same number.
    // `scrollHeight` is an integer while the turn's own top is fractional, so an
    // exact test flips there for reasons no reader can see. Flipping is not a
    // small difference: the other branch lands the turn a whole padding-bottom
    // lower. `LANDING_REACH_SLACK_PX` absorbs it.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 2900, height: 470 });
    // One pixel SHORT of what the landing needs: the line is 2892 and this puts
    // the furthest the container goes at 2891.
    el.scrollHeight = 3391;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2891);         // the line, as far as the container allows
    expect(el.scrollTop).toBeGreaterThan(2870); // and NOT the reveal branch's answer
  });

  it('moves NOBODY for a queued follow-up that is already on screen', () => {
    // The point of asking to REVEAL rather than to land. The first message rests
    // on the landing line and its reply fills less than a screen. The queued
    // bubble renders in the room left over and is already visible. Nothing is
    // owed, so nothing is written, and the first message stays at the top.
    const el = makeEl({ scrollTop: 2000, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.writes = 0;

    // Viewport 2000..2500, and the bubble lands inside it.
    el.addUserMessage({ top: 2300, height: 120, status: false });
    el.scrollHeight = 2480;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2000);
  });

  it('a lapsed landing does not block the NEXT submit', () => {
    // The deadline is wall-clock and only the growth branch checks it. So a
    // landing whose turn never becomes addressable sits on `_pendingLanding`
    // for as long as nothing grows. The reader's next submit must not then
    // return early without installing its own resolver.
    //
    // The turn that never answers is the second and later queued follow-up,
    // which folds into a CLOSED disclosure group and has no box. The second
    // submit is a CARD, resolving at submit time and gliding immediately. It
    // never reaches the growth branch, so a stale pending landing is the only
    // thing that can stop it. A second SEND would be rescued by luck: the stale
    // resolver is `awaitsNewTurn(lastUserMessage)`, which the newer row
    // satisfies.
    const nowSpy = vi.spyOn(performance, 'now');
    let clock = 1_000_000;
    nowSpy.mockImplementation(() => clock);
    try {
      const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
      makeScrollObservers(el);
      setActiveScrollElement(el);
      el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

      followSentMessage(); // goes pending: its own turn has not rendered
      clock += 5000;       // the deadline passes with nothing growing

      followAnsweredQuestion('q1');
      vi.advanceTimersByTime(1500);

      expect(el.scrollTop).toBe(2392); // 2400 (the card's turn top) - the clearance
    } finally {
      nowSpy.mockRestore();
    }
  });

  it('stops clear of the chrome at the top, by the clearance the turn asks for', () => {
    // The transcript's top edge is not clear space: the desktop fade band and
    // the floating up-chevron sit over it, and on mobile the app header and the
    // sticky thread-title row do. A turn landed flush against that edge would
    // be half under them. The turn names the room it needs as
    // `scroll-margin-top` (chat/response.css), the SAME line a deep link and
    // ⌘↑/⌘↓ turn stepping rest on. The landing reads the resolved px.
    const styles = vi.fn(() => ({ scrollMarginTop: '24px' }));
    (globalThis as any).getComputedStyle = styles;
    try {
      const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
      const { onResize } = makeScrollObservers(el);
      setActiveScrollElement(el);

      followSentMessage();
      el.addUserMessage({ top: 2900, height: 120 });
      el.scrollHeight = 3400;
      onResize();
      vi.advanceTimersByTime(1500);

      expect(el.scrollTop).toBe(2876); // 2900 (the row's top) - the 24px it asked for
    } finally {
      delete (globalThis as any).getComputedStyle;
    }
  });

  it('holds that landing when the transcript grows again mid-glide', () => {
    // The target is re-read per frame off the message, so a working indicator
    // mounting under it during the glide cannot drag the landing past it.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();

    vi.advanceTimersByTime(100);
    el.scrollHeight = 6000; // the reply starts streaming while the glide runs
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2892);
  });

  it('then holds still, so the reply grows in UNDER the turn it landed', () => {
    // The landing is the WHOLE reaction, not the first half of a ride: the turn
    // stays exactly where the glide left it and the answer arrives beneath it.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2892);
    el.writes = 0;

    for (const height of [5000, 9000]) {
      el.scrollHeight = height;
      onResize();
    }

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2892);
  });

  it('leaves a reader who scrolls away after the landing entirely alone', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    readerScrollsTo(el, 1800, onScroll); // the reader goes back to read something
    const parked = el.scrollTop;
    el.writes = 0;

    for (const height of [4000, 5000, 9000]) {
      el.scrollHeight = height;
      onResize();
    }

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
  });

  it('stops the glide the moment the reader scrolls, rather than dragging them back', () => {
    // The cancel and the tween must not disagree. A submit arms nothing, so the
    // landing carries its own position stamp and its own cancel rather than
    // taking them from the follow's disarm. A chevron tap's tween still reaches
    // its target.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(60);
    expect(el.scrollTop).toBeGreaterThan(500);

    readerScrollsTo(el, 800, onScroll); // the reader takes over mid-glide
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(800);
  });

  it('is cancelled by a gesture made BEFORE the message renders, so it never runs', () => {
    // The pending phase has written nothing, so there is no write to read the
    // gesture against. The stamp the submit takes at call time is what lets a
    // flick in that window cancel the landing.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    readerScrollsTo(el, 900, onScroll); // the reader scrolls on while the row is still rendering
    el.writes = 0;

    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(900);
  });

  it('holds the glide on its last measured target when the turn leaves the layout', () => {
    // Nothing cancels a tween when the reader opens another thread mid-glide,
    // and a detached node reports an all-zero rect. Against the container's own
    // top that reads as a target ABOVE the reader, so the glide would turn
    // round and haul the newly-opened thread backwards.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    const sent = el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();

    vi.advanceTimersByTime(60);
    const midGlide = el.scrollTop;
    expect(midGlide).toBeGreaterThan(500);
    sent.turn.isConnected = false; // the reader opens another thread

    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBeGreaterThanOrEqual(midGlide);
    expect(el.scrollTop).toBeLessThanOrEqual(2892);
  });

  it('LAPSES and moves nobody when the message has no box to land on', () => {
    // The second and later queued follow-ups fold into a CLOSED disclosure
    // group, so the message the reader just sent has no rect. A submit arms
    // nothing, so past the deadline there is no follow to honour, and a turn
    // with no box is nothing to show them. The landing lapses (ADR 0064).
    const nowSpy = vi.spyOn(performance, 'now');
    let clock = 1_000_000;
    nowSpy.mockImplementation(() => clock);
    try {
      const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
      const { onResize } = makeScrollObservers(el);
      setActiveScrollElement(el);

      followSentMessage();
      el.writes = 0;
      el.addUserMessage({ top: 2900, height: 120, visible: false });
      el.scrollHeight = 3400;
      onResize();
      expect(el.scrollTop).toBe(500); // inside the deadline: still waiting for it

      clock += 1500; // past LANDING_DEADLINE_MS
      el.scrollHeight = 3800;
      onResize();
      el.scrollHeight = 9000; // and nothing later revives it either
      onResize();
      vi.advanceTimersByTime(1500);

      expect(el.writes).toBe(0);
      expect(el.scrollTop).toBe(500);
    } finally {
      nowSpy.mockRestore();
    }
  });

  it('is not cancelled by the reflow correction while the landing is still pending', () => {
    // A width change re-wraps the transcript and the anchor correction writes
    // scrollTop to hold the reader on the same content. That is the app, not
    // the reader. The growth branch stands down for the pending landing, so
    // nothing else re-stamps the position the disarm compares against.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const child = el.addAnchorChild(-100);

    followSentMessage();
    onScroll();            // take the reflow anchor

    el.clientWidth = 700;  // the pane is narrowed, and the content re-wraps
    child._top = -160;
    onResize();
    onScroll();            // the correction's own scroll event
    expect(el.scrollTop).toBe(440);

    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2892); // the landing still happened
  });

  it('a second call for the same send is a no-op refresh, not a second landing', () => {
    // PromptInput.submit and addPendingMessage both fire for one composer send.
    // The second keeps the first's baseline, so a render landing between them
    // cannot leave the landing waiting for a message that is already there.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 2900, height: 120 }); // the row renders between the two
    followSentMessage();

    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2892);
  });
});

describe('a send moves nobody only when the turn is already at the landing line', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** The block above is the send earning its keep. This block is the case where
   *  the ask is already satisfied, and the point is WHERE that is decided.
   *
   *  There is ONE test and it is a measurement. `landOnOwnTurn` writes nothing
   *  when its target is at or behind the current position, asked per frame
   *  against the turn itself. Every case below reaches "moves nobody" through
   *  that measurement rather than by being recognised in advance. Why a
   *  pre-emptive test cannot answer it: ADR 0064. */

  it('leaves the reader alone when their turn renders AT the top already', () => {
    // The order the two call sites fire in for one send. `PromptInput.submit`
    // runs first, from the compose view, where there is no transcript to
    // resolve. `addPendingMessage` runs second, by which time the promoted
    // thread has mounted an EMPTY one: it calls before writing `threadMap`, so
    // the optimistic row has not rendered.
    //
    // A brand-new thread's first turn IS the top of the transcript, so there is
    // nowhere to take anybody. The whole first reply then streams in below
    // without moving them either.
    followSentMessage();

    const el = makeEl({ scrollTop: 0, scrollHeight: 400, turns: 0 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    followSentMessage();
    el.writes = 0;

    el.addUserMessage({ top: 0, height: 120 }); // the optimistic row, at the top
    el.scrollHeight = 600;
    onResize();
    for (const height of [900, 3000, 9000]) {   // and the reply streams in
      el.scrollHeight = height;
      onResize();
    }
    vi.advanceTimersByTime(1500);               // no glide was ever scheduled

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(0);
    expect(awayFromBottom.value).toBe(true); // and the chevron is their way down
  });

  it('lands the FIRST turn of a brand-new thread when there is content above it', () => {
    // The compose view renders the welcome message inside the same
    // `.thread-content`, and on a short viewport it has real scroll room. A
    // brand-new thread's first turn renders below that welcome content exactly
    // as any other turn renders below the conversation above it. It is owed the
    // same landing.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000, turns: 0 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 3100, height: 120 }); // the optimistic row and its
    el.scrollHeight = 3700;                        // Requesting row, with room past it
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(3092); // 3100 (the row's top) - the clearance

    // And it arms nothing on the way, so the reply that follows moves nobody.
    el.writes = 0;
    el.scrollHeight = 6000;
    onResize();
    expect(el.writes).toBe(0);
  });

  it('leaves the reader alone in an existing thread that fits on screen', () => {
    // Why the turn test cannot answer it either. There is a conversation here,
    // but the reader is at its bottom because there is nowhere else to be.
    // What they just sent is already fully visible, and the reply grows past
    // the fold below them.
    const el = makeEl({ scrollTop: 0, scrollHeight: 400 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.writes = 0;
    el.scrollHeight = 4000;
    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(0);
  });

  it('does not retire a follow the reader had already armed', () => {
    // Declining to arm is not disarming. A rider's standing request is theirs
    // until they take it back, and a send is not them taking it back, whatever
    // the transcript's geometry.
    const el = makeEl({ scrollTop: 0, scrollHeight: 400 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    resumeFollowingBottom(el); // they were riding this thread when they left it

    followSentMessage();

    el.scrollHeight = 4000;
    onResize();
    expect(el.scrollTop).toBe(3500);
  });
});

describe('answering a question card lands the same way', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** Submitting an answer IS a send: the reader handed the agent something and
   *  is owed the sight of it being picked up. Which shape they used must not be
   *  something they can feel in the scroll. So this block is the send's block
   *  above with the card in place of the message. Typing the answer is
   *  literally a send and rides `followSentMessage`.
   *
   *  Its mirror is the card ARRIVING, which is the agent's doing and moves
   *  nobody. That case lives in the `unmovedBy` block at the bottom of this
   *  file, beside the other growths that are not requests. */

  it('writes NO scroll when the reader is already at the live edge', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    const parked = atBottom(el);
    el.writes = 0;

    followAnsweredQuestion('q1');

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
  });

  it('arms nothing, so the agent resuming underneath moves that reader 0px', () => {
    // Answering arms nothing, so the resumed reply does not carry the reader
    // down through it (ADR 0064).
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    const parked = atBottom(el);

    followAnsweredQuestion('q1');
    el.writes = 0;

    el.scrollHeight = 4000;
    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
  });

  it('glides so the answered card is the top thing in the viewport', () => {
    // The same landing a send gets, for the same reason: the reader is owed the
    // whole screen under what they answered, for the agent picking it up again.
    // Anchored on the card's TURN, as a send is anchored on its message's turn.
    // A scrollHeight target would land at 2500 here, past the whole turn.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followAnsweredQuestion('q1');
    expect(el.scrollTop).toBe(500); // a glide, not a jump

    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2392);           // 2400 (the turn top) - the clearance
    expect(el.scrollTop).not.toBe(2200);       // NOT 2700, the card's own bottom
    expect(el.scrollTop).toBeLessThan(2500);   // NOT the transcript bottom either
  });

  it('survives the answered body replacing the live one, because it holds the PANEL', () => {
    // Why the landing anchors on `.initiator-panel` rather than the
    // `.question-body` inside it. Answering swaps `QuestionBody`'s live body
    // for `AnsweredBody`, a different component, so Preact unmounts the body
    // node a frame or two into the glide. The panel around it is the same vnode
    // in the same position and is reused. Anchored on the body, this glide
    // would go dead the moment the answer rendered.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    const live = el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followAnsweredQuestion('q1');
    vi.advanceTimersByTime(60);
    const midGlide = el.scrollTop;
    expect(midGlide).toBeGreaterThan(500);

    el.answerQuestionCard(live);
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2392); // still lands on the card, not stranded
    expect(el.scrollTop).toBeGreaterThan(midGlide);
  });

  it('then holds still, so the reply grows in under the card', () => {
    // The answer's copy of the rule: the landing is the whole reaction, not the
    // first half of a ride.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followAnsweredQuestion('q1');
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2392);
    el.writes = 0;

    el.scrollHeight = 5000;
    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2392);
  });

  it('stops the glide the moment the reader scrolls, rather than dragging them back', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followAnsweredQuestion('q1');
    vi.advanceTimersByTime(60);
    expect(el.scrollTop).toBeGreaterThan(500);

    readerScrollsTo(el, 1200, onScroll); // the reader takes over mid-glide
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(1200);

    el.writes = 0;
    for (const height of [4000, 5000, 9000]) {
      el.scrollHeight = height;
      onResize();
    }

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(1200);
  });

  it('waits for a card that has no box yet, and moves nobody until it has one', () => {
    // A card with no box (the hidden dual-mount copy, a windowed-out render) must
    // not produce a blind jump. The landing waits it out on the same deferral the
    // send uses, and lands the moment the card is addressable.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'another-question', top: 2400, height: 300 });
    el.writes = 0;

    followAnsweredQuestion('q1');
    vi.advanceTimersByTime(100); // well inside the landing deadline
    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(500);

    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 }); // it renders
    el.scrollHeight = 4000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2392);
  });
});

describe('nothing is reserved under the newest turn', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** NO TAIL ROOM IS PUBLISHED, and this block is what stops it coming back by
   *  accident. It was one viewport of `min-height` under the last turn,
   *  measured here and applied in chat/response.css. That let a submit land the
   *  turn's top on the landing line. Why it was withdrawn: ADR 0064.
   *
   *  What the landing does instead is measure: see the reveal branch in
   *  `landOnOwnTurn`, covered by the queued-follow-up tests above. */
  const LAYOUT = { paddingBottom: '56px', scrollMarginTop: '72px' };

  function withLayout(run: () => void) {
    (globalThis as any).getComputedStyle = () => LAYOUT;
    try { run(); } finally { delete (globalThis as any).getComputedStyle; }
  }

  const roomOn = (el: any) => el.style.getPropertyValue('--transcript-tail-room');

  it('publishes no room, on a resize or on either edge of the follow', () => {
    withLayout(() => {
      const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
      const { onResize } = makeScrollObservers(el);
      setActiveScrollElement(el);
      el.addUserMessage({ top: 2900, height: 120 });

      // The three moments a tail-room property would be written from. Both
      // toggle edges are named: the toggle changes no element's size, so a room
      // would need republishing from the arm and the retire by hand.
      onResize();
      expect(roomOn(el)).toBe('');
      setFollowLiveEdge(true);
      expect(roomOn(el)).toBe('');
      setFollowLiveEdge(false);
      expect(roomOn(el)).toBe('');

      resumeFollowingBottom(el);
      expect(roomOn(el)).toBe('');
    });
  });

  it('leaves a rider on the bottom of the CONTENT through a reply', () => {
    withLayout(() => {
      const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
      const { onResize } = makeScrollObservers(el);
      setActiveScrollElement(el);
      el.addUserMessage({ top: 2900, height: 120 });
      // Armed FROM the live edge, so the toggle writes no scroll and starts no
      // tween: the growth branch stands down while one owns the scroll, and the
      // point here is the growth rounds.
      atBottom(el);
      setFollowLiveEdge(true);

      for (const height of [3400, 5000, 9000]) {
        el.scrollHeight = height;
        onResize();
      }

      expect(el.scrollTop).toBe(9000 - 500);
    });
  });
});

describe('a submit made while already riding the live edge goes to the bottom', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** The landing above is for a reader who had scrolled away. A reader already
   *  RIDING the live edge asked for the opposite, as a standing request: keep me
   *  at the bottom. Landing them on their own turn puts them off the bottom with
   *  the reply below the fold, the state they armed the follow to avoid.
   *
   *  This is the whole of what a submit and the standing follow have to do with
   *  each other. The submit SERVES a request the toggle made, and never makes
   *  one.
   *
   *  Armed but off the live edge is an ordinary state. The growth branch stands
   *  down while a tween owns the scroll. A reply streaming entirely during a
   *  glide therefore leaves the reader parked above the bottom, follow still
   *  armed. `ridingButParked` reproduces that, and the next submit is made from
   *  there. */
  function ridingButParked(el: any) {
    atBottom(el);
    setFollowLiveEdge(true); // the reader arms the follow, at the live edge
    el.scrollHeight = 6000;  // and the reply grew while a tween owned the scroll
    el.writes = 0;
  }

  it('takes a send to the live edge, not to the just-sent message', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    ridingButParked(el);

    followSentMessage();
    el.addUserMessage({ top: 5800, height: 120 }); // the optimistic row renders
    el.scrollHeight = 6200;                        // with the working indicator under it
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(5700);         // 6200 - 500, the true bottom
    expect(el.scrollTop).not.toBe(5448);     // and NOT the landing on its own turn
  });

  it('takes an answer to the live edge, not to the answered card', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    ridingButParked(el);

    followAnsweredQuestion('q1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(5500); // 6000 - 500
  });

  it('takes a permission decision to the live edge', () => {
    // The third submit shape. Deciding a card resumes the agent underneath the
    // reader exactly as answering a question does, and a rider is owed that.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addPermissionCard({ requestId: 'p1', top: 2400, height: 300 });
    ridingButParked(el);

    followResolvedPermission('p1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(5500);
  });

  it('takes a Continue to the live edge, without waiting for the continuation', () => {
    // The fourth. Continue's landing is deferred on a turn that does not exist
    // yet, but a rider is not waiting for it: they asked to be at the bottom, and
    // that is answerable now.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    ridingButParked(el);

    followContinuedThread();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(5500);
  });

  it('lands on the bottom the transcript has when the glide ENDS', () => {
    // The target is re-read per frame (ADR 0065), so a reply still streaming
    // during the glide is tracked rather than left one screen short.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    ridingButParked(el);

    followSentMessage();
    vi.advanceTimersByTime(100);
    el.scrollHeight = 9000;
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(8500);
  });

  it('stops the glide the moment the reader scrolls, and growth then moves them 0px', () => {
    // The glide is the follow's OWN motion, so the reader taking over retires
    // both. Same contract the landing glide has.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    ridingButParked(el);

    followSentMessage();
    vi.advanceTimersByTime(60);
    expect(el.scrollTop).toBeGreaterThan(2500);

    readerScrollsTo(el, 1000, onScroll); // the reader takes over mid-glide
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(1000);

    el.writes = 0;
    el.scrollHeight = 9000;
    onResize();
    expect(el.writes).toBe(0);
  });

  it('a second call for the same send does not queue a landing behind the glide', () => {
    // PromptInput.submit and addPendingMessage both fire for one composer send,
    // and by the second call the follow is armed and the glide is in flight. Read
    // naively, that second call would set up a landing on the row that has since
    // rendered and undo the ride the first call started.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    ridingButParked(el);

    followSentMessage();
    el.addUserMessage({ top: 5800, height: 120 }); // the row renders between the two
    el.scrollHeight = 6200;
    followSentMessage();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(5700); // the live edge, not the turn landing at 5448

    el.scrollHeight = 7000; // and no landing is left pending: growth just rides
    onResize();
    expect(el.scrollTop).toBe(6500);
  });

  it('a landing glide is superseded by another SUBMIT, not left to finish', () => {
    // Two submits in a row from a reader following nothing, which is the ordinary
    // case now that a submit arms nothing: the second one owns the viewport, so
    // it lands on ITS turn rather than letting the first tween finish on the
    // earlier one.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followSentMessage();
    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();                    // the send's landing glide starts, for 2892
    vi.advanceTimersByTime(60);

    followAnsweredQuestion('q1');  // and the reader answers the card mid-glide
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2392); // the CARD's turn, not the send's
  });

  it('a glide that already LANDED does not suppress the next submit', () => {
    // The "leave a live-edge glide alone" guard must describe a tween in
    // flight, not one that finished. Otherwise the flags a completed glide left
    // behind answer for it, and the next submit moves nobody.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    ridingButParked(el);

    followSentMessage();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(5500);

    el.scrollHeight = 9000; // more arrived while the growth branch stood down
    followSentMessage();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(8500);
  });

  it('turns the down chevron off once the glide lands', () => {
    // The last frame can land where the previous one already put the container,
    // and then no scroll event arrives to reconcile the signal.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    ridingButParked(el);
    awayFromBottom.value = true;

    followSentMessage();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(5500);
    expect(awayFromBottom.value).toBe(false);
  });

  it('writes once instead of gliding under reduced motion', () => {
    const realMatchMedia = window.matchMedia;
    (window as any).matchMedia = (q: string) => ({
      matches: q.includes('prefers-reduced-motion'),
      addEventListener() {},
      removeEventListener() {},
    });
    try {
      const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
      makeScrollObservers(el);
      setActiveScrollElement(el);
      ridingButParked(el);

      followSentMessage();

      expect(el.writes).toBe(1);
      expect(el.scrollTop).toBe(5500);
    } finally {
      (window as any).matchMedia = realMatchMedia;
    }
  });

  it('writes no scroll for a rider who is already at the live edge', () => {
    // Unchanged for all four shapes: they are already there, growth keeps them
    // there, and the write would cancel an iOS momentum scroll for nothing.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addPermissionCard({ requestId: 'p1', top: 2400, height: 300 });
    const parked = atBottom(el);
    setFollowLiveEdge(true);
    el.writes = 0;

    followResolvedPermission('p1');
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
  });

  it('a permission decision LANDS an unarmed reader on the card it decided', () => {
    // A permission decision is a submit like the others: the turn they decided
    // goes to the top, with the resumed reply under it. Moving an unarmed
    // reader zero pixels would resume the agent below the fold with nothing on
    // screen saying so.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addPermissionCard({ requestId: 'p1', top: 2400, height: 300 });

    followResolvedPermission('p1');
    expect(el.scrollTop).toBe(500); // a glide, not a jump
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2392); // 2400 (the turn top) - the clearance

    el.writes = 0;
    el.scrollHeight = 4000;
    onResize();
    expect(el.writes).toBe(0);       // and it armed nothing on the way
    expect(el.scrollTop).toBe(2392);
  });

  it('lands on the card the reader DECIDED, never on a later turn', () => {
    // The anchor is the acted-on turn, resolved through its own
    // `.permission-body[data-request-id]`. A later turn in the transcript must
    // not pull the glide past it, and the card the reader ignored must not
    // capture it either.
    const el = makeEl({ scrollTop: 500, scrollHeight: 6000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addPermissionCard({ requestId: 'p1', top: 2400, height: 300 });
    el.addPermissionCard({ requestId: 'p2', top: 4400, height: 300 });

    followResolvedPermission('p1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2392);            // p1's turn top
    expect(el.scrollTop).not.toBe(4228);        // NOT p2's
    expect(el.scrollTop).toBeLessThan(5500);    // NOT the live edge
  });
});

describe('Continue after an abort is a submit too', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** The fourth submit. Its turn does not exist when the button is pressed: the
   *  continuation renders as a fresh `ContinuationStarted` exchange, over SSE,
   *  after the POST. So it takes the send's deferred landing with a different
   *  notion of the turn it waits for. That turn is a `.chat-exchange` other
   *  than the one that was last, since a continuation renders no user
   *  message. */

  it('lands on the continuation turn\'s status line once it renders', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followContinuedThread();
    expect(el.scrollTop).toBe(500); // nothing to land on yet

    el.addContinuationTurn({ top: 2400, height: 300 }); // the SSE event arrives
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2392); // 2400 (the turn top) - the clearance
  });

  it('arms nothing, so the resumed reply grows in under it', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followContinuedThread();
    el.addContinuationTurn({ top: 2400, height: 300 });
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2392);
    el.writes = 0;

    for (const height of [5000, 9000]) {
      el.scrollHeight = height;
      onResize();
    }

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2392);
  });

  it('LAPSES and moves nobody when the continuation never renders', () => {
    // The POST failed, or the SSE never came. A deferred landing with nothing to
    // land on gives up rather than falling back to the live edge.
    const nowSpy = vi.spyOn(performance, 'now');
    let clock = 1_000_000;
    nowSpy.mockImplementation(() => clock);
    try {
      const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
      const { onResize } = makeScrollObservers(el);
      setActiveScrollElement(el);

      followContinuedThread();
      el.writes = 0;

      clock += 1500; // past LANDING_DEADLINE_MS with no continuation turn
      el.scrollHeight = 9000;
      onResize();
      onResize();
      vi.advanceTimersByTime(1500);

      expect(el.writes).toBe(0);
      expect(el.scrollTop).toBe(500);
    } finally {
      nowSpy.mockRestore();
    }
  });

  it('writes NO scroll when the reader is already at the live edge', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    const parked = atBottom(el);
    el.writes = 0;

    followContinuedThread();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
  });
});

/** Every submit, driven from the SAME starting state through the SAME four
 *  branches. One reaction everywhere is then checked rather than asserted four
 *  times in four dialects. The blocks above are each surface's own story. This
 *  is the matrix.
 *
 *  The geometry is shared on purpose. Whichever surface it is, the turn the
 *  reader acted on spans 2400..2700, so every landing in the table is 2392: its
 *  top minus the clearance. A surface answering differently is doing something
 *  the others are not. */
const SUBMIT_SURFACES: Array<{
  name: string;
  /** Render what must be on screen BEFORE the submit, at `top`. The two deferred
   *  submits render nothing: their turn does not exist yet. */
  arrange: (el: any, top?: number) => void;
  /** Make the submit. For a deferred surface, render the turn it waits for at
   *  `top` and give the growth branch the round it lands on. */
  submit: (el: any, onResize: () => void, top?: number) => void;
}> = [
  {
    name: 'a sent message',
    arrange: () => {},
    submit: (el, onResize, top = 2400) => {
      followSentMessage();
      el.addUserMessage({ top, height: 300 });
      onResize();
    },
  },
  {
    name: 'an answered question card',
    arrange: (el, top = 2400) => el.addQuestionCard({ toolUseId: 'q1', top, height: 300 }),
    submit: () => followAnsweredQuestion('q1'),
  },
  {
    name: 'a decided permission card',
    arrange: (el, top = 2400) => el.addPermissionCard({ requestId: 'p1', top, height: 300 }),
    submit: () => followResolvedPermission('p1'),
  },
  {
    name: 'Continue after an abort',
    arrange: () => {},
    submit: (el, onResize, top = 2400) => {
      followContinuedThread();
      el.addContinuationTurn({ top, height: 300 });
      onResize();
    },
  },
];

describe.each(SUBMIT_SURFACES)('the submit matrix: $name', ({ arrange, submit }) => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** The standard fixture: a real transcript with room to move, the reader
   *  parked well above the live edge. */
  function scrolledUp() {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    arrange(el);
    return { el, ...observers };
  }

  it('branch 1, the turn is already at the landing line: writes nothing', () => {
    // A thread that fits on screen, with the turn at its top, which is the only
    // way a short transcript can be laid out. The landing measures its target,
    // finds it at or behind the reader, and writes nothing. There must be no
    // pre-emptive "this thread is too short" test: asked one commit earlier, it
    // strands a reader who sent from the bottom of a long thread.
    const el = makeEl({ scrollTop: 0, scrollHeight: 400 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    arrange(el, 0);
    el.writes = 0;

    submit(el, onResize, 0);
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(0);
  });

  it('branch 2, at the live edge: writes no scroll at all', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    arrange(el);
    const parked = atBottom(el);
    el.writes = 0;

    submit(el, onResize);
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
  });

  it('branch 3, riding the live edge: glides to the live edge', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    arrange(el);
    atBottom(el);
    setFollowLiveEdge(true); // the reader arms the follow, at the live edge
    el.scrollHeight = 6000;  // and the reply grew while a tween owned the scroll
    el.writes = 0;

    submit(el, onResize);
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(5500); // 6000 - 500
  });

  it('branch 4, scrolled up: lands the turn\'s agent status line', () => {
    const { el, onResize } = scrolledUp();

    submit(el, onResize);
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2392); // 2400 (the turn top) - the clearance
  });

  it('arms nothing: growth after the submit moves the reader 0px', () => {
    const { el, onResize } = scrolledUp();

    submit(el, onResize);
    vi.advanceTimersByTime(1500);
    const landed = el.scrollTop;
    el.writes = 0;

    for (const height of [4000, 6000, 20000]) {
      el.scrollHeight = height;
      onResize();
    }

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(landed);
  });

  it('a gesture mid-glide cancels the landing rather than being fought', () => {
    const { el, onScroll, onResize } = scrolledUp();

    submit(el, onResize);
    vi.advanceTimersByTime(60);
    expect(el.scrollTop).toBeGreaterThan(500);

    readerScrollsTo(el, 700, onScroll); // the reader takes over mid-glide
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(700);
  });
});

describe('nothing but the chevron, a send and an answer arms it', () => {
  beforeEach(() => { resetFollow(); });
  afterEach(() => { resetFollow(); });

  /** Each of these growths is a regression test against the force-pin coming
   *  back (ADR 0064). */
  function unmovedBy(grow: (el: any) => void) {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const parked = atBottom(el);
    el.writes = 0;
    grow(el);
    onResize();
    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
  }

  it('an SSE sync confirming a pending message moves nobody', () => {
    // The optimistic row is replaced by the persisted event, which re-lays the
    // turn out and usually changes its height a little.
    unmovedBy((el) => { el.scrollHeight = 3040; });
  });

  it('a change being applied moves nobody', () => {
    // The resolution card is appended below the reader.
    unmovedBy((el) => { el.scrollHeight = 3400; });
  });

  it('a lazy load moves nobody', () => {
    // A whole page of older turns renders. Size is not evidence of intent.
    unmovedBy((el) => { el.scrollHeight = 20000; });
  });

  it('a question card ARRIVING moves nobody, though answering it would', () => {
    // The exact mirror of the block above. Answering is the reader producing
    // content at the bottom, being asked is the agent producing it. The same
    // element being involved must not confuse the two.
    unmovedBy((el) => {
      el.addQuestionCard({ toolUseId: 'q1', top: 2700, height: 300 });
      el.scrollHeight = 3300;
    });
  });

  /** Why this stays a source scan and not a behavioural test. The failure is a
   *  NEW store action reaching for the live edge. No behavioural test fails for
   *  a call site it does not know exists. A site that genuinely must arm the
   *  follow fails here: say why at the site and list it below, rather than
   *  weakening the scan. */
  it('no store action outside the send path reaches for the live edge', () => {
    const here = dirname(fileURLToPath(import.meta.url));
    const ACTIONS_DIR = resolve(here, '../../../store/actions');
    /** The sanctioned callers, per module. `chat.ts` owns the send's own call
     *  and `threads.ts` retires the follow when the reader opens another
     *  thread. Every other submit has NO sanctioned store-action caller, for
     *  the same reason three times: the store action is the TRANSPORT, carrying
     *  decisions nobody watched happen, while the reader's own tap lives in the
     *  component. So each is called from its card, never from its transport:
     *
     *  - `followAnsweredQuestion` from `QuestionCard` / `PromptInput`
     *  - `followResolvedPermission` from `PermissionCard`
     *  - `followContinuedThread` from `chat-exchange-parts.tsx` */
    const ALLOWED: Record<string, string[]> = {
      'chat.ts': ['followSentMessage'],
      'threads.ts': ['stopFollowingBottom'],
    };
    const CALLS = ['followSentMessage', 'followAnsweredQuestion', 'followResolvedPermission', 'followContinuedThread', 'scrollToBottom', 'scrollToBottomAnimated', 'stopFollowingBottom'];

    const offenders: string[] = [];
    for (const name of readdirSync(ACTIONS_DIR)) {
      if (!name.endsWith('.ts') || name.includes('.test.')) continue;
      const source = readFileSync(join(ACTIONS_DIR, name), 'utf8');
      for (const call of CALLS) {
        if (!source.includes(call)) continue;
        if ((ALLOWED[name] ?? []).includes(call)) continue;
        offenders.push(`${name}: ${call}`);
      }
    }
    expect(offenders).toEqual([]);
  });

  /** The follow has exactly TWO arming sites, and this is what says so. Both go
   *  through the private `armFollowOn`, so the scan reads its callers inside
   *  `scrollState.ts` rather than trusting prose. They are the follow toggle
   *  (the reader making the request) and the resume, which replays a request
   *  made in this thread earlier. Only the toggle can record one.
   *
   *  The chevrons are named as NON-arming, since that is the rule most likely
   *  to be undone. A reader of `scrollToBottomAnimated` may assume "go to the
   *  bottom" means "and stay". It does not: one button cannot be go-there,
   *  stay-here and stop-staying at once (ADR 0064). */
  it('only the follow toggle and the resume arm the follow', () => {
    const here = dirname(fileURLToPath(import.meta.url));
    const source: string = readFileSync(resolve(here, '../scrollState.ts'), 'utf8');

    /** Every function whose BODY calls `armFollowOn(...)`. Bodies are found by
     *  walking from each `function <name>(` to its balanced closing brace, which
     *  is enough for this module: every function in it is a top-level
     *  declaration. */
    const armers: string[] = [];
    const re = /\bfunction (\w+)\s*\(/g;
    for (let m = re.exec(source); m !== null; m = re.exec(source)) {
      const open = source.indexOf('{', m.index);
      if (open < 0) continue;
      let depth = 0;
      let end = -1;
      for (let i = open; i < source.length; i++) {
        if (source[i] === '{') depth++;
        else if (source[i] === '}' && --depth === 0) { end = i; break; }
      }
      if (end < 0) continue;
      const body = source.slice(open, end);
      if (/\barmFollowOn\s*\(/.test(body) && m[1] !== 'armFollowOn') armers.push(m[1]);
    }

    expect(armers.sort()).toEqual(['resumeFollowingBottom', 'setFollowLiveEdge']);
  });

  /** The scan above pins the ARMING sites inside the module. This one pins the
   *  two entry points that let the reading position record and resume a follow.
   *  It reaches the whole tree rather than `store/actions` alone, because
   *  neither belongs to a store action.
   *
   *  `resumeFollowingBottom` re-arms a follow the reader is not making now,
   *  which is safe for one reason: it fires only for a thread whose RECORDED
   *  reading position is the live edge. A second caller would lose that
   *  guarantee and become a third arming point. `onFollowArmed` broadcasts the
   *  arm and not the retirement, which lets `focusThread` retire the follow
   *  without erasing the record of it.
   *
   *  It matches a CALL shape rather than a bare mention, unlike the scan above.
   *  That scan wants the wider net: a store action so much as importing an
   *  arming entry point is already reaching for the live edge. These two are
   *  the mechanism the follow's lifetime is built from, so participating
   *  modules name them in prose while calling neither. */
  it('only the reading position records and resumes a follow', () => {
    const here = dirname(fileURLToPath(import.meta.url));
    const SRC_DIR = resolve(here, '../../..');
    const DEFINED_IN = 'components/chat/scrollState.ts';
    const ALLOWED_CALLER = 'hooks/useScrollMemory.ts';

    const found: Record<string, string[]> = { resumeFollowingBottom: [], onFollowArmed: [] };
    const walk = (dir: string) => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const full = join(dir, entry.name);
        if (entry.isDirectory()) {
          if (entry.name !== '__tests__' && entry.name !== 'generated') walk(full);
          continue;
        }
        if (!/\.tsx?$/.test(entry.name) || entry.name.includes('.test.')) continue;
        const rel = full.slice(SRC_DIR.length + 1);
        if (rel === DEFINED_IN) continue;
        const source = readFileSync(full, 'utf8');
        for (const call of Object.keys(found)) {
          if (new RegExp(`\\b${call}\\s*\\(`).test(source)) found[call].push(rel);
        }
      }
    };
    walk(SRC_DIR);

    expect(found.resumeFollowingBottom).toEqual([ALLOWED_CALLER]);
    expect(found.onFollowArmed).toEqual([ALLOWED_CALLER]);
  });
});
