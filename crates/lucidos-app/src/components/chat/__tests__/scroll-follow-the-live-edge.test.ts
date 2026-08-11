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

/** "Go to the bottom" means "and keep me there until I say otherwise", and ONLY
 *  the down chevron says it.
 *
 *  The reader still owns the transcript's scroll position, and the app still
 *  moves it only when the reader asks. What this file pins down is the DURATION
 *  of the chevron's ask (it arms a standing follow, growth honours it, and only
 *  the reader's own scroll retires it) and, against it, the ONE-SHOT reaction
 *  every SUBMIT gets: a glide to the live edge for a reader already riding it,
 *  and otherwise a landing that rests the turn's agent status line on the bottom
 *  of the viewport. A submit arms nothing. A send and an answer used to, and the
 *  blocks below that read "arms nothing" are where that inverted.
 *
 *  Its mirror is `scroll-resize-never-follows.test.ts`, which pins the other
 *  half: growth moves an UNARMED reader zero pixels, including one who happens
 *  to be sitting exactly at the live edge. Being at the bottom is a position,
 *  not a request, and that distinction is the whole point of both files. */

/** How tall the agent status line is in this fake: one row of a turn header. The
 *  landing aims at its BOTTOM edge, so every expected offset in this file is the
 *  reader's own panel bottom plus this. */
const STATUS_LINE_HEIGHT = 28;

/** A `.thread-content` stand-in that clamps `scrollTop` the way a browser does,
 *  counts writes (so a jump that happens to land where the reader already was
 *  still fails a no-scroll assertion), and can hold user-message panels and
 *  question cards whose rects follow the scroll position, which is what the
 *  send's and the answer's landings measure.
 *
 *  `panels` are given in SCROLL coordinates (`top` from the top of the content);
 *  their viewport rect is derived from the live `scrollTop` on every read, so a
 *  tween measuring them per frame sees a moving target exactly as it would in a
 *  browser. */
