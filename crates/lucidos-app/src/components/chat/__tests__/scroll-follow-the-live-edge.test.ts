import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readdirSync, readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, join, resolve } from 'node:path';

if (typeof globalThis.HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class HTMLElement {};
}

import {
  awayFromBottom,
  showScrollToBottom,
  followAnsweredQuestion,
  followCanceledTurn,
  followConfirmedCard,
  followContinuedThread,
  followResolvedPermission,
  followSentMessage,
  followSideQuestion,
  followLiveEdgeSeed,
  followSeedFromStored,
  followingLiveEdge,
  honourAnchoredMutation,
  DRAWN_ROW_SELECTOR,
  classifyScroll,
  holdAcrossRelayout,
  markAnchorScroll,
  markNavigationScroll,
  markRevealScroll,
  makeScrollObservers,
  onRebasedScroll,
  readerGestureForTest,
  resumeFollowingBottom,
  scrollToBottom,
  scrollToBottomAnimated,
  scrollToTop,
  setActiveScrollElement,
  setFollowLiveEdge,
  setTranscriptLive,
  stopFollowingBottom,
} from '../scrollState';
import { postClientLog } from '../../../utils/clientLog';
import { osReducesMotion } from '../../../utils/motion';

vi.mock('../../../utils/clientLog', () => ({ postClientLog: vi.fn() }));


const STATUS_LINE_HEIGHT = 28;


function cardClassList(kind: 'question' | 'permission', owner: { answered?: boolean; terminated?: boolean }) {
  return {
    contains: (name: string) => (
      name === `${kind}-body-answered` ? !!owner.answered
        : name === `${kind}-body-terminated` ? !!owner.terminated
          : false
    ),
  };
}

function makeEl(opts: {
  scrollTop: number;
  scrollHeight: number;
  clientHeight?: number;
  panels?: Array<{ top: number; height: number }>;
  turns?: number;
}) {
  const panels: any[] = [];
  const questionCards: any[] = [];
  const permissionCards: any[] = [];
  const sideQuestionCards: any[] = [];
  const turns: any[] = [];
  const styleProps = new Map<string, string>();
  const el: any = {
    parentElement: null,
    children: [],
    style: {
      setProperty: (name: string, value: string) => { styleProps.set(name, value); },
      getPropertyValue: (name: string) => styleProps.get(name) ?? '',
    },
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
    querySelectorAll: (list: string) => list.split(',').map(s => s.trim()).flatMap((selector) => (
      selector === '.initiator-panel-user' ? panels
        : selector === '.question-body' ? questionCards
          : selector === '.permission-body' ? permissionCards
            : selector === '.permission-body[data-tool-use-id]'
              ? permissionCards.filter(card => card.getAttribute('data-tool-use-id') != null)
              : selector === '.chat-exchange' ? turns
                : selector === '[data-side-question-id]' ? sideQuestionCards
                  : []
    )),
    addSideQuestionCard(p: { id: string; top: number; height: number }) {
      const card: any = {
        isConnected: true,
        getAttribute: (name: string) => (name === 'data-side-question-id' ? p.id : null),
        getBoundingClientRect: () => ({
          width: 800, height: p.height, top: p.top - el.scrollTop, bottom: p.top + p.height - el.scrollTop, left: 0, right: 800,
        }),
      };
      sideQuestionCards.push(card);
      return card;
    },
    addUserMessage(p: { top: number; height: number; visible?: boolean; status?: boolean; queued?: boolean }) {
      const turn = makeTurn(p.top, p.height);
      if (p.queued) turn.queued = true;
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
    mountStatusLine(panel: any) {
      const rect = panel.getBoundingClientRect();
      return mountStatusLine(panel.turn, rect.bottom + el.scrollTop);
    },
    addQuestionCard(p: { toolUseId: string; top: number; height: number; status?: boolean; rows?: number; terminated?: boolean }) {
      return addCard(questionCards, 'question', 'data-tool-use-id', p.toolUseId, p);
    },
    addPermissionCard(p: { requestId: string; toolUseId?: string; top: number; height: number; status?: boolean; rows?: number }) {
      const card = addCard(permissionCards, 'permission', 'data-request-id', p.requestId, p);
      const byRequest = card.body.getAttribute;
      card.body.getAttribute = (name: string) => (name === 'data-tool-use-id' ? p.toolUseId ?? null : byRequest(name));
      return card;
    },
    addContinuationTurn(p: { top: number; height: number; status?: boolean }) {
      const turn = makeTurn(p.top, p.height);
      if (p.status !== false) mountStatusLine(turn, p.top + p.height);
      return turn;
    },
    answerQuestionCard(card: { body: any; panel: any }) {
      card.body.isConnected = false;
      questionCards.splice(questionCards.indexOf(card.body), 1);
      const answered: any = {
        ...card.body,
        isConnected: true,
        answered: true,
        closest: (sel: string) => (
          sel === '.initiator-panel' ? card.panel
            : sel === '.chat-exchange' ? card.panel.turn : null
        ),
      };
      answered.classList = cardClassList('question', answered);
      questionCards.push(answered);
      return { body: answered, panel: card.panel };
    },
    drawResponseRow(turn: any) {
      turn.drawn.push({ isConnected: true });
      return turn;
    },
    drawIntoNewestTurn() {
      return el.drawResponseRow(turns[turns.length - 1]);
    },
    addAnchorChild(top: number) {
      const child = { isConnected: true, _top: top, getBoundingClientRect() { return { width: 800, height: 200, top: this._top, bottom: this._top + 200, left: 0, right: 800 }; } };
      el.children.push(child);
      return child;
    },
  };
  function makeTurn(top = 0, height = 100) {
    const turn: any = {
      parentElement: null,
      isConnected: true,
      statusLine: null,
      drawn: [] as any[],
      closest: (sel: string) => (sel === '.chat-exchange' ? turn : null),
      matches: (sel: string) => sel === '.chat-exchange',
      queued: false,
      querySelector: (sel: string) => (
        sel === '.response-header' ? turn.statusLine
          : sel === '.exchange-status-queued' ? (turn.queued ? { isConnected: true } : null)
            : null
      ),
      querySelectorAll: (sel: string) => (sel === DRAWN_ROW_SELECTOR ? turn.drawn : []),
      getBoundingClientRect: () => ({
        width: 800, height, top: top - el.scrollTop, bottom: top + height - el.scrollTop, left: 0, right: 800,
      }),
    };
    turns.push(turn);
    return turn;
  }

  function addCard(
    into: any[],
    kind: 'question' | 'permission',
    attr: string,
    value: string,
    p: { top: number; height: number; status?: boolean; rows?: number; terminated?: boolean },
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
    const body: any = {
      parentElement: null,
      isConnected: true,
      answered: false,
      terminated: !!p.terminated,
      getAttribute: (name: string) => (name === attr && !p.terminated ? value : null),
      closest: (sel: string) => (
        sel === '.initiator-panel' ? panel : sel === '.chat-exchange' ? turn : null
      ),
      getBoundingClientRect: rect(p.top + 20, Math.max(0, p.height - 40)),
    };
    body.classList = cardClassList(kind, body);
    if (p.status) mountStatusLine(turn, p.top + p.height);
    if (p.rows) for (let i = 0; i < p.rows; i++) turn.drawn.push({ isConnected: true });
    into.push(body);
    return { body, panel };
  }

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

function drawWholeReply(el: any, turn: any) {
  class LaidOutNode {}
  const text = { textContent: '\n  1. One' };
  vi.stubGlobal('Node', LaidOutNode);
  vi.stubGlobal('NodeFilter', { SHOW_TEXT: 4 });
  vi.stubGlobal('document', {
    hasFocus: () => true,
    createTreeWalker: () => { let walked = false; return { nextNode: () => (walked ? null : (walked = true, text)) }; },
    createRange: () => ({
      setStart() {},
      setEnd() {},
      getClientRects: () => [{ bottom: 3100 - el.scrollTop }],
    }),
  });
  turn.drawn.push(Object.assign(new LaidOutNode(), {
    isConnected: true,
    getBoundingClientRect: () => ({ bottom: 4000 - el.scrollTop }),
  }));
  el.scrollHeight = 4000;
}

function atBottom(el: any) {
  el._scrollTop = Math.max(0, el.scrollHeight - el.clientHeight);
  return el._scrollTop;
}

function resetFollow() {
  stopFollowingBottom();
  setTranscriptLive(true);
  setActiveScrollElement(null);
  awayFromBottom.value = false;
  readerGestureForTest(null, false);
}

function readerScrollsTo(el: { scrollTop: number }, top: number, onScroll: () => void) {
  readerGestureForTest(el as unknown as HTMLElement);
  el.scrollTop = top;
  onScroll();
}

function answerAndConfirm(toolUseId: string) {
  followAnsweredQuestion(toolUseId);
  followConfirmedCard(toolUseId);
}

function decideAndConfirm(requestId: string) {
  followResolvedPermission(requestId);
  followConfirmedCard(requestId);
}

describe('the follow toggle arms a standing follow', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('keeps the reader at the live edge as the reply keeps growing, not just once', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2500);

    for (const [height, expected] of [[3400, 2900], [4000, 3500], [9000, 8500]]) {
      el.scrollHeight = height;
      onResize();
      expect(el.scrollTop).toBe(expected);
    }
    expect(awayFromBottom.value).toBe(false);
  });

  it('leaves a reader who never pressed it alone under exactly the same growth', () => {
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
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    el.scrollHeight = 3400;
    onScroll();

    el.scrollHeight = 3800;
    onResize();
    expect(el.scrollTop).toBe(3300);
  });

  it('is retired by a scroll up, and later growth then leaves the reader alone', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    setFollowLiveEdge(true);
    readerScrollsTo(el, 2000, onScroll);

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
    expect(el.scrollTop).toBe(0);
  });

  it('survives everything that is not a scroll: a card resolving, expanding a turn', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);

    el.scrollHeight = 2800;
    onResize();
    onScroll();
    expect(el.scrollTop).toBe(2300);

    el.scrollHeight = 3600;
    onResize();
    expect(el.scrollTop).toBe(3100);
  });
});

