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
  followSentMessage,
  isFollowScroll,
  isNavigationScroll,
  makeScrollObservers,
  resumeFollowingBottom,
  scrollToBottom,
  scrollToTop,
  setActiveScrollElement,
  stopFollowingBottom,
} from '../scrollState';

/** "Go to the bottom" means "and keep me there until I say otherwise".
 *
 *  The reader still owns the transcript's scroll position, and the app still
 *  moves it only when the reader asks. What this file pins down is the DURATION
 *  of three particular asks: the down chevron, a send and submitting an answer
 *  to a question card all arm a standing follow, growth honours it, and only the
 *  reader's own scroll retires it.
 *
 *  Its mirror is `scroll-resize-never-follows.test.ts`, which pins the other
 *  half: growth moves an UNARMED reader zero pixels, including one who happens
 *  to be sitting exactly at the live edge. Being at the bottom is a position,
 *  not a request, and that distinction is the whole point of both files. */

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
}) {
  const panels: any[] = [];
  const cards: any[] = [];
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
        : selector === '.question-body' ? cards
          : []
    ),
    /** Render one more user message, the way an optimistic send row arrives.
     *  `visible` false models a panel with no box, which is what a queued
     *  follow-up folded into its closed disclosure group has. */
    addUserMessage(p: { top: number; height: number; visible?: boolean }) {
      const panel = {
        parentElement: null,
        isConnected: true,
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
      panels.push(panel);
      return panel;
    },
    /** Render a question card: a `.question-body` carrying the card's tool-use id
     *  inside the `.initiator-panel` that is the answer's landing target. The two
     *  are given DIFFERENT rects (the body is inset inside the panel) so a test
     *  landing on the panel cannot pass by measuring the body instead. */
    addQuestionCard(p: { toolUseId: string; top: number; height: number }) {
      const rect = (top: number, height: number) => () => ({
        width: 800, height, top: top - el.scrollTop, bottom: top + height - el.scrollTop, left: 0, right: 800,
      });
      const panel = { parentElement: null, isConnected: true, getBoundingClientRect: rect(p.top, p.height) };
      const body = {
        parentElement: null,
        isConnected: true,
        getAttribute: (name: string) => (name === 'data-tool-use-id' ? p.toolUseId : null),
        closest: (sel: string) => (sel === '.initiator-panel' ? panel : null),
        getBoundingClientRect: rect(p.top + 20, Math.max(0, p.height - 40)),
      };
      cards.push(body);
      return { body, panel };
    },
    /** The live→answered swap, as Preact actually performs it: `QuestionBody`
     *  returns a DIFFERENT component once the answer lands, so the body node is
     *  unmounted and a new one mounted, while the `.initiator-panel` around it is
     *  the same vnode in the same position and is REUSED. So the panel object is
     *  carried over, and only the body is replaced. */
    answerQuestionCard(card: { body: any; panel: any }) {
      card.body.isConnected = false;
      cards.splice(cards.indexOf(card.body), 1);
      const answered = {
        ...card.body,
        isConnected: true,
        closest: (sel: string) => (sel === '.initiator-panel' ? card.panel : null),
      };
      cards.push(answered);
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
 *  opens another thread, not a test-only hatch. */
function resetFollow() {
  stopFollowingBottom();
  setActiveScrollElement(null);
  awayFromBottom.value = false;
}

describe('the down chevron arms a standing follow', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('keeps the reader at the live edge as the reply keeps growing, not just once', () => {
    // The gap this closes. The chevron used to be a one-shot jump: it landed the
    // reader at the bottom and the next chunk stranded them above it again, so
    // following a streaming reply meant tapping it over and over.
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    scrollToBottom();
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

    scrollToBottom();
    el.scrollHeight = 3400;       // a chunk lands before the scroll event does
    onScroll();                   // the chevron's own event, arriving late

    el.scrollHeight = 3800;
    onResize();
    expect(el.scrollTop).toBe(3300); // still following
  });

  it('is retired by a scroll up, and later growth then leaves the reader alone', () => {
    const el = makeEl({ scrollTop: 100, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    scrollToBottom();
    el.scrollTop = 2000;          // wheel, drag, flick, momentum or a keypress
    onScroll();

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

    scrollToBottom();
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

    scrollToBottom();

    el.scrollHeight = 2800;  // the question card is replaced by the answer
    onResize();
    onScroll();              // the browser clamping the reader down fires one
    expect(el.scrollTop).toBe(2300);

    el.scrollHeight = 3600;  // and a turn is expanded
    onResize();
    expect(el.scrollTop).toBe(3100);
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
    el.scrollTop = 8000;
    onScroll();

    el.scrollHeight = 26000;
    onResize();
    expect(el.scrollTop).toBe(8000);
  });
});

describe('sending a message arms the same follow', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  it('writes NO scroll when the reader is already at the live edge', () => {
    // They are already there. A write that lands them where they already were is
    // still an unrequested movement, and on iOS it cancels a momentum scroll, so
    // the assertion is on the write and not only on the resulting position.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    const parked = atBottom(el);
    el.writes = 0;

    followSentMessage();

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
  });

  it('still arms, so the reply streaming in keeps the live edge in view', () => {
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    atBottom(el);

    followSentMessage();

    el.scrollHeight = 4000;
    onResize();
    expect(el.scrollTop).toBe(3500);
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

  it('glides to the just-sent message, landing its bottom edge on the viewport bottom', () => {
    // Anchored on the message ELEMENT. A scrollHeight target would land at 2900
    // here, which is past the message and hides the very thing the reader wrote.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();

    el.addUserMessage({ top: 2900, height: 120 }); // the optimistic row renders
    el.scrollHeight = 3400;                        // with the working indicator under it
    onResize();
    expect(el.scrollTop).toBe(500); // a glide, not a jump: nothing lands this frame

    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2520);                      // 3020 (message bottom) - 500
    expect(el.scrollTop).toBeLessThan(2900);              // NOT the transcript bottom
    expect(2900).toBeGreaterThanOrEqual(el.scrollTop);    // and the message is fully visible
    expect(3020).toBeLessThanOrEqual(el.scrollTop + el.clientHeight);
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

    expect(el.scrollTop).toBe(2520);
  });

  it('then rides the live edge, so the message scrolls up as the answer grows', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2520);

    el.scrollHeight = 5000;
    onResize();
    expect(el.scrollTop).toBe(4500);
  });

  it('is retired by a scroll up during the reply, and growth then moves the reader 0px', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(1500);

    el.scrollTop = 1800; // the reader goes back to read something
    onScroll();
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
    // The disarm and the tween must not disagree. The follow owns this glide, so
    // retiring the follow retires the glide; a chevron tap's tween, which arms
    // nothing until it lands, still reaches its target as it always has.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    followSentMessage();
    el.addUserMessage({ top: 2900, height: 120 });
    el.scrollHeight = 3400;
    onResize();
    vi.advanceTimersByTime(60);
    expect(el.scrollTop).toBeGreaterThan(500);

    el.scrollTop = 800; // the reader takes over mid-glide
    onScroll();
    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(800);
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
    expect(el.scrollTop).toBeLessThanOrEqual(2520);
  });

  it('gives up the landing but not the follow when the message has no box to land on', () => {
    // The second and later queued follow-ups fold into a CLOSED disclosure group,
    // so the message the reader just sent has no rect. Waiting forever would hold
    // the whole follow inert, which is the part they actually asked for.
    const nowSpy = vi.spyOn(performance, 'now');
    let clock = 1_000_000;
    nowSpy.mockImplementation(() => clock);
    try {
      const el = makeEl({ scrollTop: 500, scrollHeight: 3000, panels: [{ top: 200, height: 120 }] });
      const { onResize } = makeScrollObservers(el);
      setActiveScrollElement(el);

      followSentMessage();
      el.addUserMessage({ top: 2900, height: 120, visible: false });
      el.scrollHeight = 3400;
      onResize();
      expect(el.scrollTop).toBe(500); // inside the deadline: still waiting for it

      clock += 1500; // past SEND_LANDING_DEADLINE_MS
      el.scrollHeight = 3800;
      onResize();

      expect(el.scrollTop).toBe(3300); // riding the live edge instead
    } finally {
      nowSpy.mockRestore();
    }
  });

  it('is not retired by the reflow correction while the landing is still pending', () => {
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

    expect(el.scrollTop).toBe(2520); // the landing still happened
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

    expect(el.scrollTop).toBe(2520);
  });
});