function makeEl(opts: {
  scrollTop: number;
  scrollHeight: number;
  clientHeight?: number;
  panels?: Array<{ top: number; height: number }>;
  /** How many `.chat-exchange` turns the transcript holds. Defaults to ONE,
   *  which is what an ordinary transcript has and what every test about the
   *  follow's behaviour assumes without saying so. `0` is the brand-new thread:
   *  a compose view showing the welcome message, or a promoted thread whose
   *  first optimistic row has not rendered yet. A submit asks nothing of a
   *  transcript with no conversation in it (see `hasSomewhereToLand`), so the
   *  count is not decoration. */
  turns?: number;
}) {
  const panels: any[] = [];
  const questionCards: any[] = [];
  const permissionCards: any[] = [];
  const turns: any[] = [];
  const el: any = {
    parentElement: null,
    children: [],
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
     *  `visible` false models a panel with no box, which is what a queued
     *  follow-up folded into its closed disclosure group has.
     *
     *  It adds a TURN as well, because a user message is rendered inside a
     *  `.chat-exchange` and a transcript showing one is not an empty one. That
     *  matters for a brand-new thread, whose transcript holds no turn when the
     *  send is made and one a render later.
     *
     *  And the turn arrives WITH its agent status line, because that is what the
     *  app does: the optimistic row and the "Requesting" response panel under it
     *  are one commit. `status: false` is the turn that never gets one, which is
     *  the queued follow-up (its "Queued" tag lives in its own bubble) and the
     *  frame before the response panel mounts; `mountStatusLine` covers the
     *  second half of that. */
    addUserMessage(p: { top: number; height: number; visible?: boolean; status?: boolean }) {
      const turn = makeTurn();
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
    /** Render a question card: a `.question-body` carrying the card's tool-use id
     *  inside the `.initiator-panel` that is the answer's landing target. The two
     *  are given DIFFERENT rects (the body is inset inside the panel) so a test
     *  landing on the panel cannot pass by measuring the body instead. The turn
     *  around them carries a status line like any other, since the agent resumes
     *  under the card the reader answered. */
    addQuestionCard(p: { toolUseId: string; top: number; height: number; status?: boolean }) {
      return addCard(questionCards, 'data-tool-use-id', p.toolUseId, p);
    },
    /** Render a permission-shaped card: a `.permission-body` carrying the card's
     *  REQUEST id, inside the same `.initiator-panel` a question card sits in.
     *  One shape for all three (the coding-agent tool permission, the command
     *  guard, the MCP tool consent), because all three render this body through
     *  `PermissionBodyShell` and decide through one hook. */
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
    /** The live→answered swap, as Preact actually performs it: `QuestionBody`
     *  returns a DIFFERENT component once the answer lands, so the body node is
     *  unmounted and a new one mounted, while the `.initiator-panel` around it is
     *  the same vnode in the same position and is REUSED. So the panel object is
     *  carried over, and only the body is replaced. */
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
   *  reject an invisible one, and `turnStatusLine` walks `closest` from it (which
   *  answers with the element itself, as the DOM's does). */
  function makeTurn(top = 0, height = 100) {
    const turn: any = {
      parentElement: null,
      isConnected: true,
      statusLine: null,
      closest: (sel: string) => (sel === '.chat-exchange' ? turn : null),
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
   *  landing target. The two are given DIFFERENT rects (the body is inset inside
   *  the panel) so a test landing on the panel cannot pass by measuring the body
   *  instead. The turn around them carries a status line like any other, since
   *  the agent resumes under the card the reader resolved. */
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

/** The follow is module state, so a test that armed it would otherwise leak into
 *  the next one. Retiring it is the same call `focusThread` makes when the reader
 *  opens another thread, not a test-only hatch.
 *
 *  It also puts the thread in its LIVE state, because that is what every test in
 *  this file except the idle-scroll pair is about: a reader with a reply in
 *  flight. The disarm asks whether the agent is live (`setThreadLive`, written by
 *  `ChatExchange` from the last turn's status), so a file-wide default of `false`
 *  would silently make every disarm test pass for the wrong reason. The tests
 *  that care about idle say so explicitly. */
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
 *  Both halves are required, and that is the contract this file is mostly
 *  about. A scroll retires the follow only when a GESTURE is behind it, because
 *  the position alone cannot tell a flick from the iOS keyboard resizing the
 *  container or a backgrounded app being resumed, and those were retiring a
 *  follow the reader had armed and never touched. So writing `scrollTop` and
 *  calling `onScroll` WITHOUT this helper now models exactly that: the platform
 *  moving the container, which deliberately retires nothing.
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
    // The gap this closes. The chevron used to be a one-shot jump: it landed the
    // reader at the bottom and the next chunk stranded them above it again, so
    // following a streaming reply meant tapping it over and over.
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
    // The crux. This reader is at the identical position as the one above and
    // gets the identical growth, and is not moved a pixel, because following is
    // an explicit request and not a proximity to the bottom.
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
    // A scrollTop write fires its scroll event a frame LATER, and by then a
    // streaming thread has often grown, so the container reads as off the live
    // edge when the handler runs. Position, not proximity, is what tells the
    // follow's own write from the reader's gesture: growth changes scrollHeight
    // and never scrollTop, so the container is still exactly where the follow
    // left it.
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
    // Only a scroll retires the follow. A card resolving swaps content under the
    // reader and a disclosure grows it, and neither is the reader saying they
    // want to be somewhere else. (A card the READER resolved arms the follow in
    // its own right now; what this pins is that the resolution's reflow does not
    // retire one already armed, however it was armed.)
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

  /** THE INVERSION for the chevron. It armed the follow on landing while it was
   *  the only affordance that could, which left the mode with no visible state,
   *  no way off but scrolling, and no way ON for a reader already at the live
   *  edge, since the chevron is hidden exactly there. The follow has a button of
   *  its own now, so the chevron is a navigation like the up chevron and turn
   *  stepping: it takes the reader to the bottom and stops. */

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
    // The arming used to happen in `onDone`, so a tween superseded mid-flight
    // never armed. There is nothing left to arm in either form.
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
 * The retirement used to be read off the POSITION alone: a container that had
 * moved away from our last write had, it was reasoned, been moved by the
 * reader, because content growth changes `scrollHeight` and never `scrollTop`.
 * That premise covers growth and nothing else, and three things move the
 * container with no gesture behind them, all reported from an iOS PWA on
 * 2026-08-11:
 *
 *   - the soft keyboard opening or closing, which rewrites `--app-height`,
 *     resizes the transcript under the reader, and lets WebKit adjust the
 *     offset asynchronously through the ~350ms animation;
 *   - an app backgrounded and resumed, where the PWA restores an offset nobody
 *     wrote;
 *   - the full response / steps toggle, whose anchor correction is a write of
 *     ours made from a position we could not stamp in advance.
 *
 * Each retired a follow the reader had armed and never touched, while a reply
 * was streaming, which is the one moment the feature is worth anything. So the
 * question is asked of the INPUT now. These tests are the two directions of
 * that: the platform moves the container and the ride survives, the reader
 * moves it and the ride ends.
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
    // reader is left well above the live edge. Then WebKit adjusts the offset
    // itself, asynchronously, which is the scroll event that used to read as a
    // flick. No finger has touched the transcript in any of it.
    const { el, onScroll, onResize } = riding();

    el.clientHeight = 200;
    onResize();
    el.scrollTop = 1400;
    onScroll();

    expect(followingLiveEdge.value).toBe(true);
  });

  it('survives the keyboard closing again', () => {
    // Each step is the resize AND an offset WebKit adjusted on its own, off the
    // live edge. The offset is what makes this test say anything: a resize
    // alone runs `honourGrowth`, which writes the live edge and re-stamps the
    // hold, so `atEdge` and `tookOver` would both answer "not the reader"
    // before the gesture term was ever consulted.
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
    // The other end of the window, and the half that keeps it a WINDOW rather
    // than a latch: a flick five seconds ago says nothing about a scroll now,
    // and if it did, the keyboard opening after any earlier scroll would retire
    // the follow all over again.
    const { el, onScroll } = riding();

    readerGestureForTest(el);
    vi.advanceTimersByTime(5000);
    el.scrollTop = 900;
    onScroll();

    expect(followingLiveEdge.value).toBe(true);
  });

  it('does not read a PRESS inside the transcript as a scroll', () => {
    // Answering a question, granting a permission, expanding a turn: each is a
    // press on a control INSIDE the transcript, and each changes content, which
    // is exactly the combination this module documents as keeping the follow.
    // Arming attribution on the press would put a 1.2s window over every one of
    // them, so only MOVEMENT arms it. Modelled here the way the listeners see
    // it: a press records no movement, so the signal stays cold and the content
    // change that follows is attributed to the app.
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

describe('scrolling an IDLE thread keeps the follow', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** Two acts produce an identical scroll event, and the disarm has to tell them
   *  apart. Scrolling away from a reply IN FLIGHT means "stop dragging me".
   *  Scrolling on an IDLE thread is browsing: nothing is moving, nothing is
   *  dragging anybody, and going back to re-read a turn before writing the next
   *  message is not a decision about how the next reply should behave. It
   *  silently was one until the disarm learned to ask, and the reader paid for it
   *  at their next submit. */

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
    expect(el.scrollTop).not.toBe(2228);   // and NOT the landing on the card
  });

  it('a submit after a LIVE scroll gets the landing instead', () => {
    const { el } = armedThenScrolledUp(true);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followAnsweredQuestion('q1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2228);       // the card's status line
  });

  it('a SUBMIT makes the thread live, so scrolling away after it still disarms', () => {
    // The gap the status cannot cover. Answering a card leaves the last turn on
    // `awaiting-answer`, which is NOT an active status, until the engine's
    // resumed status arrives over SSE a round trip later. Read off the status
    // alone the thread is idle for that whole window, so a reader who answered
    // and then fled the reply would keep their follow and be hauled back the
    // moment it resumed. A submit is by definition an act the agent is expected
    // to respond to, so it marks the thread live itself.
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
    // The regression that shipped on 2026-08-10 and was reported the same day.
    // The live term needs the projection to agree, and the projection is the slow
    // half: the client's `meta.status` only advances when a per-event aggregate
    // carrying `running` arrives, which is seconds after the send. So the render
    // right after a submit writes `false` while the agent is on its way, and
    // `setThreadLive` used to clear the claim on any write, in either direction.
    // That destroyed the claim in the one window it exists for, and the reader
    // who submitted and then fled kept the follow. Intermittent in the wild,
    // because it depended on the scroll landing inside the gap.
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
    // The claim has to expire rather than stand until something contradicts it,
    // because the thing that would contradict it may never come: a Continue
    // whose POST fails, or a decision the engine never answers, leaves the last
    // turn's status exactly as it was, so `ChatExchange`'s effect never re-runs
    // and never writes `false`. Left standing, the claim would quietly cost the
    // reader their follow the next time they browsed this idle thread.
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

  /** The other half of the block above, and the half that was missing until
   *  2026-08-11. The disarm asked whether the agent was live; the WRITE did not.
   *  So the reader kept the ride when they scrolled an idle thread, exactly as
   *  promised, and was then written back to the live edge by the next thing that
   *  changed the transcript's height. On an idle thread that is the transcript
   *  finishing its own rendering: markdown settling, an image decoding, a card
   *  mounting. Reported from a session parked on a question card, which is idle
   *  by every measure this module has (`awaiting-answer` is not an active
   *  status), while reading a reply that had already finished.
   *
   *  ARMED and CARRYING are the two states this pins apart. Nothing here retires
   *  anything: the toggle stays lit and the ride resumes on its own. */

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
    // settles over several frames, and every one of them used to move the
    // reader.
    const { el, onResize } = armedThenScrolledUpOnAnIdleThread();

    for (const height of [3400, 4000, 9000]) {
      el.scrollHeight = height;
      onResize();
      expect(el.scrollTop).toBe(800);
    }
    expect(el.writes).toBe(0);
  });

  it('still shows them the chevron, so they can go back themselves', () => {
    // Standing down from the WRITE is not standing down from the signals: the
    // reader is off the live edge and has to be told, or the one way back is
    // gone along with the ride that used to carry them.
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
    // The ordering the two signals actually arrive in. The mutation that wakes a
    // thread (the new turn's row mounting) fires the ResizeObserver inside its
    // own frame, while `ChatExchange` publishes the new status from a Preact
    // effect, which is deferred to a task AFTER that frame. So `honourGrowth`
    // sees the waking resize while this module still reads idle, and stands
    // down for the one round that mattered.
    //
    // Modelled exactly that way: the growth and its resize land FIRST, the
    // liveness arrives after. A streaming reply hides this by resizing again a
    // moment later; a coding-agent turn resuming its subprocess does not, and
    // sits on its mounted row for fifteen to twenty seconds. Reported by the
    // Codex reviewer, 2026-08-11.
    const { el, onResize } = armedThenScrolledUpOnAnIdleThread();

    el.scrollHeight = 4000;  // the waking turn's row mounts
    onResize();              // and its resize is delivered while we still read idle
    expect(el.scrollTop).toBe(800);

    setThreadLive(true);     // the effect lands a task later, with no resize behind it

    expect(el.scrollTop).toBe(3500);
  });

  it('acts on the EDGE only, so a repeated live signal writes nothing', () => {
    // Only a WAKE describes new content. `ChatExchange` re-runs its effect
    // whenever its derived liveness changes, and a `true` that says what the
    // module already knew is not a second turn arriving. The distinction has to
    // be pinned from the armed side, where both answers are visible: the first
    // `true` carries the reader, and a second must move them zero pixels, or
    // every later render would re-assert the live edge over wherever the reader
    // had got to.
    const { el, onScroll } = armedThenScrolledUpOnAnIdleThread();

    setThreadLive(true);        // the wake: the round the observer missed
    expect(el.scrollTop).toBe(2500);

    // Now put the container somewhere the follow did not: with NO gesture
    // behind it, which is the iOS keyboard / app-resume case the follow
    // deliberately survives (the setup's own scroll is retired first, or its
    // coast would still be counting as the reader's and this live move would
    // disarm). Armed, live, and off the stamp is exactly the state a
    // re-asserting signal would trample.
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
    // The gap the submit's own claim covers, checked from this side too: the
    // write has to run through the seconds between the submit and the
    // projection catching up, or the reader would submit and watch the reply
    // grow in below them.
    const { el, onResize } = armedThenScrolledUpOnAnIdleThread();

    followContinuedThread();  // marks the thread live by itself
    vi.advanceTimersByTime(1500);
    el.scrollHeight = 4000;
    onResize();

    expect(el.scrollTop).toBe(3500);
  });
});

describe('the toggle remembers the last press as a seed', () => {
  /** The seed is what a thread with NO reading position starts as, and the only
   *  state the toggle can show in the compose view, which has no transcript.
   *  What it must never be is a mirror of the follow: a scroll retiring the ride
   *  on THIS thread is not the reader revising a standing preference, and a
   *  single accidental flick would otherwise turn it off for every future
   *  thread. */
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
  /** ONLY A SCROLL MAY DISARM. A collapse, an unfold, and the two
   *  transcript-wide turn controls are clicks on a control, and the reader who
   *  made one asked for more of the turn rather than for less of the ride.
   *
   *  They move the container all the same. `withScrollAnchor` pins the turn the
   *  click was on and writes the correction itself, and the two turn controls are
   *  transcript-wide, so every turn BELOW the anchored one grows too and the
   *  correction lands the reader short of the live edge. That looked exactly like
   *  the reader scrolling away, and retired a follow nobody touched.
   *
   *  The fake reproduces the shape rather than the DOM: `honourAnchoredMutation`
   *  is the entry point `withScrollAnchor` calls right after its own write, and
   *  the write is modelled by moving `_scrollTop` without counting it, since the
   *  app made it. */
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
    // A tween was tried here and is what the reader reported: the reveal grows
    // every turn above them too, so they were left a screenful short of the
    // live edge and the transcript then scrolled itself down. This write lands
    // in the same frame the caller unfreezes, before the paint the mutation
    // causes, and it keeps the newest content, which is what a riding reader is
    // looking at, exactly where it already was. See `honourAnchoredMutation`.
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
  /** The follow is one global that `focusThread` retires on every open, so the
   *  request used to end the moment the reader looked at another thread: they
   *  came back to the pixel offset the transcript had when they walked away, with
   *  everything the agent produced meanwhile below them. The request is recorded
   *  per thread as a reading position now (`hooks/useScrollMemory.ts`), and this
   *  is the entry point that resumes it. What that file pins is the RECORDING;
   *  what this pins is that resuming produces a real follow and not a one-shot
   *  landing at the bottom. */
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('lands on the live edge the thread has NOW, and keeps riding it', () => {
    // `.thread-content` is one element reused across threads, so on arrival it
    // holds the OUTGOING thread's offset: arming without writing would leave the
    // reader sitting there until the next growth round.
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
    // expansion's margin by construction, and the same reasoning applies to a
    // write the reader did not make with their finger. The restore goes through
    // `markFollowScroll`, which is `markNavigationScroll` plus the follow's own
    // position stamp.
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

describe('sending a message lands on the turn\'s agent status line', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('lands the reader who sent FROM the live edge, once their turn renders', () => {
    // The reported bug, and the ordinary case: sending from the bottom is what
    // most sends are. Being at the live edge WHEN THE SUBMIT IS MADE says nothing
    // about where the turn will sit, because the submit is what appends it. The
    // reader was left on the top of their own message with the status line below
    // the fold and nothing to take them there.
    //
    // The two halves of the fixture are the whole point, and the old fake had
    // them the wrong way round: park at the live edge, submit, and only THEN
    // grow the transcript with the turn.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const parked = atBottom(el);
    el.writes = 0;

    followSentMessage();
    expect(el.writes).toBe(0);      // nothing yet: the turn does not exist
    expect(el.scrollTop).toBe(parked);

    el.addUserMessage({ top: 2900, height: 120 }); // the row and its status line
    el.scrollHeight = 3400;                        // render below the old fold
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2548); // 3048 (status line bottom) - 500
    expect(el.scrollTop).toBeGreaterThan(parked);
  });

  it('still writes nothing when that turn lands fully in view', () => {
    // The other half, and where the deleted at-the-live-edge branch was RIGHT:
    // a reader whose new status line renders above the fold has nowhere to go.
    // Reached by measuring the target rather than by predicting it, which is the
    // difference that makes the case above work.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const parked = atBottom(el);
    el.writes = 0;

    followSentMessage();
    el.addUserMessage({ top: 2510, height: 20 }); // a one-line message, in view
    el.scrollHeight = 3000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
  });

  it('arms nothing, so the reply streaming in leaves that reader where they are', () => {
    // THE INVERSION. A send used to arm the standing follow, and the reader who
    // sent from the live edge was then carried down through the whole reply.
    // Riding is the chevron's request now, so the same reader is left at the
    // bottom of what they can see and the chevron is their way down.
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

  it('glides to the turn\'s agent status line, not to the end of the message', () => {
    // What a send actually asks. The reader wrote the message a second ago and
    // does not need to be shown it; what they are waiting for is whether the
    // agent took it, and that is the Requesting / Working row underneath. The
    // landing used to stop on the message's own bottom edge, which parked the
    // status line one row below the fold with nothing on screen saying anything
    // had happened.
    //
    // Anchored on an ELEMENT either way. A scrollHeight target would land at 2900
    // here, which is past the whole turn and hides the very thing they wrote.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();

    el.addUserMessage({ top: 2900, height: 120 }); // the optimistic row renders
    el.scrollHeight = 3400;                        // with the working indicator under it
    onResize();
    expect(el.scrollTop).toBe(500); // a glide, not a jump: nothing lands this frame

    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2548);                      // 3048 (status line bottom) - 500
    expect(el.scrollTop).not.toBe(2520);                  // NOT 3020, the message's own bottom
    expect(el.scrollTop).toBeLessThan(2900);              // NOT the transcript bottom either
    expect(2900).toBeGreaterThanOrEqual(el.scrollTop);    // the message is fully visible
    expect(3048).toBeLessThanOrEqual(el.scrollTop + el.clientHeight); // and so is the status line
  });

  it('WAITS for the response panel rather than settling on the message', () => {
    // The reported bug, 2026-08-10: "it scrolls to weird place or to end of
    // message, but not to agent response header". The optimistic row can arrive
    // a commit before the response panel under it, and the landing used to
    // resolve on the row. `landOnOwnTurn` asks its do-not-scroll-backwards test
    // once, at the moment it is called, so anchored on the message it either
    // declined outright or aimed one row short and settled there when the header
    // arrived after the tween had finished.
    //
    // So the landing does not start until the status line is in the DOM. The
    // round that has the row but not the header must move NOBODY.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();

    const sent = el.addUserMessage({ top: 2900, height: 120, status: false });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(500);  // nothing to aim at yet, so nobody moves
    expect(el.writes).toBe(0);

    el.mountStatusLine(sent);        // the response panel renders, which grows
    onResize();                      // the transcript, so a real RO round fires
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2548); // 3048 (status line bottom) - 500
  });

  it('moves NOBODY for a turn that never gets a status line', () => {
    // A queued follow-up shows its "Queued" tag in its own bubble and renders no
    // response panel at all. There is no status line to aim at, and the reader's
    // own message is not a substitute for one: what a submit asks is whether the
    // agent took it, and a queued turn's honest answer is "not yet". The landing
    // lapses on its deadline and writes nothing, which is what that deadline was
    // documented to be for all along.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();

    el.addUserMessage({ top: 2900, height: 120, status: false });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);
    el.scrollHeight = 4000; // the reply streams in under the queued row
    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(500);
  });

  it('a lapsed landing does not block the NEXT submit', () => {
    // The deadline is wall-clock, and the growth branch is the only thing that
    // was checking it, so a landing whose turn never renders a status line sat
    // on `_pendingLanding` for as long as nothing grew. A queued follow-up is
    // exactly that turn, and the reader's next submit then returned early
    // without installing its own resolver: it never landed. Found by the Codex
    // reviewer while hardening this change, and reachable BECAUSE the landing
    // now waits for a status line.
    // The second submit is a CARD, which resolves at submit time and glides
    // immediately: it never reaches the growth branch, so a stale pending
    // landing is the only thing that can stop it. That is what makes this the
    // shape that reproduces. A second SEND would be rescued by luck, because the
    // stale resolver is `awaitsNewTurn(lastUserMessage)` and the newer row
    // happens to satisfy it.
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

      expect(el.scrollTop).toBe(2228); // 2728 (the card's status line) - 500
    } finally {
      nowSpy.mockRestore();
    }
  });

  it('stops short of the prompt dissolve, by the clearance the row asks for', () => {
    // The container's bottom edge is not clear space: the prompt dissolve paints
    // a bg-coloured band over it, so a status line rested flush against that edge
    // is as invisible as one below the fold. The row names the room it needs as
    // `scroll-margin-bottom` (chat/response.css) and the landing reads the
    // resolved px, which is what this stands in for.
    const styles = vi.fn((el: any) => ({
      scrollMarginBottom: el.getBoundingClientRect().height === STATUS_LINE_HEIGHT ? '24px' : '0px',
    }));
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

      expect(el.scrollTop).toBe(2572); // 2548, plus the 24px the row asked for
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

    expect(el.scrollTop).toBe(2548);
  });

  it('then holds still, so the reply grows in UNDER the status line it landed', () => {
    // THE INVERSION. The landing used to be the first half of a ride: it put the
    // reader on their own turn and the follow then dragged them down through the
    // reply. The landing is the whole reaction now, so the status line stays
    // exactly where the glide left it and the answer arrives beneath it.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2548);
    el.writes = 0;

    for (const height of [5000, 9000]) {
      el.scrollHeight = height;
      onResize();
    }

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2548);
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
    // The cancel and the tween must not disagree. This used to come free from the
    // follow's disarm, because the send had armed one; with a submit arming
    // nothing the landing carries its own position stamp and its own cancel. A
    // chevron tap's tween, which arms nothing until it lands, still reaches its
    // target as it always has.
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
    // The pending phase has written nothing yet, so there is no write to read the
    // reader's gesture against: the stamp the submit takes at call time is what
    // makes their flick in that window cancel the landing rather than be
    // overridden by it a frame later.
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

  it('holds the glide on its last measured target when the message leaves the layout', () => {
    // Nothing cancels a tween when the reader opens another thread mid-glide, and
    // a detached node reports an all-zero rect. Subtracting the container's
    // BOTTOM edge from that would make the target a whole viewport negative and
    // scroll the thread to its top, over whatever else had just positioned it.
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
    sent.isConnected = false; // the reader opens another thread

    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBeGreaterThanOrEqual(midGlide);
    expect(el.scrollTop).toBeLessThanOrEqual(2548);
  });

  it('LAPSES and moves nobody when the message has no box to land on', () => {
    // THE INVERSION. The second and later queued follow-ups fold into a CLOSED
    // disclosure group, so the message the reader just sent has no rect. Past the
    // deadline that landing used to fall back to the live edge, because the send
    // had armed a follow that had to be honoured; with no arming there is nothing
    // to honour, and such a turn has no response panel and so no status line to
    // show them anyway. So the landing lapses and the reader is not moved.
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
    // scrollTop to hold the reader on the same content. That is the app, not the
    // reader, and the growth branch is standing down for the pending landing, so
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

    expect(el.scrollTop).toBe(2548); // the landing still happened
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

    expect(el.scrollTop).toBe(2548);
  });
});

describe('a send moves nobody only when the status line is already in view', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** The block above is the send earning its keep. This block is the case where
   *  the ask is already satisfied, and the point of keeping it is WHERE that is
   *  now decided.
   *
   *  It used to be two pre-emptive tests in `followSubmit`: "is the reader at the
   *  live edge" and "does this transcript hold a turn and scroll room"
   *  (`hasSomewhereToLand`). Both asked about the transcript AS IT STANDS in
   *  order to predict where a turn that had not rendered would sit, and both got
   *  the ordinary case wrong: a reader who sends from the bottom is at the live
   *  edge when asked and one commit later has the new status line below the fold.
   *  Reported twice on 2026-08-10.
   *
   *  Now there is ONE test and it is a measurement: `landOnOwnTurn` writes
   *  nothing when its target is at or behind the current position, asked per
   *  frame against the status line itself. Every case below reaches "moves
   *  nobody" through it rather than by being recognised in advance. */

  it('leaves the reader alone for the whole first reply in a brand-new thread', () => {
    // The reported bug, in the order the two call sites actually fire for one
    // send. `PromptInput.submit` runs first, from the compose view, where there
    // is no transcript to resolve at all. `addPendingMessage` runs second, by
    // which time the promoted thread has mounted an EMPTY one: it calls before
    // writing `threadMap`, precisely so the optimistic row has not rendered.
    followSentMessage();

    const el = makeEl({ scrollTop: 0, scrollHeight: 400, turns: 0 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    followSentMessage();
    el.writes = 0;

    el.addUserMessage({ top: 40, height: 120 }); // the optimistic row renders
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

  it('lands the FIRST turn of a brand-new thread when its status line is below the fold', () => {
    // The compose view renders the welcome message inside the same
    // `.thread-content`, and on a short viewport it has real scroll room. That
    // used to be answered with a pre-emptive "this transcript holds no turn, so
    // there is nowhere to take anybody", which is the same mistake as the
    // at-the-live-edge branch: it predicts where a turn that has not rendered
    // will sit. The first turn of a brand-new thread lands below the fold here
    // exactly like any other, and the reader is owed the same sight of the agent
    // taking it.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000, turns: 0 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 3100, height: 120 }); // the optimistic row and its
    el.scrollHeight = 3400;                        // Requesting row render
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2748); // 3248 (status line bottom) - 500

    // And it arms nothing on the way, so the reply that follows moves nobody.
    el.writes = 0;
    el.scrollHeight = 6000;
    onResize();
    expect(el.writes).toBe(0);
  });

  it('leaves the reader alone in an existing thread that fits on screen', () => {
    // Why the turn test cannot answer it either. There is a conversation here,
    // but the reader is at its bottom because there is nowhere else to be, not
    // because they chose to be, and what they just sent is already fully
    // visible. The reply grows past the fold below them.
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
    // the transcript's geometry is when they make it.
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

describe('answering a question card lands on the same status line', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** Submitting an answer IS a send: the reader handed the agent something and
   *  is owed the sight of it being picked up. Which of the three shapes they
   *  used must not be something they can feel in the scroll, so this whole block
   *  is the send's block above with the card in place of the message. The third
   *  shape, typing the answer, is literally a send and rides `followSentMessage`.
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
    // THE INVERSION. Answering armed the standing follow, so the resumed reply
    // carried the reader down through it. It arms nothing now.
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

  it('glides to the status line under the answered card, not to the card\'s own bottom', () => {
    // The same landing a send gets, for the same reason: the reader knows what
    // they just answered, and what they are owed is the agent picking it up
    // again. The card's PANEL is the fallback anchor, exactly as a send falls
    // back to its message panel; a scrollHeight target would land at 2500 here,
    // past the whole turn, and hide the thing they just answered.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followAnsweredQuestion('q1');
    expect(el.scrollTop).toBe(500); // a glide, not a jump

    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2228);           // 2728 (status line bottom) - 500
    expect(el.scrollTop).not.toBe(2200);       // NOT 2700, the card's own bottom
    expect(el.scrollTop).toBeLessThan(2500);   // NOT the transcript bottom either
  });

  it('survives the answered body replacing the live one, because it holds the PANEL', () => {
    // Why the landing anchors on `.initiator-panel` rather than the
    // `.question-body` inside it. Answering swaps `QuestionBody`'s live body for
    // `AnsweredBody`, a different component, so Preact unmounts the body node a
    // frame or two into the glide; the panel around it is the same vnode in the
    // same position and is reused. Anchored on the body, this glide would go
    // dead the moment the answer rendered and freeze wherever it had got to.
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

    expect(el.scrollTop).toBe(2228); // still lands on the card, not stranded
    expect(el.scrollTop).toBeGreaterThan(midGlide);
  });

  it('then holds still, so the reply grows in under the card', () => {
    // THE INVERSION, the answer's copy of it: the landing was the first half of a
    // ride and is the whole reaction now.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followAnsweredQuestion('q1');
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2228);
    el.writes = 0;

    el.scrollHeight = 5000;
    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2228);
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

    expect(el.scrollTop).toBe(2228);
  });
});

describe('a submit made while already riding the live edge goes to the bottom', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** The landing above is for a reader who had scrolled away: it shows them the
   *  agent taking what they submitted, with the reply growing underneath. A
   *  reader who is already RIDING the live edge asked for the opposite, and asked
   *  for it as a standing request: keep me at the bottom. Landing them on their
   *  own turn puts them off the bottom with the chevron on and the reply below
   *  the fold, which is the state they armed the follow to never be in.
   *
   *  This is the ONE branch a submit did not change, and it is the whole of what
   *  a submit and the standing follow still have to do with each other: the
   *  submit SERVES a request the chevron made, and never makes one.
   *
   *  Armed but off the live edge is an ordinary state rather than a corner. The
   *  growth branch stands down while a tween owns the scroll, so a reply that
   *  streams entirely during a glide leaves the reader parked above the bottom
   *  with the follow still armed and no growth left to carry them out of it.
   *  `ridingButParked` reproduces exactly that, and the reader's next submit is
   *  made from there. */
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
    // The target is re-read per frame, so a reply still streaming during the
    // glide is tracked rather than leaving the reader one screen short of it.
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
    onResize();                    // the send's landing glide starts, for 2548
    vi.advanceTimersByTime(60);

    followAnsweredQuestion('q1');  // and the reader answers the card mid-glide
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2228); // the CARD's status line, not the send's
  });

  it('a glide that already LANDED does not suppress the next submit', () => {
    // The "leave a live-edge glide alone" guard must describe a tween in flight
    // and not one that finished, or the flags a completed glide left behind
    // would answer for it and the next submit would move nobody.
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
    // THE INVERSION, and the largest one. `followResolvedPermission` called
    // itself "the one submit that ARMS NOTHING" and moved a reader who was
    // following nothing zero pixels, so deciding a card resumed the agent below
    // the fold with nothing on screen saying so. It is a submit like the others
    // now: the reader gets the status line of the turn they decided.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addPermissionCard({ requestId: 'p1', top: 2400, height: 300 });

    followResolvedPermission('p1');
    expect(el.scrollTop).toBe(500); // a glide, not a jump
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2228); // 2728 (status line bottom) - 500

    el.writes = 0;
    el.scrollHeight = 4000;
    onResize();
    expect(el.writes).toBe(0);       // and it armed nothing on the way
    expect(el.scrollTop).toBe(2228);
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

    expect(el.scrollTop).toBe(2228);            // p1's status line
    expect(el.scrollTop).not.toBe(4228);        // NOT p2's
    expect(el.scrollTop).toBeLessThan(5500);    // NOT the live edge
  });
});

describe('Continue after an abort is a submit too', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** The fourth submit, and the one that used to move nobody at all, not even a
   *  reader who was riding. Its turn does not exist when the button is pressed:
   *  the continuation renders as a fresh `ContinuationStarted` exchange, over
   *  SSE, after the POST. So it takes the send's deferred landing with a
   *  different notion of the turn it is waiting for, a `.chat-exchange` that is
   *  not the one that was last, since a continuation renders no user message. */

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

    expect(el.scrollTop).toBe(2228); // 2728 (status line bottom) - 500
  });

  it('arms nothing, so the resumed reply grows in under it', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followContinuedThread();
    el.addContinuationTurn({ top: 2400, height: 300 });
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2228);
    el.writes = 0;

    for (const height of [5000, 9000]) {
      el.scrollHeight = height;
      onResize();
    }

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2228);
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
 *  branches, so "one reaction everywhere" is checked rather than asserted four
 *  times in four dialects. The blocks above are each surface's own story (what
 *  its turn is, when it renders, what it falls back to); this is the matrix.
 *
 *  The geometry is shared on purpose: whichever surface it is, the turn the
 *  reader acted on spans 2400..2700 and its status line 2700..2728, so every
 *  landing in the table lands on 2228. A surface that answered differently would
 *  be doing something the others are not. */