describe('the chevron navigates and arms nothing', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });


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
    expect(awayFromBottom.value).toBe(true);
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
    expect(el.scrollTop).toBe(100);
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

    el.scrollHeight = 4000;
    onResize();
    expect(el.scrollTop).toBe(3500);
  });

  it('OFF disarms and writes NO scroll, leaving the reader where they read', () => {
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
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    expect(followingLiveEdge.value).toBe(true);

    readerScrollsTo(el, 1200, onScroll);

    expect(followingLiveEdge.value).toBe(false);
  });
});

describe('a scroll retires the follow only when the READER made it', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  function riding() {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000, clientHeight: 500 });
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    expect(followingLiveEdge.value).toBe(true);
    return { el, ...observers };
  }

  it('survives the iOS keyboard opening under a streaming reply', () => {
    const { el, onScroll, onResize } = riding();

    el.clientHeight = 200;
    onResize();
    el.scrollTop = 1400;
    onScroll();

    expect(followingLiveEdge.value).toBe(true);
  });

  it('survives the keyboard closing again', () => {
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
    const { el, onScroll } = riding();

    el.scrollTop = 0;
    onScroll();

    expect(followingLiveEdge.value).toBe(true);
  });

  it('survives the platform scrolling the container for any other reason', () => {
    const { el, onScroll } = riding();

    el.scrollTop = 700;
    onScroll();
    el.scrollTop = 1900;
    onScroll();

    expect(followingLiveEdge.value).toBe(true);
  });

  it('ends on the reader flicking, and on every other way they scroll', () => {
    const { el, onScroll } = riding();

    readerScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(false);
  });

  it('ends on the coast after the finger lifts, not just under it', () => {
    const { el, onScroll } = riding();

    readerGestureForTest(el);
    vi.advanceTimersByTime(200);
    el.scrollTop = 900;
    onScroll();

    expect(followingLiveEdge.value).toBe(false);
  });

  it('ends on the up chevron, which is a press rather than a gesture', () => {
    riding();

    scrollToTop();

    expect(followingLiveEdge.value).toBe(false);
  });

  it('keeps the ride when the platform scrolls the container to the top', () => {
    const { el, onScroll } = riding();

    el.scrollTop = 0;
    onScroll();

    expect(followingLiveEdge.value).toBe(true);
  });

  it('lets the coast lapse, so a scroll long after the flick is the platform again', () => {
    const { el, onScroll } = riding();

    readerGestureForTest(el);
    vi.advanceTimersByTime(5000);
    el.scrollTop = 900;
    onScroll();

    expect(followingLiveEdge.value).toBe(true);
  });

  it('does not read a PRESS inside the transcript as a scroll', () => {
    const { el, onScroll } = riding();

    readerGestureForTest(el, false);
    el.scrollTop = 900;
    onScroll();

    expect(followingLiveEdge.value).toBe(true);
  });

  it('forgets a gesture made on a DIFFERENT transcript', () => {
    const { el, onScroll } = riding();
    const other = makeEl({ scrollTop: 0, scrollHeight: 3000 });

    readerGestureForTest(other);
    el.scrollTop = 900;
    onScroll();

    expect(followingLiveEdge.value).toBe(true);
  });
});

describe('the follow puts the reader back when the PLATFORM moves them', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  function ridingAndAnchored() {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000, clientHeight: 500 });
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    observers.onScroll();
    expect(el.scrollTop).toBe(2500);
    expect(followingLiveEdge.value).toBe(true);
    el.writes = 0;
    return { el, ...observers };
  }

  function platformScrollsTo(el: { scrollTop: number }, top: number, onScroll: () => void) {
    vi.advanceTimersByTime(200);
    el.scrollTop = top;
    onScroll();
  }

  it('puts them back after the iOS keyboard adjusts the offset on its own', () => {
    const { el, onScroll, onResize } = ridingAndAnchored();

    el.clientHeight = 200;
    onResize();
    expect(el.scrollTop).toBe(2800);

    platformScrollsTo(el, 1400, onScroll);

    expect(el.scrollTop).toBe(2800);
    expect(followingLiveEdge.value).toBe(true);
    expect(awayFromBottom.value).toBe(false);
  });

  it('puts them back after an app resume restores an offset nobody wrote', () => {
    const { el, onScroll } = ridingAndAnchored();

    platformScrollsTo(el, 0, onScroll);

    expect(el.scrollTop).toBe(2500);
    expect(awayFromBottom.value).toBe(false);
  });

  it('does it on an IDLE thread too, because the platform moves them either way', () => {
    const { el, onScroll } = ridingAndAnchored();

    platformScrollsTo(el, 0, onScroll);

    expect(el.scrollTop).toBe(2500);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('keeps leaving them there when the platform moves them again afterwards', () => {
    const { el, onScroll } = ridingAndAnchored();
    readerScrollsTo(el, 900, onScroll);

    readerGestureForTest(null, false);
    platformScrollsTo(el, 400, onScroll);

    expect(el.scrollTop).toBe(400);
  });

  it('does not undo the reader flicking away, on any thread', () => {
    const { el, onScroll } = ridingAndAnchored();

    readerScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(false);
    expect(el.scrollTop).toBe(900);
  });

  it('writes once, and its own trailing scroll event writes nothing', () => {
    const { el, onScroll } = ridingAndAnchored();

    platformScrollsTo(el, 0, onScroll);
    expect(el.writes).toBe(2);

    onScroll();

    expect(el.writes).toBe(2);
    expect(el.scrollTop).toBe(2500);
  });

  it('moves an UNARMED reader nowhere, however the platform scrolls them', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000, clientHeight: 500 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);
    atBottom(el);
    onScroll();
    el.writes = 0;

    platformScrollsTo(el, 900, onScroll);

    expect(el.scrollTop).toBe(900);
    expect(el.writes).toBe(1);
  });

  it("leaves a NAVIGATION of the app's own where it put the reader, and retires the ride", () => {
    const { el, onScroll } = ridingAndAnchored();

    markNavigationScroll(el, 0);
    onScroll();

    expect(el.scrollTop).toBe(0);
    expect(followingLiveEdge.value).toBe(false);
  });

  it("leaves a REVEAL of the app's own alone, though it writes no scrollTop", () => {
    const { el, onScroll } = ridingAndAnchored();

    vi.advanceTimersByTime(200);
    el.scrollTop = 900;
    markRevealScroll(el);
    onScroll();

    expect(el.scrollTop).toBe(900);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('and stops deferring to it once the window has passed', () => {
    const { el, onScroll } = ridingAndAnchored();

    markNavigationScroll(el, 0);
    platformScrollsTo(el, 0, onScroll);

    expect(el.scrollTop).toBe(2500);
  });

  function platformScrollsNowTo(el: { scrollTop: number }, top: number, onScroll: () => void) {
    el.scrollTop = top;
    onScroll();
  }

  it("corrects one landing inside the ride's own write window", () => {
    const { el, onScroll, onResize } = ridingAndAnchored();

    el.scrollHeight = 4000;
    onResize();
    expect(el.scrollTop).toBe(3500);

    platformScrollsNowTo(el, 900, onScroll);

    expect(el.scrollTop).toBe(3500);
    expect(followingLiveEdge.value).toBe(true);
    expect(classifyScroll(el).followSurvives).toBe(true);
  });

  it('and does not strand them for every round after it', () => {
    const { el, onScroll, onResize } = ridingAndAnchored();

    el.scrollHeight = 4000;
    onResize();
    platformScrollsNowTo(el, 900, onScroll);

    el.scrollHeight = 5000;
    onResize();

    expect(el.scrollTop).toBe(4500);
    expect(followingLiveEdge.value).toBe(true);
  });
});