describe('answering a question card arms the same follow', () => {
  beforeEach(() => { resetFollow(); vi.useFakeTimers(); });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); resetFollow(); });

  /** Submitting an answer IS a send: the reader produced the content at the
   *  bottom and is owed the reply to it. Which of the three shapes they used
   *  must not be something they can feel in the scroll, so this whole block is
   *  the send's block above with the card in place of the message. The third
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

  it('still arms, so the agent resuming underneath keeps the live edge in view', () => {
    // The half the reader actually asked for. Answering used to arm nothing, so
    // the resumed reply streamed in below a reader who stayed put.
    const el = makeEl({ scrollTop: 0, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });
    atBottom(el);

    followAnsweredQuestion('q1');

    el.scrollHeight = 4000;
    onResize();
    expect(el.scrollTop).toBe(3500);
  });

  it('glides to the answered card, landing its bottom edge on the viewport bottom', () => {
    // Anchored on the card's PANEL, exactly as a send anchors on its message
    // panel: a scrollHeight target would land at 2500 here, past the card, and
    // hide the thing they just answered.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followAnsweredQuestion('q1');
    expect(el.scrollTop).toBe(500); // a glide, not a jump

    vi.advanceTimersByTime(1500);

    expect(el.scrollTop).toBe(2200);           // 2700 (card bottom) - 500
    expect(el.scrollTop).toBeLessThan(2500);   // NOT the transcript bottom
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

    expect(el.scrollTop).toBe(2200); // still lands on the card, not stranded
    expect(el.scrollTop).toBeGreaterThan(midGlide);
  });

  it('then rides the live edge, so the card scrolls up as the reply grows', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followAnsweredQuestion('q1');
    vi.advanceTimersByTime(1500);
    expect(el.scrollTop).toBe(2200);

    el.scrollHeight = 5000;
    onResize();
    expect(el.scrollTop).toBe(4500);
  });

  it('is retired by a scroll up during the reply, and growth then moves the reader 0px', () => {
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'q1', top: 2400, height: 300 });

    followAnsweredQuestion('q1');
    vi.advanceTimersByTime(1500);

    el.scrollTop = 1200; // the reader goes back to read something
    onScroll();
    const parked = el.scrollTop;
    el.writes = 0;

    for (const height of [4000, 5000, 9000]) {
      el.scrollHeight = height;
      onResize();
    }

    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(parked);
  });

  it('arms the follow even when the card cannot be resolved, and scrolls nobody meanwhile', () => {
    // The landing is the nicety; the follow is the request. A card with no box
    // (the hidden dual-mount copy, a windowed-out render) must not cost the
    // reader the live edge, and must not produce a blind jump either.
    const el = makeEl({ scrollTop: 500, scrollHeight: 3000 });
    const { onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.addQuestionCard({ toolUseId: 'another-question', top: 2400, height: 300 });
    el.writes = 0;

    followAnsweredQuestion('q1');
    vi.advanceTimersByTime(1500);
    expect(el.writes).toBe(0);
    expect(el.scrollTop).toBe(500);

    el.scrollHeight = 4000;
    onResize();
    expect(el.scrollTop).toBe(3500);
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
    /** The sanctioned callers, per module. `chat.ts` owns the send's arming;
     *  `threads.ts` retires the follow when the reader opens another thread.
     *  `followAnsweredQuestion` has no sanctioned store-action caller at all: the
     *  two card-submitted answers arm it from the components that own the tap
     *  (`QuestionCard`, `PromptInput`), and `answerThreadQuestion` in
     *  `chat-claude-code.ts` deliberately does not, because it is also the
     *  transport for an answer nobody watched happen. */
    const ALLOWED: Record<string, string[]> = {
      'chat.ts': ['followSentMessage'],
      'threads.ts': ['stopFollowingBottom'],
    };
    const CALLS = ['followSentMessage', 'followAnsweredQuestion', 'scrollToBottom', 'scrollToBottomAnimated', 'stopFollowingBottom'];

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

  /** The scan above pins the three ARMING points. This one pins the two entry
   *  points that let the reading position record and resume a follow, and it has
   *  to reach the whole tree rather than `store/actions` alone, because neither
   *  belongs to a store action at all.
   *
   *  `resumeFollowingBottom` re-arms a follow the reader is not making right now,
   *  which is safe for exactly one reason: it fires only for a thread whose
   *  RECORDED reading position is the live edge, so only for a thread the reader
   *  armed themselves. A second caller would lose that guarantee and become the
   *  fourth arming point the rule above exists to prevent. `onFollowArmed`
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