const SUBMIT_SURFACES: Array<{
  name: string;
  /** Render what must be on screen BEFORE the submit, at `top`. The two deferred
   *  submits render nothing: their turn does not exist yet. */
  arrange: (el: any, top?: number) => void;
  /** Make the submit, then, for a deferred surface, render the turn it waits for
   *  at `top` and give the growth branch the round it lands on. */
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

  it('branch 1, the status line is already in view: writes nothing', () => {
    // A thread that fits on screen, with the turn INSIDE it, which is the only
    // way a short transcript can actually be laid out. The landing measures its
    // target, finds it behind the reader, and writes nothing. There is no
    // pre-emptive "this thread is too short" test any more, and there must not
    // be: the same test asked one commit earlier is what left a reader who sent
    // from the bottom of a long thread staring at their own message.
    const el = makeEl({ scrollTop: 0, scrollHeight: 400 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    arrange(el, 40);
    el.writes = 0;

    submit(el, onResize, 40);
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

    expect(el.scrollTop).toBe(2228); // 2728 (status line bottom) - 500
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

  /** Each of these growths had a `scrollToBottom()` call site before the pin was
   *  removed, so each is a regression test against the pin coming back. */
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
    // content at the bottom; being asked is the agent producing it, and the two
    // must not be confused just because the same element is involved.
    unmovedBy((el) => {
      el.addQuestionCard({ toolUseId: 'q1', top: 2700, height: 300 });
      el.scrollHeight = 3300;
    });
  });

  /** Why this stays a source scan and not a behavioural test: the failure is a
   *  NEW store action reaching for the live edge to "make sure you see it", and
   *  no behavioural test can fail for a call site it does not know exists. Five
   *  modules under `store/actions` carried such a call before the pin was
   *  removed. A site that genuinely must arm the follow will fail here: say why
   *  at the site and list it below rather than weakening the scan. */
  it('no store action outside the send path reaches for the live edge', () => {
    const here = dirname(fileURLToPath(import.meta.url));
    const ACTIONS_DIR = resolve(here, '../../../store/actions');
    /** The sanctioned callers, per module. `chat.ts` owns the send's own call;
     *  `threads.ts` retires the follow when the reader opens another thread.
     *  Every other submit has NO sanctioned store-action caller at all, and the
     *  reason is the same one three times: the store action is the TRANSPORT,
     *  which also carries decisions nobody watched happen, while the reader's own
     *  tap lives in the component. The two card-submitted answers call
     *  `followAnsweredQuestion` from `QuestionCard` / `PromptInput` and
     *  `answerThreadQuestion` in `chat-claude-code.ts` deliberately does not; the
     *  three permission-shaped cards call `followResolvedPermission` from
     *  `PermissionCard` and `store/actions/permissions.ts` does not; Continue
     *  calls `followContinuedThread` from `chat-exchange-parts.tsx` and the
     *  `continueThread` API client does not. */
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
   *  `scrollState.ts` rather than trusting the module header's prose: the follow
   *  toggle (the reader making the request) and the resume (replaying one they
   *  made in this thread earlier, which can only ever be a toggle request, since
   *  only the toggle records one).
   *
   *  The chevrons are named explicitly as NON-arming, because that is the change
   *  most likely to be undone by someone reading `scrollToBottomAnimated` and
   *  thinking "go to the bottom" obviously means "and stay". It does not, and
   *  the reason is in the module header: one button cannot be go-there,
   *  stay-here and stop-staying at once. */
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
   *  two entry points that let the reading position record and resume a follow,
   *  and it has to reach the whole tree rather than `store/actions` alone,
   *  because neither belongs to a store action at all.
   *
   *  `resumeFollowingBottom` re-arms a follow the reader is not making right now,
   *  which is safe for exactly one reason: it fires only for a thread whose
   *  RECORDED reading position is the live edge, so only for a thread the reader
   *  armed themselves. A second caller would lose that guarantee and become a
   *  third arming point the rule above exists to prevent. `onFollowArmed`
   *  deliberately broadcasts the arm and not the retirement, which is what lets
   *  `focusThread` retire the follow without erasing the record of it; a second
   *  subscriber would be a second thing acting on half a lifecycle.
   *
   *  It matches a CALL shape rather than a bare mention, unlike the scan above.
   *  That scan wants the wider net, because a store action that so much as
   *  imports an arming entry point is already reaching for the live edge. These
   *  two are the opposite: they are the mechanism the follow's own lifetime is
   *  built from, so the modules that participate in it name them in prose while
   *  calling neither, and `focusThread`'s comment naming `onFollowArmed` is
   *  exactly the explanation a future reader needs there. */
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