describe('a carrying ride verifies its own landing', () => {
  beforeEach(() => {
    resetFollow();
    vi.useFakeTimers();
    vi.mocked(postClientLog).mockClear();
  });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  function riding() {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000, clientHeight: 500 });
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    observers.onScroll();
    expect(el.scrollTop).toBe(2500);
    el.writes = 0;
    return { el, ...observers };
  }

  function growsAndLandsShort(el: any, onResize: () => void, drawn: number, real: number) {
    el.scrollHeight = drawn;
    onResize();
    el.scrollHeight = real;
  }

  it('writes again on the next frame when its own write came to rest short', () => {
    const { el, onResize } = riding();

    growsAndLandsShort(el, onResize, 4000, 4600);
    expect(el.scrollTop).toBe(3500);

    vi.advanceTimersByTime(20);

    expect(el.scrollTop).toBe(4100);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('settles the chevron with it, so nothing is lit over a ride that landed', () => {
    const { el, onScroll, onResize } = riding();

    growsAndLandsShort(el, onResize, 4000, 4600);
    onScroll();
    expect(awayFromBottom.value).toBe(true);

    vi.advanceTimersByTime(20);

    expect(awayFromBottom.value).toBe(false);
  });

  it('writes nothing more when the round landed where it aimed', () => {
    const { el, onResize } = riding();

    el.scrollHeight = 4000;
    onResize();
    const writes = el.writes;

    vi.advanceTimersByTime(20);

    expect(el.writes).toBe(writes);
    expect(el.scrollTop).toBe(3500);
  });

  it('corrects once and polls for nothing after that', () => {
    const { el, onResize } = riding();

    growsAndLandsShort(el, onResize, 4000, 4600);
    vi.advanceTimersByTime(20);
    expect(el.scrollTop).toBe(4100);

    el.scrollHeight = 5200;
    const writes = el.writes;
    vi.advanceTimersByTime(200);

    expect(el.writes).toBe(writes);
  });

  it('a TURN CONTROL press that leaves the reader short retires the ride, writing nothing', () => {
    const { el } = riding();

    el.scrollHeight = 4600;
    el._scrollTop = 3500;
    const writes = el.writes;
    honourAnchoredMutation(el);
    vi.advanceTimersByTime(20);

    expect(el.scrollTop).toBe(3500);
    expect(el.writes).toBe(writes);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('a TURN CONTROL press during the ride\'s own glide keeps the ride', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000, clientHeight: 500 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);

    honourAnchoredMutation(el);
    expect(followingLiveEdge.value).toBe(true);

    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2500);
  });

  it('an anchor write\'s own scroll event during the ride\'s glide keeps the ride', () => {
    setTranscriptLive(false);
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000, clientHeight: 500 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);

    el.scrollHeight = 5000;
    markAnchorScroll(el, 2100);
    onScroll();
    expect(followingLiveEdge.value).toBe(true);

    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(4500);
  });

  it('a glide carries an anchor write\'s shift instead of dropping it', () => {
    setTranscriptLive(false);
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000, clientHeight: 500 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(50);
    const before = el.scrollTop;
    expect(before).toBeGreaterThan(100);

    el.scrollHeight = 5000;
    markAnchorScroll(el, before + 2000);
    onScroll();
    vi.advanceTimersByTime(17);

    expect(el.scrollTop).toBeGreaterThanOrEqual(before + 2000);
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(4500);
  });

  it('a TURN CONTROL press that leaves the reader on the edge keeps the ride', () => {
    const { el } = riding();

    el.scrollHeight = 2400;
    el._scrollTop = 1900;
    honourAnchoredMutation(el);

    expect(followingLiveEdge.value).toBe(true);
  });

  it('stands down when the reader takes over between the write and the frame', () => {
    const { el, onScroll, onResize } = riding();

    growsAndLandsShort(el, onResize, 4000, 4600);
    readerScrollsTo(el, 1200, onScroll);
    const writes = el.writes;

    vi.advanceTimersByTime(20);

    expect(el.scrollTop).toBe(1200);
    expect(el.writes).toBe(writes);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('stands down when anything else has moved the container since', () => {
    const { el, onResize } = riding();

    growsAndLandsShort(el, onResize, 4000, 4600);
    el._scrollTop = 1200;
    const writes = el.writes;

    vi.advanceTimersByTime(20);

    expect(el.scrollTop).toBe(1200);
    expect(el.writes).toBe(writes);
  });

  it('stands down once the ride is retired', () => {
    const { el, onResize } = riding();

    growsAndLandsShort(el, onResize, 4000, 4600);
    stopFollowingBottom();
    const writes = el.writes;

    vi.advanceTimersByTime(20);

    expect(el.scrollTop).toBe(3500);
    expect(el.writes).toBe(writes);
  });

  it('says so ONCE for the ride, however many rounds land short', () => {
    const { el, onResize } = riding();

    growsAndLandsShort(el, onResize, 4000, 4600);
    vi.advanceTimersByTime(20);
    expect(postClientLog).toHaveBeenCalledTimes(1);

    growsAndLandsShort(el, onResize, 5200, 5800);
    vi.advanceTimersByTime(20);

    expect(postClientLog).toHaveBeenCalledTimes(1);
  });

  it('says nothing at all when every round lands where it aimed', () => {
    const { el, onResize } = riding();

    el.scrollHeight = 4000;
    onResize();
    vi.advanceTimersByTime(20);

    expect(postClientLog).not.toHaveBeenCalled();
  });

  it('names the shortfall and carries nothing of the reader\'s', () => {
    const { el, onResize } = riding();

    growsAndLandsShort(el, onResize, 4000, 4600);
    vi.advanceTimersByTime(20);

    expect(postClientLog).toHaveBeenCalledWith('follow', 'short', {
      short: 600, edge: 4100, view: 500,
    });
  });

  it('retires the ride on an anchor write that leaves the reader off the edge', () => {
    const { el, onScroll } = riding();

    markAnchorScroll(el, 900);
    onScroll();

    expect(el.scrollTop).toBe(900);
    expect(followingLiveEdge.value).toBe(false);
  });
});

describe('a reader who scrolls away ends the follow on a LIVE thread', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  function armedThenScrolledUp() {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    readerScrollsTo(el, 800, observers.onScroll);
    return { el, ...observers };
  }

  it('turns the toggle off, and later growth leaves the reader where they are', () => {
    const { el, onResize } = armedThenScrolledUp();

    expect(followingLiveEdge.value).toBe(false);
    el.writes = 0;

    el.scrollHeight = 4000;
    onResize();
    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(800);
    expect(awayFromBottom.value).toBe(true);
  });

  it('a submit after the scroll still goes to the live edge, as a landing', () => {
    const { el } = armedThenScrolledUp();
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    answerAndConfirm('q1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2500);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('scrolling away right after a submit disarms too', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    atBottom(el);
    setFollowLiveEdge(true);

    answerAndConfirm('q1');
    readerScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(false);
    el.writes = 0;

    el.scrollHeight = 9000;
    onResize();
    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(900);
  });
});

describe('a scroll turns the follow off only while the thread is live', () => {
  beforeEach(() => { resetFollow(); setTranscriptLive(false); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  function parkedAt800() {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    readerScrollsTo(el, 800, observers.onScroll);
    return { el, ...observers };
  }

  it('keeps the toggle lit on a waiting thread, and leaves the reader where they scrolled', () => {
    const { el } = parkedAt800();

    expect(followingLiveEdge.value).toBe(true);
    expect(el.scrollTop).toBe(800);
  });

  it('moves a parked reader nowhere on growth, a box change or a platform scroll', () => {
    const { el, onScroll, onResize } = parkedAt800();
    el.writes = 0;

    el.scrollHeight = 4000;
    onResize();
    el.clientWidth = 600;
    onResize();
    el.scrollTop = 700;
    el.writes = 0;
    onScroll();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(700);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('glides the parked reader back to the live edge when the thread goes live', () => {
    const { el, onResize } = parkedAt800();

    setTranscriptLive(true);
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2500);
    expect(followingLiveEdge.value).toBe(true);

    el.scrollHeight = 3400;
    onResize();
    expect(el.scrollTop).toBe(2900);
  });

  it('catches up on the first growth when the thread went live with no transcript showing', () => {
    const { el, onResize } = parkedAt800();
    setActiveScrollElement(null);

    setTranscriptLive(true);
    expect(el.scrollTop).toBe(800);

    setActiveScrollElement(el);
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2900);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('rides again when the reader scrolls back down to the edge themselves', () => {
    const { el, onScroll, onResize } = parkedAt800();

    readerScrollsTo(el, 2500, onScroll);
    el.scrollHeight = 3400;
    onResize();

    expect(el.scrollTop).toBe(2900);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('stays parked when a shrink clamps the reader onto the edge', () => {
    const { el, onScroll, onResize } = parkedAt800();
    readerGestureForTest(null, false);

    el.scrollHeight = 1200;
    el.scrollTop = 700;
    onScroll();
    el.writes = 0;
    el.scrollHeight = 1600;
    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(700);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('rides again on a submit, which takes the reader to the live edge', () => {
    const { el, onResize } = parkedAt800();
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    answerAndConfirm('q1');
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2500);

    el.scrollHeight = 3400;
    onResize();
    expect(el.scrollTop).toBe(2900);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('rides on to the live edge when a cancel follows a flick, through its coast', () => {
    const { el, onScroll } = parkedAt800();
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    readerScrollsTo(el, 2300, onScroll);
    expect(followingLiveEdge.value).toBe(true);

    followCanceledTurn('q1');
    vi.advanceTimersByTime(30);
    el.scrollTop = el.scrollTop + 5;
    onScroll();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2500);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('still parks on a NEW gesture made after the submit', () => {
    const { el, onScroll } = parkedAt800();
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followCanceledTurn('q1');
    vi.advanceTimersByTime(30);
    readerScrollsTo(el, 900, onScroll);
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(900);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('records the parked reader\'s place, not the live edge', () => {
    const { el } = parkedAt800();

    expect(classifyScroll(el as any).followSurvives).toBe(false);
  });

  it('still turns off on a press of the lit toggle, writing nothing', () => {
    const { el } = parkedAt800();
    el.writes = 0;

    setFollowLiveEdge(false);

    expect(followingLiveEdge.value).toBe(false);
    expect(el.writes).toBe(0);
  });

  it('turns off on the same scroll once the thread is live', () => {
    const { el, onScroll, onResize } = parkedAt800();
    setTranscriptLive(true);
    vi.advanceTimersByTime(1500);

    readerScrollsTo(el, 800, onScroll);
    expect(followingLiveEdge.value).toBe(false);

    el.writes = 0;
    el.scrollHeight = 3400;
    onResize();
    expect(el.writes).toBe(0);
  });
});

describe('an armed reader is kept on the live edge whatever the thread is doing', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });


  function armedAtTheEdgeOnAnIdleThread() {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    atBottom(el);
    observers.onScroll();
    setFollowLiveEdge(true);
    expect(followingLiveEdge.value).toBe(true);
    el.writes = 0;
    return { el, ...observers };
  }

  it('keeps them on the edge when a question card mounts under them', () => {
    const { el, onResize } = armedAtTheEdgeOnAnIdleThread();

    el.scrollHeight = 3400;
    onResize();

    expect(el.scrollTop).toBe(2900);
    expect(awayFromBottom.value).toBe(false);
  });

  it('keeps them there across round after round of it', () => {
    const { el, onResize } = armedAtTheEdgeOnAnIdleThread();

    for (const [height, edge] of [[3400, 2900], [3600, 3100], [4000, 3500]]) {
      el.scrollHeight = height;
      onResize();
      expect(el.scrollTop).toBe(edge);
    }
    expect(awayFromBottom.value).toBe(false);
  });

  it('does it in the resize round itself, so nothing is painted short of the edge', () => {
    const { el, onResize } = armedAtTheEdgeOnAnIdleThread();

    el.scrollHeight = 3400;
    onResize();

    expect(el.scrollTop).toBe(el.scrollHeight - el.clientHeight);
    expect(el.writes).toBe(1);
  });

  it('lets go the moment they scroll off the edge, and the toggle goes with it', () => {
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
    expect(followingLiveEdge.value).toBe(false);
  });

  it('leaves a TURN CONTROL where it was pressed, growth round included', () => {
    const { el, onResize } = armedAtTheEdgeOnAnIdleThread();

    el.scrollHeight = 5000;
    markAnchorScroll(el, 3000);
    honourAnchoredMutation(el);

    expect(el.scrollTop).toBe(3000);

    onResize();

    expect(el.scrollTop).toBe(3000);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('agrees even when the correction moved nobody', () => {
    const { el, onResize } = armedAtTheEdgeOnAnIdleThread();

    el.scrollHeight = 5000;
    markAnchorScroll(el, el.scrollTop);
    honourAnchoredMutation(el);
    el.writes = 0;

    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2500);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('moves an UNARMED reader on the edge zero pixels under the same growth', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    atBottom(el);
    onScroll();
    el.writes = 0;

    el.scrollHeight = 3400;
    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2500);
    expect(awayFromBottom.value).toBe(true);
  });
});

describe('the toggle remembers the last press as a seed', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); setFollowLiveEdge(false); });

  it('reads a device that has pressed nothing as ARMED', () => {
    expect(followSeedFromStored(null)).toBe(true);
    expect(followSeedFromStored('')).toBe(true);
  });

  it('reads a stored disarm press as disarmed, and an arm press as armed', () => {
    expect(followSeedFromStored('false')).toBe(false);
    expect(followSeedFromStored('true')).toBe(true);
  });

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

    expect(followingLiveEdge.value).toBe(false);
    expect(followLiveEdgeSeed.value).toBe(true);
  });

  it('is NOT touched by the retire a thread switch makes', () => {
    setActiveScrollElement(null);
    setFollowLiveEdge(true);

    stopFollowingBottom();

    expect(followLiveEdgeSeed.value).toBe(true);
  });
});

describe('a reveal retires the follow only where it leaves the reader off the edge', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  function armAndReveal(el: any, above: number, below: number) {
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    el.scrollHeight += above + below;
    el._scrollTop += above;
    honourAnchoredMutation(el);
  }

  it('retires the follow when a reveal leaves the reader short of the live edge', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);

    armAndReveal(el, 500, 1500);
    onScroll();

    expect(followingLiveEdge.value).toBe(false);
  });

  it('leaves the reader where the correction held them, writing nothing', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);

    armAndReveal(el, 500, 1500);
    const writesOnPress = el.writes;

    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(3000);
    expect(el.writes).toBe(writesOnPress);
  });

  it('keeps the follow for a reveal that grows only ABOVE the reader', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);

    armAndReveal(el, 500, 0);

    expect(el.scrollTop).toBe(3000);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('moves an UNARMED reader zero pixels, whatever the reveal did', () => {
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
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);

    armAndReveal(el, 500, 0);
    vi.advanceTimersByTime(1500);

    readerScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(false);
  });
});

describe('coming back to a thread resumes the follow it was left with', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('lands on the live edge the thread has NOW, and keeps riding it', () => {
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
    const el = makeEl({ scrollTop: 300, scrollHeight: 20000 });
    setActiveScrollElement(el);

    resumeFollowingBottom(el);

    expect(classifyScroll(el).navigation).toBe(true);
    expect(classifyScroll(el).followSurvives).toBe(true);
  });

  it('is retired by the reader exactly as a freshly armed one is', () => {
    const el = makeEl({ scrollTop: 300, scrollHeight: 20000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    resumeFollowingBottom(el);
    readerScrollsTo(el, 8000, onScroll);

    el.scrollHeight = 26000;
    onResize();
    expect(el.scrollTop).toBe(8000);
  });


  it('keeps the edge when the transcript grows after the resume on an idle thread', () => {
    const el = makeEl({ scrollTop: 300, scrollHeight: 2000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    resumeFollowingBottom(el);
    expect(el.scrollTop).toBe(1500);

    el.scrollHeight = 6000;
    onScroll();
    onResize();

    expect(el.scrollTop).toBe(5500);
    expect(awayFromBottom.value).toBe(false);
  });

  it('keeps it when the resume MOVED NOBODY, so no scroll event ever arrives', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 200 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    resumeFollowingBottom(el);
    expect(el.scrollTop).toBe(0);

    el.scrollHeight = 6000;
    onResize();

    expect(el.scrollTop).toBe(5500);
  });
});

describe('resuming IN PLACE, when a deep link owns the position', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('arms without moving the reader a pixel', () => {
    const el = makeEl({ scrollTop: 6000, scrollHeight: 20000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.writes = 0;

    resumeFollowingBottom(el, 'in-place');

    expect(followingLiveEdge.value).toBe(true);
    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(6000);
  });

  it('leaves an ALREADY ARMED follow and its held stamp alone', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 20000 });
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    el.scrollHeight = 21000;
    expect(el.scrollTop).toBe(19500);
    expect(classifyScroll(el).followSurvives).toBe(true);

    resumeFollowingBottom(el, 'in-place');

    expect(followingLiveEdge.value).toBe(true);
    expect(classifyScroll(el).followSurvives).toBe(true);
  });
});

describe('sending a message lands the reader on the live edge', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('lands the reader who sent FROM the live edge, once their turn renders', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const parked = atBottom(el);
    el.writes = 0;

    followSentMessage();
    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);

    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2900);
    expect(el.scrollTop).toBe(el.scrollHeight - el.clientHeight);
    expect(el.scrollTop).toBeGreaterThan(parked);
  });

  it('still lands after the platform moves the reader before their turn renders', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    atBottom(el);

    followSentMessage();
    el.scrollTop = 2440;
    onScroll();
    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2900);
  });

  it('writes nothing when the turn rendered without moving the live edge', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const parked = atBottom(el);
    el.writes = 0;

    followSentMessage();
    el.addUserMessage({ top: parked, height: 20 });
    el.scrollHeight = 3000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
  });

  it('writes nothing for a reader a rounded pixel off the edge either', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el._scrollTop = atBottom(el) - 1;
    el.writes = 0;

    followSentMessage();
    el.addUserMessage({ top: 2400, height: 20 });
    el.scrollHeight = 3000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2499);
  });

  it('arms nothing, so the reply streaming in leaves that reader where they are', () => {
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
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.writes = 0;

    el.clientHeight = 520;
    onResize();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(500);
  });

  it('glides to the BOTTOM, clear of the room the transcript reserves', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();

    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3108;
    onResize();
    expect(el.scrollTop).toBe(500);

    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2608);
    expect(el.scrollTop).not.toBe(2548);
    expect(3048).toBeLessThan(el.scrollTop + el.clientHeight);
  });

  it('goes on gliding while the response panel mounts under the row', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();

    const sent = el.addUserMessage({ top: 2900, height: 120, status: false });
    el.scrollHeight = 3080;
    onResize();
    vi.advanceTimersByTime(50);

    el.mountStatusLine(sent);
    el.scrollHeight = 3108;
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2608);
    expect(3048).toBeLessThan(el.scrollTop + el.clientHeight);
  });

  it('takes a QUEUED follow-up to the bottom too, since the reader submitted', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();

    el.addUserMessage({ top: 2900, height: 120, queued: true });
    el.scrollHeight = 3080;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2580);
    expect(3020).toBeLessThan(el.scrollTop + el.clientHeight);
    expect(el.scrollTop).toBeLessThan(2900);
  });

  it('never scrolls BACKWARDS, so a bubble already on screen moves nobody', () => {
    const el = makeEl({ scrollTop: 2000, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.writes = 0;

    el.addUserMessage({ top: 2300, height: 120, queued: true });
    el.scrollHeight = 2480;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2000);
  });

  it('a lapsed landing does not block the NEXT submit', () => {
    const nowSpy = vi.spyOn(performance, 'now');
    let clock = 1_000_000;
    nowSpy.mockImplementation(() => clock);
    try {
      const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
      makeScrollObservers(el);
      setActiveScrollElement(el);

      followSentMessage();
      clock += 20_000;

      el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
      answerAndConfirm('q1');
      vi.advanceTimersByTime(1500);

      expect(el.scrollTop).toBe(2500);
    } finally {
      nowSpy.mockRestore();
    }
  });

  it('lands on the bottom the transcript has when the glide ENDS', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();

    vi.advanceTimersByTime(100);
    el.scrollHeight = 6000;
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(5500);
  });

  it('then holds still, so the reply grows in UNDER where it landed', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    const sent = el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2900);

    el.drawResponseRow(sent.turn);
    onResize();
    vi.advanceTimersByTime(1500);
    el.writes = 0;

    for (const height of [5000, 9000]) {
      el.scrollHeight = height;
      onResize();
    }
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2900);
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

    readerScrollsTo(el, 1800, onScroll);
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
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(60);
    expect(el.scrollTop).toBeGreaterThan(500);

    readerScrollsTo(el, 800, onScroll);
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(800);
  });

  it('is cancelled by a gesture made BEFORE the message renders, so it never runs', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    readerScrollsTo(el, 900, onScroll);
    el.writes = 0;

    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(900);
  });

  it('keeps going forwards when the turn leaves the layout mid-glide', () => {
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
    sent.turn.isConnected = false;

    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBeGreaterThanOrEqual(midGlide);
    expect(el.scrollTop).toBe(2900);
  });

  it('LAPSES and moves nobody when the message has no box to land on', () => {
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
      expect(el.scrollTop).toBe(500);

      clock += 1500;
      el.scrollHeight = 3800;
      onResize();
      el.scrollHeight = 9000;
      onResize();
      vi.advanceTimersByTime(1500);

      expect(el.writes).toBe(0);
      expect(el.scrollTop).toBe(500);
    } finally {
      nowSpy.mockRestore();
    }
  });

  it('is not cancelled by the reflow correction while the landing is still pending', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const child = el.addAnchorChild(-100);

    followSentMessage();
    onScroll();

    el.clientWidth = 700;
    child._top = -160;
    onResize();
    onScroll();
    expect(el.scrollTop).toBe(440);

    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2900);
  });

  it('is not cancelled by the mobile keyboard closing after both send calls', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    followSentMessage();
    holdAcrossRelayout(el, el.scrollTop + 44);
    onScroll();

    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2900);
  });

  it('a second call for the same send is a no-op refresh, not a second landing', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 2900, height: 120 });
    followSentMessage();

    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2900);
  });
});

describe('the reader\'s own message glides into view, gently', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  function sendFromTheEdge(el: any, onResize: () => void) {
    followSentMessage();
    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
  }

  it('lands at the send pace: still under way past a navigation\'s floor, and starting slowly', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const parked = atBottom(el);

    sendFromTheEdge(el, onResize);
    vi.advanceTimersByTime(64);
    expect(el.scrollTop - parked).toBeLessThan(40);
    vi.advanceTimersByTime(250);
    expect(el.scrollTop).toBeLessThan(2900);

    vi.advanceTimersByTime(1000);
    expect(el.scrollTop).toBe(2900);
  });

  it('glides a rider to their sent message instead of snapping them there', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    const parked = el.scrollTop;
    expect(parked).toBe(2500);

    sendFromTheEdge(el, onResize);
    expect(el.scrollTop, 'the follow snapped the rider to the sent message').toBe(parked);
    vi.advanceTimersByTime(250);
    expect(el.scrollTop).toBeGreaterThan(parked);
    expect(el.scrollTop).toBeLessThan(2900);

    vi.advanceTimersByTime(1000);
    expect(el.scrollTop).toBe(2900);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('glides only the arrival: the reply streaming after it is carried as before', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);

    sendFromTheEdge(el, onResize);
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2900);

    el.scrollHeight = 3700;
    onResize();
    expect(el.scrollTop).toBe(3200);
  });

  it('spends the arrival on a turn that renders inside a glide already running', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    el.scrollHeight = 3200;
    el._scrollTop = 2400;

    followSentMessage();
    vi.advanceTimersByTime(48);
    el.addUserMessage({ top: 3100, height: 120 });
    el.scrollHeight = 3600;
    onResize();
    vi.advanceTimersByTime(600);
    expect(el.scrollTop).toBe(3100);

    el.scrollHeight = 3900;
    onResize();
    expect(el.scrollTop).toBe(3400);
  });

  it('records no arrival for a card answered while riding: the reply is carried', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2600, height: 300 });
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2500);

    answerAndConfirm('q1');
    el.scrollHeight = 3300;
    onResize();
    expect(el.scrollTop).toBe(2800);
  });
});

describe('a send moves nobody only when the live edge is already behind the reader', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });


  it('leaves the reader alone when the turn renders with no room under it', () => {
    followSentMessage();

    const el = makeEl({ scrollTop: 0, scrollHeight: 400, turns: 0 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    followSentMessage();
    el.writes = 0;

    const sent = el.addUserMessage({ top: 0, height: 120 });
    el.scrollHeight = 500;
    onResize();
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(0);

    el.drawResponseRow(sent.turn);
    el.scrollHeight = 500;
    onResize();
    vi.advanceTimersByTime(1500);
    el.writes = 0;
    for (const height of [900, 3000, 9000]) {
      el.scrollHeight = height;
      onResize();
    }
    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(0);
    expect(awayFromBottom.value).toBe(true);
  });

  it('lands the FIRST turn of a brand-new thread when there is content above it', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000, turns: 0 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 3100, height: 120 });
    el.scrollHeight = 3700;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(3200);

    el.writes = 0;
    el.scrollHeight = 6000;
    onResize();
    expect(el.writes).toBe(0);
  });

  it('leaves the reader alone in an existing thread that fits on screen', () => {
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
    const el = makeEl({ scrollTop: 0, scrollHeight: 400 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    resumeFollowingBottom(el);

    followSentMessage();

    el.scrollHeight = 4000;
    onResize();
    expect(el.scrollTop).toBe(3500);
  });
});

describe('answering a question card lands the same way', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });


  it('writes NO scroll when the reader is already at the live edge', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    const parked = atBottom(el);
    el.writes = 0;

    answerAndConfirm('q1');

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
  });

  it('arms nothing, so the agent resuming underneath moves that reader 0px', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const card = el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    const parked = atBottom(el);

    answerAndConfirm('q1');
    el.drawResponseRow(card.panel.turn);
    onResize();
    vi.advanceTimersByTime(1500);
    el.writes = 0;

    el.scrollHeight = 4000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
  });

  it('glides to the live edge, exactly as a send does', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    answerAndConfirm('q1');
    expect(el.scrollTop).toBe(500);

    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2500);
  });

  it('survives the answered body replacing the live one mid-glide', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    const live = el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    answerAndConfirm('q1');
    vi.advanceTimersByTime(60);
    const midGlide = el.scrollTop;
    expect(midGlide).toBeGreaterThan(500);

    el.answerQuestionCard(live);
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2500);
    expect(el.scrollTop).toBeGreaterThan(midGlide);
  });

  it('then holds still, so the reply grows in under the card', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const card = el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    answerAndConfirm('q1');
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2500);

    el.drawResponseRow(card.panel.turn);
    onResize();
    vi.advanceTimersByTime(1500);
    el.writes = 0;

    el.scrollHeight = 5000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
  });

  it('stops the glide the moment the reader scrolls, rather than dragging them back', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    answerAndConfirm('q1');
    vi.advanceTimersByTime(60);
    expect(el.scrollTop).toBeGreaterThan(500);

    readerScrollsTo(el, 1200, onScroll);
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

  it('a TYPED answer lands on the card, not on the row the send inserts', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followSentMessage();
    followSentMessage();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2500);
  });

  it('a TYPED answer lands on a permission card the engine raised as a question', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addPermissionCard({ requestId: 'q1', toolUseId: 'q1', top: 2400, height: 300 });

    followSentMessage();
    followSentMessage();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2500);
  });

  it('lands at once on the typed answer and its spinner, before any confirm', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const card = el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followSentMessage();
    followSentMessage();
    el.answerQuestionCard(card);
    el.scrollHeight = 3200;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2700);
  });

  it('then holds through a slow confirm, and shows the agent line when it comes', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const card = el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followSentMessage();
    followSentMessage();
    el.answerQuestionCard(card);
    el.scrollHeight = 3200;
    onResize();
    vi.advanceTimersByTime(12_000);

    followConfirmedCard('q1');
    el.scrollHeight = 3300;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2800);

    el.drawResponseRow(card.panel.turn);
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2900);

    el.writes = 0;
    el.scrollHeight = 5000;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.writes).toBe(0);
  });

  it('stops for the reader who scrolls away while the typed answer sends', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const card = el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followSentMessage();
    followSentMessage();
    vi.advanceTimersByTime(1500);
    readerScrollsTo(el, 1200, onScroll);

    el.answerQuestionCard(card);
    el.scrollHeight = 3400;
    onResize();
    followConfirmedCard('q1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(1200);
  });

  function sendPastADeadCard(kill: (el: any) => void) {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    kill(el);
    el.writes = 0;

    followSentMessage();
    vi.advanceTimersByTime(300);
    expect(el.writes).toBe(0);

    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2900);
  }

  it('leaves an ANSWERED card alone, so a plain follow-up waits for its own row', () => {
    sendPastADeadCard((el) => {
      el.answerQuestionCard(el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 }));
    });
  });

  it('leaves a TERMINATED card alone too, which an abort is how you get', () => {
    sendPastADeadCard((el) => {
      el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300, terminated: true });
    });
  });

  it('waits for a card that has no box yet, and moves nobody until it has one', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'another-question', top: 2400, height: 300 });
    el.writes = 0;

    answerAndConfirm('q1');
    vi.advanceTimersByTime(100);
    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(500);

    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    el.scrollHeight = 4000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(3500);
  });
});

describe('an answer lands once the engine confirms it', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('moves nobody while the answer is sending, then glides to the live edge', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const card = el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    el.writes = 0;

    followAnsweredQuestion('q1');
    el.answerQuestionCard(card);
    el.scrollHeight = 3100;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(500);

    followConfirmedCard('q1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2600);
  });

  it('waits the same way for a permission decision', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addPermissionCard({ requestId: 'p1', top: 2400, height: 300 });
    el.writes = 0;

    followResolvedPermission('p1');
    vi.advanceTimersByTime(1500);
    expect(el.writes).toBe(0);

    followConfirmedCard('p1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2500);
  });

  it('keeps a rider where they parked until the answer is confirmed', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    atBottom(el);
    setFollowLiveEdge(true);
    setTranscriptLive(false);
    readerScrollsTo(el, 1500, onScroll);
    el.writes = 0;

    followAnsweredQuestion('q1');
    el.scrollHeight = 3100;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.writes).toBe(0);

    followConfirmedCard('q1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2600);
  });

  it('keeps the reader where they scrolled while the answer was sending', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followAnsweredQuestion('q1');
    readerScrollsTo(el, 300, onScroll);
    followConfirmedCard('q1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(300);
  });

  it('still lands after a scroll the reader did not make while the answer was sending', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const card = el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followAnsweredQuestion('q1');
    el.answerQuestionCard(card);
    el.scrollTop = 460;
    onScroll();
    followConfirmedCard('q1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2500);
  });

  it('still lands when WebKit moves the offset just after the keyboard compensation', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followAnsweredQuestion('q1');
    holdAcrossRelayout(el, 544);
    el.scrollTop = 520;
    onScroll();
    followConfirmedCard('q1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2500);
  });

  it('moves nobody for an answer the engine never confirms', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    el.writes = 0;

    followAnsweredQuestion('q1');
    vi.advanceTimersByTime(9000);
    followConfirmedCard('q1');
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
  });

  it('moves nobody for a confirm the reader did not just submit', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    el.writes = 0;

    followConfirmedCard('q1');
    followAnsweredQuestion('q2');
    followConfirmedCard('q1');
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
  });

  function answerTheFirstOfTwo() {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const first = el.addQuestionCard({ toolUseId: 'q0', top: 2400, height: 300 });
    followAnsweredQuestion('q0');
    el.answerQuestionCard(first);
    followConfirmedCard('q0');
    return { el, first, onScroll, onResize };
  }

  it('brings the next question into view when it follows a drawn row', () => {
    const { el, first, onResize } = answerTheFirstOfTwo();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2500);

    el.drawResponseRow(first.panel.turn);
    onResize();
    el.addQuestionCard({ toolUseId: 'q1', top: 3000, height: 300 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2900);
  });

  it('brings it into view when it lands mid-glide too', () => {
    const { el, first, onResize } = answerTheFirstOfTwo();
    vi.advanceTimersByTime(60);

    el.drawResponseRow(first.panel.turn);
    onResize();
    el.addQuestionCard({ toolUseId: 'q1', top: 3000, height: 300 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2900);
  });

  it('ends the hold on the next question, so nothing after it carries the reader', () => {
    const { el, onResize } = answerTheFirstOfTwo();
    vi.advanceTimersByTime(1500);

    el.addQuestionCard({ toolUseId: 'q1', top: 3000, height: 300 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2900);
    el.writes = 0;

    el.scrollHeight = 5000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
  });

  it('leaves a question that arrives later where it lands', () => {
    const { el, first, onResize } = answerTheFirstOfTwo();
    vi.advanceTimersByTime(1500);
    el.drawResponseRow(first.panel.turn);
    onResize();
    vi.advanceTimersByTime(5000);
    el.writes = 0;

    el.addQuestionCard({ toolUseId: 'q1', top: 3000, height: 300 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
  });

  it('leaves the next question alone once the reader has scrolled away', () => {
    const { el, first, onScroll, onResize } = answerTheFirstOfTwo();
    vi.advanceTimersByTime(1500);
    el.drawResponseRow(first.panel.turn);
    onResize();
    readerScrollsTo(el, 1200, onScroll);
    el.writes = 0;

    el.addQuestionCard({ toolUseId: 'q1', top: 3000, height: 300 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(1200);
  });
});

describe('a submit holds the bottom until the agent draws', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('takes a card answered FROM the live edge down as the agent resumes', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const card = el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    const parked = atBottom(el);
    el.writes = 0;

    answerAndConfirm('q1');
    expect(el.scrollTop).toBe(parked);

    el.drawResponseRow(card.panel.turn);
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2900);
  });

  it('keeps holding a card whose agent takes its time to resume', () => {
    const nowSpy = vi.spyOn(performance, 'now');
    let clock = 1_000_000;
    nowSpy.mockImplementation(() => clock);
    try {
      const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
      const { onResize } = makeScrollObservers(el);
      setActiveScrollElement(el);
      const card = el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
      atBottom(el);

      answerAndConfirm('q1');

      clock += 1500;
      el.scrollHeight = 3100;
      onResize();
      vi.advanceTimersByTime(1500);

      clock += 1500;
      el.drawResponseRow(card.panel.turn);
      el.scrollHeight = 3400;
      onResize();
      vi.advanceTimersByTime(1500);

      expect(el.scrollTop).toBe(2900);
    } finally {
      nowSpy.mockRestore();
    }
  });

  it('keeps holding for a coding agent that takes many seconds to write', () => {
    const nowSpy = vi.spyOn(performance, 'now');
    let clock = 1_000_000;
    nowSpy.mockImplementation(() => clock);
    try {
      const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
      const { onResize } = makeScrollObservers(el);
      setActiveScrollElement(el);
      const card = el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
      atBottom(el);

      answerAndConfirm('q1');
      el.scrollHeight = 3060;
      onResize();
      vi.advanceTimersByTime(1500);
      expect(el.scrollTop).toBe(2560);

      clock += 14_000;
      el.drawResponseRow(card.panel.turn);
      el.scrollHeight = 3200;
      onResize();
      vi.advanceTimersByTime(1500);

      expect(el.scrollTop).toBe(2700);
    } finally {
      nowSpy.mockRestore();
    }
  });

  it('takes a send down again for a row that lands after the glide', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    const sent = el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3080;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2580);

    el.drawResponseRow(sent.turn);
    el.scrollHeight = 3200;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2700);
  });

  it('LETS GO once the agent has drawn, so the reply moves nobody', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    const sent = el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3080;
    onResize();
    vi.advanceTimersByTime(1500);

    el.drawResponseRow(sent.turn);
    el.scrollHeight = 3200;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2700);
    el.writes = 0;

    for (const height of [5000, 9000, 20000]) {
      el.scrollHeight = height;
      onResize();
    }
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2700);
  });

  it('lets go on a turn that will never draw, once the DOM has settled', () => {
    const nowSpy = vi.spyOn(performance, 'now');
    let clock = 1_000_000;
    nowSpy.mockImplementation(() => clock);
    try {
      const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
      const { onResize } = makeScrollObservers(el);
      setActiveScrollElement(el);

      followSentMessage();
      el.addUserMessage({ top: 2900, height: 120, queued: true });
      el.scrollHeight = 3080;
      onResize();
      vi.advanceTimersByTime(1500);
      expect(el.scrollTop).toBe(2580);

      clock += 1500;
      el.scrollHeight = 3200;
      onResize();
      vi.advanceTimersByTime(1500);
      el.writes = 0;

      for (const height of [5000, 9000]) {
        el.scrollHeight = height;
        onResize();
      }
      vi.advanceTimersByTime(1500);

      expect(el.writes).toBe(0);
    } finally {
      nowSpy.mockRestore();
    }
  });

  it('keeps holding when the response panel mounts a commit AFTER its row', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    const sent = el.addUserMessage({ top: 2900, height: 120, status: false });
    el.scrollHeight = 3080;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2580);

    el.mountStatusLine(sent);
    el.drawResponseRow(sent.turn);
    el.scrollHeight = 3300;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2800);
  });

  it('is ended by the reader scrolling away mid-hold', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const card = el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    atBottom(el);

    answerAndConfirm('q1');
    readerScrollsTo(el, 1200, onScroll);
    el.writes = 0;

    el.drawResponseRow(card.panel.turn);
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(1200);
  });

  it('gives up QUICKLY on a turn that never gets a box, so the next submit lands', () => {
    const nowSpy = vi.spyOn(performance, 'now');
    let clock = 1_000_000;
    nowSpy.mockImplementation(() => clock);
    try {
      const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
      const { onResize } = makeScrollObservers(el);
      setActiveScrollElement(el);

      followSentMessage();
      el.addUserMessage({ top: 2900, height: 120, visible: false });
      onResize();
      clock += 1500;

      el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
      answerAndConfirm('q1');
      vi.advanceTimersByTime(1500);

      expect(el.scrollTop).toBe(2500);
    } finally {
      nowSpy.mockRestore();
    }
  });

  it('watches the turn it RESOLVED, not whichever turn is newest later', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    const first = el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3080;
    onResize();
    vi.advanceTimersByTime(1500);

    el.addUserMessage({ top: 3100, height: 120, queued: true });
    el.scrollHeight = 3300;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2800);

    el.drawResponseRow(first.turn);
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2900);
    el.writes = 0;

    el.scrollHeight = 9000;
    onResize();
    expect(el.writes).toBe(0);
  });

  it('stops at the FIRST row, not the one that lands during the releasing glide', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    const sent = el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3080;
    onResize();
    vi.advanceTimersByTime(1500);

    el.drawResponseRow(sent.turn);
    el.scrollHeight = 3200;
    onResize();
    vi.advanceTimersByTime(60);

    el.drawResponseRow(sent.turn);
    el.scrollHeight = 3600;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2700);
    expect(el.scrollTop).not.toBe(3100);
  });

  it('lets go for a row drawn DURING the glide, without waiting for another resize', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    const sent = el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3080;
    onResize();
    vi.advanceTimersByTime(60);
    el.drawResponseRow(sent.turn);
    el.scrollHeight = 3200;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2700);
    el.writes = 0;
    el.scrollHeight = 9000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2700);
  });

  it('lets go at once for a turn that renders WITH the agent already drawing', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    const sent = el.addUserMessage({ top: 2900, height: 120 });
    el.drawResponseRow(sent.turn);
    el.drawResponseRow(sent.turn);
    el.scrollHeight = 3120;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2620);

    el.writes = 0;
    el.scrollHeight = 9000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2620);
  });

  it('keeps holding a CARD whose turn already carried a reply before it asked', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const card = el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300, status: true, rows: 3 });
    atBottom(el);

    answerAndConfirm('q1');
    el.scrollHeight = 3200;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2700);

    el.drawResponseRow(card.panel.turn);
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2900);

    el.writes = 0;
    el.scrollHeight = 9000;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.writes).toBe(0);
  });

  it('freezes the glide at the FIRST row, so a fast reply carries nobody', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    const sent = el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3080;
    onResize();
    vi.advanceTimersByTime(60);

    el.drawResponseRow(sent.turn);
    el.scrollHeight = 3200;
    onResize();
    vi.advanceTimersByTime(60);
    el.scrollHeight = 9000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2700);
    expect(el.scrollTop).not.toBe(8500);
  });

  it('rests on the first LINE of a reply that arrives as one row', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    const sent = el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3080;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2580);

    try {
      drawWholeReply(el, sent.turn);
      onResize();
      vi.advanceTimersByTime(1500);

      expect(el.scrollTop).toBe(2600);
      expect(el.scrollTop).not.toBe(3500);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('stops a glide in flight on that first line, and never steps back', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    const sent = el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3080;
    onResize();
    vi.advanceTimersByTime(250);

    try {
      drawWholeReply(el, sent.turn);
      const tops = [el.scrollTop];
      for (let ms = 0; ms < 1500; ms += 16) {
        vi.advanceTimersByTime(16);
        tops.push(el.scrollTop);
      }

      expect(el.scrollTop).toBe(2600);
      tops.forEach((top, i) => { if (i > 0) expect(top).toBeGreaterThanOrEqual(tops[i - 1]); });
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('carries an ARMED reader through that same fast reply', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    atBottom(el);
    setFollowLiveEdge(true);

    followSentMessage();
    const sent = el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3080;
    onResize();
    vi.advanceTimersByTime(60);

    el.drawResponseRow(sent.turn);
    el.scrollHeight = 3200;
    onResize();
    vi.advanceTimersByTime(60);
    el.scrollHeight = 9000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(8500);
  });

  it('lets go when its turn LEAVES the layout, rather than sitting out the backstop', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    const sent = el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3080;
    onResize();
    vi.advanceTimersByTime(1500);

    sent.turn.isConnected = false;
    el.scrollHeight = 9000;
    onResize();
    el.writes = 0;
    el.scrollHeight = 20000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
  });

  it('lets go on the BACKSTOP when the agent never draws at all', () => {
    const nowSpy = vi.spyOn(performance, 'now');
    let clock = 1_000_000;
    nowSpy.mockImplementation(() => clock);
    try {
      const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
      const { onResize } = makeScrollObservers(el);
      setActiveScrollElement(el);

      followSentMessage();
      el.addUserMessage({ top: 2900, height: 120 });
      el.scrollHeight = 3080;
      onResize();
      vi.advanceTimersByTime(1500);
      expect(el.scrollTop).toBe(2580);

      clock += 61_000;
      el.scrollHeight = 9000;
      onResize();
      el.writes = 0;
      el.scrollHeight = 20000;
      onResize();
      vi.advanceTimersByTime(1500);

      expect(el.writes).toBe(0);
    } finally {
      nowSpy.mockRestore();
    }
  });
});

describe('nothing is reserved under the newest turn', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

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

  function ridingButParked(el: any) {
    atBottom(el);
    setFollowLiveEdge(true);
    el.scrollHeight = 6000;
    el.writes = 0;
  }

  it('takes a send to the live edge, not to the just-sent message', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    ridingButParked(el);

    followSentMessage();
    el.addUserMessage({ top: 5800, height: 120 });
    el.scrollHeight = 6200;
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(5700);
    expect(el.scrollTop).not.toBe(5448);
  });

  it('takes an answer to the live edge, not to the answered card', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    ridingButParked(el);

    answerAndConfirm('q1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(5500);
  });

  it('takes a permission decision to the live edge', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addPermissionCard({ requestId: 'p1', top: 2400, height: 300 });
    ridingButParked(el);

    decideAndConfirm('p1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(5500);
  });

  it('takes a Continue to the live edge, without waiting for the continuation', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    ridingButParked(el);

    followContinuedThread();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(5500);
  });

  it('lands on the bottom the transcript has when the glide ENDS', () => {
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
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    ridingButParked(el);

    followSentMessage();
    vi.advanceTimersByTime(60);
    expect(el.scrollTop).toBeGreaterThan(2500);

    readerScrollsTo(el, 1000, onScroll);
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(1000);

    el.writes = 0;
    el.scrollHeight = 9000;
    onResize();
    expect(el.writes).toBe(0);
  });

  it('a second call for the same send does not queue a landing behind the glide', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    ridingButParked(el);

    followSentMessage();
    el.addUserMessage({ top: 5800, height: 120 });
    el.scrollHeight = 6200;
    followSentMessage();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(5700);

    el.scrollHeight = 7000;
    onResize();
    expect(el.scrollTop).toBe(6500);
  });

  it('a landing glide in flight absorbs a second SUBMIT rather than restarting', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(60);

    el.addQuestionCard({ toolUseId: 'q1', top: 3400, height: 300 });
    el.scrollHeight = 3800;
    answerAndConfirm('q1');
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(3300);
  });

  it('a glide that already LANDED does not suppress the next submit', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    ridingButParked(el);

    followSentMessage();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(5500);

    el.scrollHeight = 9000;
    followSentMessage();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(8500);
  });

  it('turns the down chevron off once the glide lands', () => {
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
    osReducesMotion.value = true;
    try {
      const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
      makeScrollObservers(el);
      setActiveScrollElement(el);
      ridingButParked(el);

      followSentMessage();

      expect(el.writes).toBe(1);
      expect(el.scrollTop).toBe(5500);
    } finally {
      osReducesMotion.value = false;
    }
  });

  it('writes no scroll for a rider who is already at the live edge', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addPermissionCard({ requestId: 'p1', top: 2400, height: 300 });
    const parked = atBottom(el);
    setFollowLiveEdge(true);
    el.writes = 0;

    decideAndConfirm('p1');
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
  });

  it('a permission decision LANDS an unarmed reader at the bottom', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addPermissionCard({ requestId: 'p1', top: 2400, height: 300 });

    decideAndConfirm('p1');
    expect(el.scrollTop).toBe(500);
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2500);

    el.writes = 0;
    el.scrollHeight = 4000;
    onResize();
    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2500);
  });

  it('waits for the card the reader DECIDED, never for a different one', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 6000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addPermissionCard({ requestId: 'p2', top: 4400, height: 300 });
    el.writes = 0;

    decideAndConfirm('p1');
    vi.advanceTimersByTime(100);
    expect(el.writes).toBe(0);

    el.addPermissionCard({ requestId: 'p1', top: 2400, height: 300 });
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(5500);
  });
});

describe('Cancel is a submit too, in both the acts it covers', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });


  it('lands a CANCELLED card once on its own turn, and does NOT hold', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followCanceledTurn('q1');
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2500);
    el.writes = 0;

    for (const height of [5000, 9000]) {
      el.scrollHeight = height;
      onResize();
    }
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2500);
  });

  it('waits out a SLOW stop, past the deadline a landing with no box gets', () => {
    const nowSpy = vi.spyOn(performance, 'now');
    let clock = 1_000_000;
    nowSpy.mockImplementation(() => clock);
    try {
      const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
      const { onResize } = makeScrollObservers(el);
      setActiveScrollElement(el);

      followCanceledTurn();
      clock += 3000;

      el.addContinuationTurn({ top: 2400, height: 300, status: false });
      el.scrollHeight = 3200;
      onResize();
      vi.advanceTimersByTime(1500);

      expect(el.scrollTop).toBe(2700);
    } finally {
      nowSpy.mockRestore();
    }
  });

  it('does not swallow the next submit while a STOP waits for its boundary', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followCanceledTurn();
    answerAndConfirm('q1');
    el.scrollHeight = 3200;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2700);
  });

  it('lands a STOP once on the boundary, and does NOT hold after it', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followCanceledTurn();
    expect(el.scrollTop).toBe(500);

    el.addContinuationTurn({ top: 2400, height: 300, status: false });
    el.scrollHeight = 3200;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2700);
    el.writes = 0;

    for (const height of [5000, 9000, 20000]) {
      el.scrollHeight = height;
      onResize();
    }
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2700);
  });
});

describe('Continue after an abort is a submit too', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });


  it('lands at the bottom once the continuation turn renders', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followContinuedThread();
    expect(el.scrollTop).toBe(500);

    el.addContinuationTurn({ top: 2400, height: 300 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2900);
  });

  it('arms nothing, so the resumed reply grows in under it', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followContinuedThread();
    const turn = el.addContinuationTurn({ top: 2400, height: 300 });
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2500);

    el.drawResponseRow(turn);
    onResize();
    vi.advanceTimersByTime(1500);
    el.writes = 0;

    for (const height of [5000, 9000]) {
      el.scrollHeight = height;
      onResize();
    }
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(2500);
  });

  it('LAPSES and moves nobody when the continuation never renders', () => {
    const nowSpy = vi.spyOn(performance, 'now');
    let clock = 1_000_000;
    nowSpy.mockImplementation(() => clock);
    try {
      const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
      const { onResize } = makeScrollObservers(el);
      setActiveScrollElement(el);

      followContinuedThread();
      el.writes = 0;

      clock += 1500;
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

const SUBMIT_SURFACES: Array<{
  name: string;
  arrange: (el: any, top?: number) => void;
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
    submit: () => answerAndConfirm('q1'),
  },
  {
    name: 'a decided permission card',
    arrange: (el, top = 2400) => el.addPermissionCard({ requestId: 'p1', top, height: 300 }),
    submit: () => decideAndConfirm('p1'),
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
  {
    name: 'a cancelled question card',
    arrange: (el, top = 2400) => el.addQuestionCard({ toolUseId: 'q1', top, height: 300 }),
    submit: () => followCanceledTurn('q1'),
  },
  {
    name: 'a stopped running turn',
    arrange: () => {},
    submit: (el, onResize, top = 2400) => {
      followCanceledTurn();
      el.addContinuationTurn({ top, height: 300 });
      onResize();
    },
  },
];

describe.each(SUBMIT_SURFACES)('the submit matrix: $name', ({ arrange, submit }) => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  function scrolledUp() {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    arrange(el);
    return { el, ...observers };
  }

  it('branch 1, the live edge is already behind them: writes nothing', () => {
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
    setFollowLiveEdge(true);
    el.scrollHeight = 6000;
    el.writes = 0;

    submit(el, onResize);
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(5500);
  });

  it('branch 4, scrolled up: goes to the live edge, same as every other branch', () => {
    const { el, onResize } = scrolledUp();

    submit(el, onResize);
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2500);
  });

  it('arms nothing: growth after the hold ends moves the reader 0px', () => {
    const { el, onResize } = scrolledUp();

    submit(el, onResize);
    vi.advanceTimersByTime(1500);

    el.drawIntoNewestTurn();
    onResize();
    vi.advanceTimersByTime(1500);
    const landed = el.scrollTop;
    el.writes = 0;

    for (const height of [4000, 6000, 20000]) {
      el.scrollHeight = height;
      onResize();
    }
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(landed);
  });

  it('a gesture mid-glide cancels the landing rather than being fought', () => {
    const { el, onScroll, onResize } = scrolledUp();

    submit(el, onResize);
    vi.advanceTimersByTime(60);
    expect(el.scrollTop).toBeGreaterThan(500);

    readerScrollsTo(el, 700, onScroll);
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(700);
  });
});

describe('a side question scrolls to its card start', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('waits for the new card, then rests its top on the landing line', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 6000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSideQuestion();
    vi.advanceTimersByTime(100);
    expect(el.scrollTop).toBe(500);

    el.addSideQuestionCard({ id: 'side-question-1', top: 3000, height: 200 });
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(3000);
  });

  it('takes over from a send landing still gliding, as the one tween', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    const sent = el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3080;
    onResize();
    vi.advanceTimersByTime(100);
    el.drawResponseRow(sent.turn);
    el.scrollHeight = 3200;
    vi.advanceTimersByTime(32);

    followSideQuestion();
    el.addSideQuestionCard({ id: 'side-question-1', top: 3100, height: 200 });
    el.scrollHeight = 4000;
    const tops: number[] = [];
    for (let ms = 0; ms < 1500; ms += 16) {
      vi.advanceTimersByTime(16);
      tops.push(el.scrollTop);
    }

    expect(el.scrollTop).toBe(3100);
    expect(Math.max(...tops)).toBeLessThanOrEqual(3100);
  });

  it('never goes past the live edge', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 6000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSideQuestion();
    el.addSideQuestionCard({ id: 'side-question-1', top: 5800, height: 200 });
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(5500);
  });

  it('lands on the card just asked, not one already on screen', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 6000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addSideQuestionCard({ id: 'side-question-1', top: 900, height: 200 });

    followSideQuestion();
    vi.advanceTimersByTime(100);
    expect(el.scrollTop).toBe(500);

    el.addSideQuestionCard({ id: 'side-question-2', top: 3000, height: 200 });
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(3000);
  });

  it('does not chase the answer growing under the card', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 6000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSideQuestion();
    el.addSideQuestionCard({ id: 'side-question-1', top: 3000, height: 200 });
    onResize();
    vi.advanceTimersByTime(1500);
    el.writes = 0;

    el.scrollHeight = 9000;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(3000);
  });

  it('leaves a reader riding the live edge riding', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);

    followSideQuestion();
    el.addSideQuestionCard({ id: 'side-question-1', top: 2900, height: 200 });
    el.scrollHeight = 3200;
    onResize();
    vi.advanceTimersByTime(1500);

    expect(followingLiveEdge.value).toBe(true);
    expect(el.scrollTop).toBe(2700);
  });

  it('never scrolls a reader back up to it', () => {
    const el = makeEl({ scrollTop: 4000, scrollHeight: 6000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.writes = 0;

    followSideQuestion();
    el.addSideQuestionCard({ id: 'side-question-1', top: 3000, height: 200 });
    onResize();
    vi.advanceTimersByTime(1500);

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(4000);
  });
});

describe('nothing but the chevron, a send and an answer arms it', () => {
  beforeEach(() => { resetFollow(); });
  afterEach(() => { resetFollow(); });

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
    unmovedBy((el) => { el.scrollHeight = 3040; });
  });

  it('a change being applied moves nobody', () => {
    unmovedBy((el) => { el.scrollHeight = 3400; });
  });

  it('a lazy load moves nobody', () => {
    unmovedBy((el) => { el.scrollHeight = 20000; });
  });

  it('a question card ARRIVING moves nobody, though answering it would', () => {
    unmovedBy((el) => {
      el.addQuestionCard({ toolUseId: 'q1', top: 2700, height: 300 });
      el.scrollHeight = 3300;
    });
  });

  it('no store action outside the send path reaches for the live edge', () => {
    const here = dirname(fileURLToPath(import.meta.url));
    const ACTIONS_DIR = resolve(here, '../../../store/actions');
    const ALLOWED: Record<string, string[]> = {
      'chat.ts': ['followSentMessage'],
      'threads.ts': ['stopFollowingBottom'],
      'transcript-find.ts': ['stopFollowingBottom'],
    };
    const CALLS = ['followSentMessage', 'followAnsweredQuestion', 'followResolvedPermission', 'followContinuedThread', 'followCanceledTurn', 'scrollToBottom', 'scrollToBottomAnimated', 'stopFollowingBottom'];

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

  it('only the follow toggle and the resume arm the follow', () => {
    const here = dirname(fileURLToPath(import.meta.url));
    const source: string = readFileSync(resolve(here, '../scrollState.ts'), 'utf8');

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

  it('only the reading position records and resumes a follow', () => {
    const here = dirname(fileURLToPath(import.meta.url));
    const SRC_DIR = resolve(here, '../../..');
    const DEFINED_IN = 'components/chat/scrollState.ts';
    const ALLOWED_CALLER = 'hooks/useScrollMemory.ts';

    const found: Record<string, string[]> = { resumeFollowingBottom: [], onFollowRideStarted: [] };
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
    expect(found.onFollowRideStarted).toEqual([ALLOWED_CALLER]);
  });
});

describe("the follow's carry holds the mobile dynamic bars", () => {
  beforeEach(() => {
    resetFollow();
    vi.useFakeTimers();
  });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); vi.unstubAllGlobals(); resetFollow(); });

  function riding() {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000, clientHeight: 500 });
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    observers.onScroll();
    expect(el.scrollTop).toBe(2500);
    return { el, ...observers };
  }

  it('marks the growth write a carry, which the bars hold for', () => {
    const { el, onResize } = riding();
    el.scrollHeight = 3400;
    onResize();
    expect(el.scrollTop).toBe(2900);
    expect(classifyScroll().navigation).toBe(true);
    expect(classifyScroll().kind === 'carry').toBe(true);
  });

  it('keeps the arming glide and a resume held, so both still reveal the bars', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000, clientHeight: 500 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(100);
    expect(classifyScroll().navigation).toBe(true);
    expect(classifyScroll().kind === 'carry').toBe(false);
    vi.advanceTimersByTime(1500);

    stopFollowingBottom();
    resumeFollowingBottom(el as unknown as HTMLElement);
    expect(classifyScroll().navigation).toBe(true);
    expect(classifyScroll().kind === 'carry').toBe(false);
  });

  it("leaves a resume's kind alone for a carry in the same frame, so their one event still reveals", () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000, clientHeight: 500 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    resumeFollowingBottom(el as unknown as HTMLElement);
    el.scrollHeight = 3400;
    onResize();
    expect(el.scrollTop).toBe(2900);
    expect(classifyScroll().navigation).toBe(true);
    expect(classifyScroll().kind === 'carry').toBe(false);

    vi.advanceTimersByTime(100);
    el.scrollHeight = 3800;
    onResize();
    expect(classifyScroll().kind === 'carry').toBe(true);
  });

  it('tells a delta consumer about a carry at the write, so a late event moves nothing', () => {
    const rebased = vi.fn();
    const unsubscribe = onRebasedScroll(rebased);
    const { el, onResize } = riding();
    rebased.mockClear();
    el.scrollHeight = 3400;
    onResize();
    unsubscribe();
    expect(rebased).toHaveBeenCalledWith(el, 'carry');
  });

  it("tells it a carry sharing a resume's event is held, which must still reveal", () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000, clientHeight: 500 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const rebased = vi.fn();
    const unsubscribe = onRebasedScroll(rebased);
    resumeFollowingBottom(el as unknown as HTMLElement);
    el.scrollHeight = 3400;
    onResize();
    unsubscribe();
    expect(rebased).toHaveBeenCalled();
    expect(rebased).not.toHaveBeenCalledWith(el, 'carry');
  });

  it('tells a delta consumer about a placement at the write, so a late event moves nothing', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000, clientHeight: 500 });
    const rebased = vi.fn();
    const unsubscribe = onRebasedScroll(rebased);
    markNavigationScroll(el as unknown as HTMLElement, 2200);
    unsubscribe();
    expect(rebased).toHaveBeenCalledWith(el, 'placement');
    expect(el.scrollTop).toBe(2200);
  });

  it('marks a quiet thread\'s follow write a carry too, so a late resize holds the bars', () => {
    const { el, onResize } = riding();
    setTranscriptLive(false);
    el.scrollHeight = 3400;
    onResize();
    expect(el.scrollTop).toBe(2900);
    expect(classifyScroll().navigation).toBe(true);
    expect(classifyScroll().kind === 'carry').toBe(true);
  });

  it('reveals nothing when a ridden thread goes quiet', () => {
    const dispatchEvent = vi.fn();
    vi.stubGlobal('document', { dispatchEvent });

    riding();
    setTranscriptLive(false);

    expect(dispatchEvent.mock.calls.filter(([e]) => e.type === 'reveal-mobile-bars')).toHaveLength(0);
  });
});


describe('down chevron visibility', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('hides throughout a riding follow-up glide despite the geometric gap', () => {
    const el = makeEl({ scrollTop: 2500, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    followSentMessage();
    el.addUserMessage({ top: 3100, height: 120 });
    el.scrollHeight = 3700;
    onResize();
    expect(awayFromBottom.value).toBe(true);
    expect(showScrollToBottom.value).toBe(false);
    vi.advanceTimersByTime(80);
    onScroll();
    expect(awayFromBottom.value).toBe(true);
    expect(showScrollToBottom.value).toBe(false);
    vi.advanceTimersByTime(1500);
    onScroll();
    expect(awayFromBottom.value).toBe(false);
    expect(showScrollToBottom.value).toBe(false);
  });

  it.each([true, false])('offers a jump immediately after interruption, live=%s', (live) => {
    const el = makeEl({ scrollTop: 2500, scrollHeight: 3000 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    setTranscriptLive(live);
    readerScrollsTo(el, 1000, onScroll);
    expect(followingLiveEdge.value).toBe(!live);
    expect(awayFromBottom.value).toBe(true);
    expect(showScrollToBottom.value).toBe(true);
    readerScrollsTo(el, 2500, onScroll);
    expect(showScrollToBottom.value).toBe(false);
  });

  it('offers an unarmed reader a jump when growth leaves them behind', () => {
    const el = makeEl({ scrollTop: 2500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.scrollHeight = 3700;
    onResize();
    expect(awayFromBottom.value).toBe(true);
    expect(showScrollToBottom.value).toBe(true);
  });

  it.each([
    ['typed', () => followSentMessage()],
    ['picked', () => answerAndConfirm('q1')],
  ])('hides while an unarmed %s answer lands, and returns once it lets go', (_shape, answer) => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    const card = el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    onScroll();
    expect(showScrollToBottom.value).toBe(true);

    answer();
    expect(awayFromBottom.value).toBe(true);
    expect(showScrollToBottom.value).toBe(false);
    vi.advanceTimersByTime(80);
    onScroll();
    expect(showScrollToBottom.value).toBe(false);
    vi.advanceTimersByTime(1500);
    onScroll();
    expect(showScrollToBottom.value).toBe(false);

    el.drawResponseRow(card.panel.turn);
    onResize();
    vi.advanceTimersByTime(1500);
    el.scrollHeight = 6000;
    onResize();
    expect(followingLiveEdge.value).toBe(false);
    expect(awayFromBottom.value).toBe(true);
    expect(showScrollToBottom.value).toBe(true);
  });

  it('stays while a submit that has not found its turn moves nobody', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onScroll } = makeScrollObservers(el);
    setActiveScrollElement(el);
    onScroll();

    followCanceledTurn();
    expect(showScrollToBottom.value).toBe(true);
    followSentMessage();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(500);
    expect(showScrollToBottom.value).toBe(true);
  });
});
