import { describe, it, expect, beforeAll, afterAll, beforeEach, afterEach, vi } from 'vitest';

// The press ledger, driven through the document stub rather than asserted from
// source. The eighth episode of the dead-composer report was SILENT, and three
// of the ways the probe can go silent are wiring rather than arithmetic. Only a
// driven listener catches those.
//
// Full reconstruction of the episode, and why each case below exists:
// docs/plans/2026-08-29-the-composer-says-when-send-is-unreachable.md

const showToast = vi.hoisted(() => vi.fn());
const postClientLog = vi.hoisted(() => vi.fn());
vi.mock('../../../store/store', () => ({ showToast }));
vi.mock('../../../utils/clientLog', () => ({ postClientLog }));

import { installDeadPressProbe } from '../deadPressProbe';
import { notePressOutcome } from '../../../utils/tapGesture';

interface Box { left: number; right: number; top: number; bottom: number }

/** The smallest element the probe actually reads. It asks for an aria-label, a
 *  class list, a box, ancestry and `isConnected`, and nothing else. */
class FakeEl {
  disabled = false;
  isConnected = true;
  textContent = '';
  classes: string[];
  /** Public and mutable: the morph swaps only its label between send and
   *  cancel, and a case has to be able to put it in either mode. */
  label: string | null;
  /** Public and mutable, so a case can move a face BETWEEN two taps and tell
   *  the resulting lines apart by the box each press snapshotted. */
  box: Box;
  private row: FakeEl | null;

  /** Counted, because the dead-tap rescue activates a face by clicking it. */
  clicks = 0;

  constructor(label: string | null, box: Box, classes: string[] = ['action-btn'], row: FakeEl | null = null) {
    this.label = label;
    this.box = box;
    this.classes = classes;
    this.row = row;
  }

  get tagName() { return 'BUTTON'; }
  getAttribute(name: string) { return name === 'aria-label' ? this.label : null; }
  get classList() {
    return {
      item: (i: number) => this.classes[i] ?? null,
      contains: (c: string) => this.classes.includes(c),
    };
  }
  getBoundingClientRect() {
    return {
      ...this.box,
      width: this.box.right - this.box.left,
      height: this.box.bottom - this.box.top,
    };
  }
  /** Counted AND dispatched, because a real `HTMLElement.click()` reaches the
   *  document. The rescue activates a face that way, so the probe's own click
   *  listener sees it, and what it does there is a real question. */
  click() {
    this.clicks += 1;
    fire('click', { target: this });
  }
  contains(other: unknown) { return other === this; }
  closest(sel: string) {
    // A face is a real `<button>`; the row and the transcript are divs. Which
    // of the two a click landed on is what says whether anything could have
    // taken it. See `clickClaimedPress`.
    if (sel === 'button') {
      return this.classes.some((c) => c === 'action-btn' || c === 'icon-btn') ? this : null;
    }
    if (sel !== '.prompt-actions-row') return null;
    if (this.classes.includes('prompt-actions-row')) return this;
    return this.row;
  }
}

const ROW_BOX: Box = { left: 0, right: 390, top: 400, bottom: 444 };
const SEND_BOX: Box = { left: 330, right: 374, top: 400, bottom: 444 };

let row: FakeEl;
let send: FakeEl;
/** What the row currently holds. A case in ANSWER mode swaps the morph out for
 *  a Submit. `PromptInput` chooses between the two at one JSX position, so the
 *  morph node is not in the document there. That is the state the thirteenth
 *  episode was in, and the state the rescue used to have no face for. */
let faces: FakeEl[];
/** A target outside the composer, so `closest` answers null and the touch is
 *  genuinely unattributed. Using the row itself would make `onRow` true and let
 *  a case pass through the old gate it was written to bypass. */
let elsewhere: FakeEl;
/** What `elementFromPoint` answers, for the hit-test disagreement cases. */
let atPoint: unknown = null;
/** A per-point answer, for the one state a single answer cannot express: a face
 *  reachable at its own centre while the finger reaches nothing. That IS the
 *  wedge, so the harness has to be able to say it. */
let atPointNear: ((x: number, y: number) => unknown) | null = null;

function installDom() {
  const doc = globalThis.document as unknown as Record<string, unknown>;
  doc.querySelectorAll = (sel: string) => {
    if (sel === '.prompt-actions-row') return [row];
    if (sel === '.prompt-actions-row .action-btn') return faces;
    return [];
  };
  doc.elementFromPoint = (x: number, y: number) => (atPointNear ? atPointNear(x, y) : atPoint);
  doc.querySelector = (sel: string) => {
    if (sel === '.prompt-actions-row .send-cancel-morph') {
      return faces.find((f) => f.classes.includes('send-cancel-morph')) ?? null;
    }
    if (sel === '.prompt-actions-row [aria-label="Submit answer"]') {
      return faces.find((f) => f.label === 'Submit answer') ?? null;
    }
    return null;
  };
  (globalThis as unknown as Record<string, unknown>).MutationObserver = class {
    observe() { /* the row is never mutated in these cases */ }
    disconnect() { /* nothing to release */ }
  };
}

/** `id` is the finger. The probe binds a press to one identifier, so a second
 *  finger's lift or travel must not settle or move the first finger's press. */
function touch(el: unknown, x: number, y: number, fingers = 1, id = 0, screenOff = 0) {
  const point = {
    clientX: x, clientY: y, screenX: x + screenOff, screenY: y + screenOff, identifier: id,
  };
  return { target: el, changedTouches: [point], touches: new Array(fingers).fill(point) };
}

function fire(type: string, event: Record<string, unknown>) {
  (globalThis.document as unknown as { dispatchEvent(e: unknown): void })
    .dispatchEvent({ type, ...event });
}

interface Line {
  face: string;
  verdict: string;
  rowRect?: Box;
  faceRect?: Box;
  /** Written by the scheduled check, which has no gesture behind it. */
  scheduled?: boolean;
  /** Why no watchable face took a press attributed to the row. */
  under?: string;
  underFace?: string | null;
  faceCount?: number;
  watchableCount?: number;
  /** Where the finger landed, and the nearest face it failed to reach. */
  point?: { x: number; y: number };
  missedBy?: { face: string; px: number; dx?: number; dy?: number; rect?: Box } | null;
  /** The silence that ended at the input this line belongs to. */
  quiet?: {
    ms: number; checks: number; unreachable: number; covered: number; nudges: number;
  } | null;
  /** The row's own state, read at the PRESS. See `PressContext`. */
  morph?: string;
  viewport?: { appHeight: string; keyboardActive: boolean };
  /** Why the rescue refused. Only `rescue-stood-down` carries it. */
  standDown?: string;
  /** Which cover the app had up when it declined to judge the press. */
  cover?: string;
  /** Set only on a repair line. */
  nudged?: boolean;
  connected?: boolean;
  /** The touch's screen-to-client offset. See `screenOffset`. */
  screenOff?: { x: number; y: number };
}

/** Every `composer-press` line written so far, newest last. */
function lines(): Line[] {
  return postClientLog.mock.calls
    .filter((c) => c[0] === 'composer-press')
    .map((c) => c[2] as Line);
}

function verdicts(): string[] {
  return lines().map((l) => l.verdict);
}

beforeAll(() => {
  // ONE fake clock for the whole file, never re-installed. The probe holds
  // absolute timestamps for its throttle and its touch-behind-click window, and
  // a per-case `useFakeTimers` resets the clock to real time. That runs it
  // BACKWARDS past those timestamps, so a case silently throttles itself out.
  vi.useFakeTimers();
  const g = globalThis as unknown as Record<string, unknown>;
  g.innerWidth = 390;
  // A real height, because the probe refuses to hit-test a point outside the
  // viewport: `elementFromPoint` answers null there, which is indistinguishable
  // from a covered element.
  g.innerHeight = 844;
  g.scrollY = 0;
  // A device that can produce a touch. `click-no-touch` is meaningless without
  // one, so the probe checks for touch capability and not just a narrow window.
  g.ontouchstart = null;
  installDom();
  installDeadPressProbe();
});

afterAll(() => {
  vi.useRealTimers();
  delete (globalThis as unknown as Record<string, unknown>).ontouchstart;
});

beforeEach(() => {
  row = new FakeEl(null, ROW_BOX, ['prompt-actions-row']);
  send = new FakeEl('Send message', SEND_BOX, ['action-btn', 'send-cancel-morph'], row);
  faces = [send];
  elsewhere = new FakeEl(null, { left: 0, right: 390, top: 0, bottom: 300 }, ['thread-content']);
  atPoint = send;
  atPointNear = null;
  showToast.mockClear();
  postClientLog.mockClear();
});

afterEach(() => {
  // Drain every grace window, so one case's settling press cannot rule inside
  // the next one and be read as its result.
  vi.advanceTimersByTime(5000);
});

/** A whole tap: down, then up. The caller advances the grace window. */
function tapSend() {
  fire('touchstart', touch(send, 350, 420));
  fire('touchend', touch(send, 350, 420));
}

describe('the press ledger keeps every press it watched', () => {
  it('writes a line for each of two taps 100ms apart', () => {
    // The old probe dropped the FIRST press at the second `touchstart`, timer
    // and all. Tapping again is what a user does to a dead-feeling button, so
    // the gesture the bug provokes was the gesture that erased the evidence.
    tapSend();
    vi.advanceTimersByTime(100);
    tapSend();
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toEqual(['dead', 'dead']);
  });

  it('keeps each press with the claim it earned, not a neighbour’s', () => {
    // `takePressOutcome` is one consuming slot. Read at the end of a 600ms
    // window, an earlier press would swallow a later press's claim. It would
    // then report itself served while the later press read dead.
    fire('touchstart', touch(send, 350, 420));
    fire('touchend', touch(send, 350, 420));
    vi.advanceTimersByTime(0);            // press one takes its (absent) claim
    vi.advanceTimersByTime(100);
    fire('touchstart', touch(send, 350, 420));
    fire('touchend', touch(send, 350, 420));
    notePressOutcome('served');
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toEqual(['dead', 'served']);
  });

  it('reports an armed press whose lift never arrived', () => {
    // The shape a touch pipeline that stops mid-gesture leaves. WebKit owes a
    // `touchend` or a `touchcancel` for every `touchstart`.
    fire('touchstart', touch(send, 350, 420));
    fire('touchstart', touch(row, 10, 420));
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toContain('no-lift');
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('the lift never arrived'),
      'warning',
    );
  });

  it('does not call a second finger a lost lift', () => {
    fire('touchstart', touch(send, 350, 420));
    fire('touchstart', touch(row, 10, 420, 2, 1));
    vi.advanceTimersByTime(1000);
    expect(verdicts()).not.toContain('no-lift');
  });

  it('still rules the first press when a second finger joined and then lifted', () => {
    // Clearing `armed` for the second finger stranded the press: no lift could
    // reach it and no line was ever written.
    fire('touchstart', touch(send, 350, 420));
    fire('touchstart', touch(row, 10, 420, 2, 1));
    fire('touchend', touch(send, 350, 420));
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toEqual(['dead']);
  });

  it('ignores a second finger lifting first, which is not this press ending', () => {
    // The lift handler read `armed` without asking which finger lifted, so
    // finger two's release settled finger one's press. It then reported `dead`
    // and toasted, while the real press went on to run Send.
    fire('touchstart', touch(send, 350, 420));
    fire('touchstart', touch(row, 10, 420, 2, 1));
    fire('touchend', touch(row, 10, 420, 1, 1));
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toEqual([]);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('does not let a second finger travel discard the first finger\u2019s press', () => {
    fire('touchstart', touch(send, 350, 420));
    fire('touchstart', touch(row, 10, 420, 2, 1));
    fire('touchmove', touch(row, 10, 200, 2, 1));
    fire('touchend', touch(send, 350, 420));
    vi.advanceTimersByTime(1000);
    // Stationary, so the report is not suppressed as a swipe.
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('did not register'),
      'warning',
    );
  });

  it('reports a press that is never followed by anything at all', () => {
    // THE episode's shape. A `no-lift` that waits for the next touch says
    // nothing when the pipeline stopped, because a stopped pipeline delivers no
    // next touch. A deadline is what makes the silence speak.
    fire('touchstart', touch(send, 350, 420));
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toEqual([]);
    vi.advanceTimersByTime(4000);
    expect(verdicts()).toEqual(['no-lift']);
  });

  it('does not toast at the deadline, when the finger may still be down', () => {
    // All the deadline knows is that the lift is overdue. Asserting the press
    // died would contradict the send a late lift still runs.
    fire('touchstart', touch(send, 350, 420));
    vi.advanceTimersByTime(5000);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('does not call an ordinary tap a lost lift once the deadline passes', () => {
    fire('touchstart', touch(send, 350, 420));
    fire('touchend', touch(send, 350, 420));
    vi.advanceTimersByTime(6000);
    expect(verdicts()).toEqual(['dead']);
  });

  it('gives a click to the newer of two settling presses', () => {
    // Insertion order handed it to the older one, which reversed the evidence:
    // the tap that died read `clicked` and the retry that worked read `dead`.
    // The face MOVES between the taps. Each press snapshots its own box, so
    // the two lines are told apart by that rather than by log order.
    const MOVED: Box = { left: 300, right: 344, top: 500, bottom: 544 };
    fire('touchstart', touch(send, 350, 420));
    fire('touchend', touch(send, 350, 420));
    vi.advanceTimersByTime(100);
    send.box = MOVED;
    fire('touchstart', touch(send, 320, 520));
    fire('touchend', touch(send, 320, 520));
    fire('click', { target: send });
    vi.advanceTimersByTime(1000);
    const clicked = lines().find((l) => l.verdict === 'clicked');
    expect(clicked?.faceRect).toEqual(MOVED);
    expect(lines().find((l) => l.verdict === 'dead')?.faceRect).toEqual(SEND_BOX);
  });

  it('lets a click inside the grace window claim its own press', () => {
    tapSend();
    vi.advanceTimersByTime(0);
    fire('click', { target: send });
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toEqual(['clicked']);
  });
});

describe('a click with no touch behind it', () => {
  /** Push the clock past the window in which a touch still counts as behind a
   *  click. The probe's reading of that is module state, so a touch fired by an
   *  earlier case would otherwise still be recent. */
  function noRecentTouch() {
    vi.advanceTimersByTime(2000);
    postClientLog.mockClear();
    showToast.mockClear();
  }

  function root(): Record<string, unknown> {
    return (globalThis.document as unknown as { documentElement: Record<string, unknown> })
      .documentElement;
  }

  /** The state every report describes, and the one `stray-click` is gated on. */
  function keyboardUp() {
    root().hasAttribute = (name: string) => name === 'data-keyboard-active';
  }

  let priorHasAttribute: unknown;
  beforeEach(() => { priorHasAttribute = root().hasAttribute; });
  afterEach(() => { root().hasAttribute = priorHasAttribute; });

  it('is recorded, because a live click path over a dead touch path is the split', () => {
    noRecentTouch();
    fire('click', { target: send });
    expect(verdicts()).toEqual(['click-no-touch']);
  });

  it('never toasts, because the click ran the action', () => {
    noRecentTouch();
    fire('click', { target: send });
    expect(showToast).not.toHaveBeenCalled();
  });

  it('stays quiet when a touch did precede it', () => {
    tapSend();
    vi.advanceTimersByTime(0);
    fire('click', { target: send });
    vi.advanceTimersByTime(1000);
    expect(verdicts()).not.toContain('click-no-touch');
  });

  it('is recorded when it reached no face either, which used to be silent', () => {
    // The last of the three silences an input could disappear into. It says the
    // same about the pipeline as `click-no-touch`, and more about the hit test:
    // the page took a click and answered somewhere the composer is not.
    noRecentTouch();
    keyboardUp();
    fire('click', { target: elsewhere, clientX: 40, clientY: 120, screenX: 40, screenY: 179 });
    expect(verdicts()).toEqual(['stray-click']);
    // Named, since where a touchless click DID land is the reading.
    expect(lines()[0].face).toContain('thread-content');
    expect(lines()[0].point).toEqual({ x: 40, y: 120 });
    // The one reading not taken from the layout side. A verdict about the page
    // hit-testing elsewhere is the one that needs it most.
    expect(lines()[0].screenOff).toEqual({ x: 0, y: 59 });
  });

  it('says nothing while the keyboard is down, which no report describes', () => {
    // Without this gate the verdict is not rare: a pointer click on a
    // touch-capable laptop writes one per click, and buries the ledger.
    noRecentTouch();
    root().hasAttribute = () => false;
    fire('click', { target: elsewhere, clientX: 40, clientY: 120 });
    expect(verdicts()).toEqual([]);
  });

  it('says nothing under a cover, where the shell is inert by design', () => {
    noRecentTouch();
    root().hasAttribute = (name: string) => name === 'data-keyboard-active'
      || name === 'data-overlay-open';
    fire('click', { target: elsewhere, clientX: 40, clientY: 120 });
    expect(verdicts()).toEqual([]);
  });

  it('never toasts for one, since nothing here says the press was owed', () => {
    noRecentTouch();
    keyboardUp();
    fire('click', { target: elsewhere, clientX: 40, clientY: 120 });
    expect(showToast).not.toHaveBeenCalled();
  });

  it('writes one line for a burst of them, not a stream', () => {
    noRecentTouch();
    keyboardUp();
    for (let i = 0; i < 3; i++) {
      fire('click', { target: elsewhere, clientX: 40, clientY: 120 + i });
    }
    expect(verdicts().filter((v) => v === 'stray-click')).toHaveLength(1);
  });

  it('carries no point for a programmatic click, which landed nowhere', () => {
    noRecentTouch();
    keyboardUp();
    fire('click', { target: elsewhere, clientX: 0, clientY: 0 });
    expect(lines()[0].point).toBeUndefined();
    expect(lines()[0].screenOff).toBeUndefined();
  });
});

describe('the reachability question is no longer behind the row gate', () => {
  /** Both the throttle and the reported-face latch are module state that
   *  outlives a case. Clear the first by advancing, and the second by letting
   *  the face answer once, which is the documented way it is forgotten. */
  function freshWedgeState() {
    vi.advanceTimersByTime(1000);
    atPoint = send;
    fire('touchstart', touch(elsewhere, 10, 90));
    vi.advanceTimersByTime(1000);
    postClientLog.mockClear();
    showToast.mockClear();
  }

  it('reports a face the page will not answer with, for a touch that missed the row', () => {
    // The gate this sits in front of is `!onRow && !inRow`, and a coordinate
    // space out of step with layout defeats exactly that gate. For two rounds
    // the immune check sat behind it. The target is OUTSIDE the row, so the
    // gate would have returned before the question was ever asked.
    freshWedgeState();
    atPoint = row;
    fire('touchstart', touch(elsewhere, 10, 90));
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('not reachable where it is drawn'),
      'warning',
    );
    // A repair follows the detection on this path too. The latch would
    // otherwise strand a wedge the user found by tapping, since the scheduled
    // check goes quiet once the face is reported.
    expect(verdicts()).toContain('unreachable');
    expect(verdicts()).not.toContain('missed');
  });

  it('stays quiet about a composer parked off-screen on another pane', () => {
    // The mobile swipe track is 300% wide and keeps all three panes laid out.
    // The thread pane's row therefore has a real box outside the viewport
    // whenever the user is elsewhere. `elementFromPoint` answers null for any
    // such point, so asking would call the composer wedged on every tap.
    freshWedgeState();
    send.box = { left: -420, right: -376, top: 400, bottom: 444 };
    atPoint = null;
    fire('touchstart', touch(elsewhere, 10, 90));
    expect(verdicts()).toEqual([]);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('stays quiet for an ordinary touch far from a reachable row', () => {
    freshWedgeState();
    fire('touchstart', touch(elsewhere, 10, 90));
    expect(verdicts()).toEqual([]);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('does not put a missed line under every touch while a wedge lasts', () => {
    // The finding gets its own line and its own latch. Widening the gate
    // instead would log every touch in the app for as long as the wedge held.
    freshWedgeState();
    atPoint = row;
    fire('touchstart', touch(elsewhere, 10, 90));
    vi.advanceTimersByTime(1000);
    fire('touchstart', touch(elsewhere, 10, 90));
    expect(verdicts().filter((v) => v === 'unreachable')).toHaveLength(1);
    expect(verdicts()).not.toContain('missed');
  });
});

/** The scheduled check's period, mirrored from the module. */
const TICK = 3000;

/** Let the row answer once. A healthy reading forgets both latches. They are
 *  keyed by face NAME, so they outlive the fresh `FakeEl` each case builds. */
function settleHealthy() {
  atPoint = send;
  vi.advanceTimersByTime(TICK);
  vi.advanceTimersByTime(200);
  postClientLog.mockClear();
  showToast.mockClear();
}

describe('the reading that does not wait to be touched', () => {
  // The tenth episode wrote no line at all, which proves the page took neither
  // a touch nor a click while the composer sat dead. Every other path in the
  // module needs one of those to arrive first.
  //
  // Full reconstruction:
  // docs/plans/2026-09-05-the-probe-speaks-when-no-face-can-take-the-press.md

  it('writes a line with no event dispatched at all', () => {
    settleHealthy();
    atPoint = row;
    vi.advanceTimersByTime(TICK);
    expect(verdicts()).toContain('unreachable');
    expect(lines()[0].scheduled).toBe(true);
  });

  it('stays quiet while no composer row is laid out', () => {
    settleHealthy();
    row.box = { left: 0, right: 0, top: 0, bottom: 0 };
    atPoint = null;
    vi.advanceTimersByTime(TICK);
    expect(verdicts()).toEqual([]);
  });

  it('stays quiet while the document is hidden', () => {
    settleHealthy();
    const doc = globalThis.document as unknown as Record<string, unknown>;
    doc.visibilityState = 'hidden';
    atPoint = row;
    vi.advanceTimersByTime(TICK);
    doc.visibilityState = 'visible';
    expect(verdicts()).toEqual([]);
  });

  it('writes once for a wedge that lasts, and again after one that returns', () => {
    settleHealthy();
    atPoint = row;
    vi.advanceTimersByTime(TICK * 4);
    expect(verdicts().filter((v) => v === 'unreachable')).toHaveLength(1);
    settleHealthy();
    atPoint = row;
    vi.advanceTimersByTime(TICK);
    expect(verdicts().filter((v) => v === 'unreachable')).toHaveLength(1);
  });
});

describe('a cover the app raised itself is not a wedge', () => {
  // The eleventh report was a false alarm. A client refresh dims and locks the
  // page, and the probe read the blocker at Cancel's own centre. It toasted the
  // wedge report over the app's own "Refreshing" status.

  function root(): Record<string, unknown> {
    return (globalThis.document as unknown as { documentElement: Record<string, unknown> })
      .documentElement;
  }

  /** Raise or drop `data-ui-blocked`, which is what `UiBlockingOverlay` sets. */
  function blockUi(on: boolean) {
    root().hasAttribute = (name: string) => on && name === 'data-ui-blocked';
  }

  // Every case in this file shares one root element. Put the shared setup's own
  // stub back, rather than leaving a lookalike behind.
  let priorHasAttribute: unknown;
  beforeEach(() => { priorHasAttribute = root().hasAttribute; });
  afterEach(() => { root().hasAttribute = priorHasAttribute; });

  it('stays quiet when the scheduled check runs under the blocker', () => {
    settleHealthy();
    blockUi(true);
    atPoint = row;                        // the cover answers, not the face
    vi.advanceTimersByTime(TICK);
    expect(verdicts()).toEqual([]);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('does not JUDGE a tap that landed on the blocker, but does record it', () => {
    // The blocker takes pointer events, so it answers at the composer's own
    // pixels. The landing report would name it on every frustrated tap.
    //
    // Standing the judgement down is right. Standing the LINE down was not: a
    // press the probe refused then read exactly like a press that never
    // arrived, which is the ambiguity the thirteenth episode died in.
    settleHealthy();
    blockUi(true);
    atPoint = row;
    fire('touchstart', touch(row, 350, 420));
    expect(verdicts()).toEqual(['covered']);
    expect(lines()[0].cover).toBe('data-ui-blocked');
    expect(showToast).not.toHaveBeenCalled();
  });

  it('names an open overlay as the cover when that is what is up', () => {
    settleHealthy();
    root().hasAttribute = (name: string) => name === 'data-overlay-open';
    atPoint = row;
    fire('touchstart', touch(row, 350, 420));
    expect(lines()[0].cover).toBe('data-overlay-open');
  });

  it('stays silent for a tap that reached neither the row nor a face', () => {
    // The noise this whole stand-down exists to avoid. Every tap inside an open
    // menu is the user working the overlay, not a composer that went dead.
    settleHealthy();
    blockUi(true);
    atPoint = elsewhere;
    fire('touchstart', touch(elsewhere, 40, 120));
    expect(verdicts()).toEqual([]);
  });

  it('writes one line for a flick across the covered row, not a stream', () => {
    settleHealthy();
    blockUi(true);
    atPoint = row;
    fire('touchstart', touch(row, 350, 420));
    fire('touchstart', touch(row, 350, 425));
    fire('touchstart', touch(row, 350, 430));
    expect(verdicts().filter((v) => v === 'covered')).toHaveLength(1);
  });

  it('counts the checks a cover stood down, so a silence can be read', () => {
    // The reading the thirteenth episode needed and did not have. Its line
    // carried 17 checks over 50 seconds of silence, and could not say whether
    // a cover had been up for all of them.
    settleHealthy();
    tapSend();
    vi.advanceTimersByTime(1000);
    postClientLog.mockClear();
    blockUi(true);
    vi.advanceTimersByTime(30000);
    root().hasAttribute = priorHasAttribute as () => boolean;
    tapSend();
    vi.advanceTimersByTime(1000);
    const press = lines().find((l) => l.verdict === 'dead' || l.verdict === 'clicked');
    expect(press?.quiet?.checks).toBeGreaterThan(0);
    expect(press?.quiet?.covered).toBe(press?.quiet?.checks);
    expect(press?.quiet?.unreachable).toBe(0);
  });

  it('reports a real wedge again once the blocker is gone', () => {
    // A stand-down, not a latch. The next check must still report a refresh
    // that left the row wedged behind it.
    settleHealthy();
    blockUi(true);
    atPoint = row;
    vi.advanceTimersByTime(TICK);
    blockUi(false);
    vi.advanceTimersByTime(TICK);
    expect(verdicts()).toContain('unreachable');
  });
});

describe('the repair, and whether it worked', () => {
  /** A style object that actually stores, so the restore can be asserted. The
   *  shared setup's stub answers the empty string for every property, which
   *  the module reads as a shell it does not own. */
  let props: Record<string, string>;
  let priorStyle: unknown;

  beforeEach(() => {
    const root = (globalThis.document as unknown as { documentElement: Record<string, unknown> })
      .documentElement;
    priorStyle = root.style;
    props = { '--app-height': '844px' };
    root.style = {
      setProperty: (k: string, v: string) => { props[k] = v; },
      getPropertyValue: (k: string) => props[k] ?? '',
      removeProperty: (k: string) => { delete props[k]; },
    };
  });

  afterEach(() => {
    const root = (globalThis.document as unknown as { documentElement: Record<string, unknown> })
      .documentElement;
    root.style = priorStyle;
  });

  /** Advance in steps shorter than the repair's own settle timer, stopping the
   *  moment the check reports. The shared interval's phase drifts across cases,
   *  so a single long advance can run the repair before the case can answer. */
  function tickUntilUnreachable() {
    for (let i = 0; i < 400 && !verdicts().includes('unreachable'); i++) {
      vi.advanceTimersByTime(20);
    }
  }

  it('says it worked when the face answers afterwards', () => {
    settleHealthy();
    atPoint = row;
    tickUntilUnreachable();
    atPoint = send;                       // the nudge took effect
    vi.advanceTimersByTime(200);
    expect(verdicts()).toContain('repaired');
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('stopped taking taps'),
      'warning',
    );
  });

  it('says it did not work when the face still will not answer', () => {
    settleHealthy();
    atPoint = row;
    vi.advanceTimersByTime(TICK);
    vi.advanceTimersByTime(200);
    expect(verdicts()).toContain('repair-failed');
    expect(showToast).not.toHaveBeenCalledWith(
      expect.stringContaining('stopped taking taps'),
      'warning',
    );
  });

  it('restores the height it nudged', () => {
    settleHealthy();
    atPoint = row;
    vi.advanceTimersByTime(TICK);
    vi.advanceTimersByTime(200);
    expect(props['--app-height']).toBe('844px');
  });

  it('never touches a healthy row', () => {
    settleHealthy();
    vi.advanceTimersByTime(TICK * 3);
    expect(verdicts()).toEqual([]);
    expect(props['--app-height']).toBe('844px');
  });

  it('spends one attempt per episode, not one per tick', () => {
    settleHealthy();
    atPoint = row;
    vi.advanceTimersByTime(TICK * 5);
    const attempts = verdicts().filter((v) => v === 'repaired' || v === 'repair-failed');
    expect(attempts).toHaveLength(1);
  });

  it('does not call a row that re-rendered under it a failed repair', () => {
    // The face left the document, so it answers nothing. Scoring that as a
    // failure would poison the split the repair exists to read.
    settleHealthy();
    atPoint = row;
    tickUntilUnreachable();
    send.isConnected = false;
    vi.advanceTimersByTime(200);
    const judged = lines().find((l) => l.verdict === 'repair-failed');
    expect(judged?.connected).toBe(false);
  });

  it('leaves a unit it does not own alone', () => {
    settleHealthy();
    props['--app-height'] = '100%';
    atPoint = row;
    vi.advanceTimersByTime(TICK);
    vi.advanceTimersByTime(200);
    expect(props['--app-height']).toBe('100%');
    expect(lines().find((l) => l.verdict === 'repair-failed')?.nudged).toBe(false);
  });

  it('declines to write a height the shell never set', () => {
    settleHealthy();
    delete props['--app-height'];
    atPoint = row;
    vi.advanceTimersByTime(TICK);
    vi.advanceTimersByTime(200);
    expect(verdicts()).toContain('repair-failed');
    expect(props['--app-height']).toBeUndefined();
  });
});

describe('a dead composer tap runs the recovery the user runs by hand', () => {
  // The reported state is a composer that takes no tap ANYWHERE until the
  // keyboard is dismissed and reopened. That bounce rewrites `--app-height`,
  // so the lift of a dead tap rewrites it too (ADR 0183).
  let props: Record<string, string>;
  let writes: string[];
  let priorStyle: unknown;

  beforeEach(() => {
    const el = (globalThis.document as unknown as { documentElement: Record<string, unknown> })
      .documentElement;
    priorStyle = el.style;
    writes = [];
    // The keyboard-up shell: shorter than the 844 layout viewport the harness
    // reports, which is exactly the pair the bounce travels between.
    props = { '--app-height': '476px' };
    el.style = {
      setProperty: (k: string, v: string) => { props[k] = v; if (k === '--app-height') writes.push(v); },
      getPropertyValue: (k: string) => props[k] ?? '',
      removeProperty: (k: string) => { delete props[k]; },
    };
  });

  afterEach(() => {
    const el = (globalThis.document as unknown as { documentElement: Record<string, unknown> })
      .documentElement;
    el.style = priorStyle;
  });

  /** The reported state: every face answers where it is drawn, and the finger
   *  still reaches nothing. A single `elementFromPoint` answer cannot say that,
   *  which is why the harness takes a per-point one. */
  function wedged() {
    atPointNear = (x: number, y: number) => (
      x >= SEND_BOX.left && x <= SEND_BOX.right && y >= SEND_BOX.top && y <= SEND_BOX.bottom
        ? send
        : row
    );
  }

  /** A stationary tap on row space that no face and no button answers. */
  function tapDeadSpace() {
    wedged();
    fire('touchstart', touch(row, 10, 420));
    fire('touchend', touch(row, 10, 420));
  }

  it('relayouts the shell by the span the keyboard takes, then puts it back', () => {
    // 476 shell inside an 844 layout viewport: a 368px keyboard, so the bounce
    // travels the same distance the user's does, DOWNWARD (see bounceHeight).
    settleHealthy();
    tapDeadSpace();
    vi.advanceTimersByTime(700);
    expect(writes).toEqual(['108px', '476px']);
    expect(props['--app-height']).toBe('476px');
  });

  it('spends it on the FIRST dead tap, not the tenth', () => {
    settleHealthy();
    tapDeadSpace();
    vi.advanceTimersByTime(700);
    expect(writes.length).toBeGreaterThan(0);
  });

  it('says so in the log', () => {
    settleHealthy();
    tapDeadSpace();
    vi.advanceTimersByTime(700);
    const repair = lines().find((l) => l.verdict === 'repaired' || l.verdict === 'repair-failed');
    expect(repair?.nudged).toBe(true);
  });

  it('never toasts, because nothing here can score the repair', () => {
    // The face answers at its own centre throughout this state, so a toast
    // would claim a fix on every stray tap on empty row space.
    settleHealthy();
    tapDeadSpace();
    vi.advanceTimersByTime(700);
    expect(showToast).not.toHaveBeenCalledWith(
      expect.stringContaining('stopped taking taps'),
      'warning',
    );
  });

  it('leaves a swipe that began on the row alone', () => {
    settleHealthy();
    wedged();
    fire('touchstart', touch(row, 10, 420));
    fire('touchmove', touch(row, 10, 500));
    fire('touchend', touch(row, 10, 500));
    vi.advanceTimersByTime(700);
    expect(writes).toEqual([]);
  });

  it('leaves a gesture the system took alone', () => {
    settleHealthy();
    wedged();
    fire('touchstart', touch(row, 10, 420));
    fire('touchcancel', touch(row, 10, 420));
    vi.advanceTimersByTime(700);
    expect(writes).toEqual([]);
  });

  it('leaves a press the app drops on purpose alone', () => {
    // An excluded face under the finger is a no-op the app chose. Only a tap
    // that reached NOTHING is the dead composer this recovers.
    settleHealthy();
    wedged();
    send.disabled = true;
    fire('touchstart', touch(row, 350, 420));
    fire('touchend', touch(row, 350, 420));
    vi.advanceTimersByTime(700);
    expect(lines().find((l) => l.verdict === 'missed')?.under).toBe('disabled-face');
    expect(writes).toEqual([]);
  });
});

describe('a touch the composer never saw still leaves a line', () => {
  // The blind spot twelve episodes died in: "no line" meant both "iOS
  // delivered no touch" and "iOS delivered it somewhere else", which have
  // different fixes and no shared one (ADR 0183).
  let priorHasAttribute: unknown;

  function root(): Record<string, unknown> {
    return (globalThis.document as unknown as { documentElement: Record<string, unknown> })
      .documentElement;
  }

  beforeEach(() => { priorHasAttribute = root().hasAttribute; });
  afterEach(() => { root().hasAttribute = priorHasAttribute; });

  it('records where it landed, what answered, and the screen offset', () => {
    settleHealthy();
    root().hasAttribute = (name: string) => name === 'data-keyboard-active';
    atPoint = elsewhere;
    fire('touchstart', touch(elsewhere, 40, 120, 1, 0, 59));
    const stray = lines().find((l) => l.verdict === 'keyboard-touch');
    expect(stray?.point).toEqual({ x: 40, y: 120 });
    expect(stray?.screenOff).toEqual({ x: 59, y: 59 });
    expect(stray?.rowRect).toEqual(ROW_BOX);
  });

  it('says nothing while the keyboard is down, which no report describes', () => {
    settleHealthy();
    root().hasAttribute = () => false;
    atPoint = elsewhere;
    fire('touchstart', touch(elsewhere, 40, 120));
    expect(verdicts()).not.toContain('keyboard-touch');
  });

  it('writes one line for a flick, not a stream', () => {
    settleHealthy();
    root().hasAttribute = (name: string) => name === 'data-keyboard-active';
    atPoint = elsewhere;
    fire('touchstart', touch(elsewhere, 40, 120));
    fire('touchstart', touch(elsewhere, 40, 130));
    fire('touchstart', touch(elsewhere, 40, 140));
    expect(verdicts().filter((v) => v === 'keyboard-touch')).toHaveLength(1);
  });
});

describe('a dead tap runs Send itself', () => {
  // The app knows enough to do this: a touch reached the document, inside the
  // composer row, no button claimed it, and Send is live (ADR 0183). A
  // relayout helps the NEXT tap; this answers the one just made.
  let priorHasAttribute: unknown;

  function root(): Record<string, unknown> {
    return (globalThis.document as unknown as { documentElement: Record<string, unknown> })
      .documentElement;
  }

  function wedgedWithKeyboardUp() {
    atPointNear = (x: number, y: number) => (
      x >= SEND_BOX.left && x <= SEND_BOX.right && y >= SEND_BOX.top && y <= SEND_BOX.bottom
        ? send
        : row
    );
    root().hasAttribute = (name: string) => name === 'data-keyboard-active';
  }

  /** The state the caught episode was in. The face answers at its own centre
   *  throughout, the finger reaches nothing, and no textarea is focused. */
  function keyboardDownAndWedged() {
    atPointNear = (x: number, y: number) => (
      x >= send.box.left && x <= send.box.right && y >= send.box.top && y <= send.box.bottom
        ? send
        : row
    );
    root().hasAttribute = () => false;
  }

  function tapDeadSpace() {
    fire('touchstart', touch(row, 10, 420));
    fire('touchend', touch(row, 10, 420));
  }

  beforeEach(() => { priorHasAttribute = root().hasAttribute; });
  afterEach(() => { root().hasAttribute = priorHasAttribute; });

  it('sends, and says it did', () => {
    settleHealthy();
    wedgedWithKeyboardUp();
    tapDeadSpace();
    vi.advanceTimersByTime(700);
    expect(send.clicks).toBe(1);
    expect(verdicts()).toContain('activated');
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('did not register'),
      'warning',
    );
  });

  it('never stops a running turn', () => {
    // The same node in cancel mode. A dropped tap must not cancel the run.
    settleHealthy();
    wedgedWithKeyboardUp();
    send.label = 'Cancel';
    tapDeadSpace();
    vi.advanceTimersByTime(700);
    expect(send.clicks).toBe(0);
  });

  it('runs anyway when the dead press’s own click lands on the row', () => {
    // The fourteenth episode. A tap on the row dispatches a click ON the row,
    // an inert div. The rescue used to stand down on any click at all. So
    // every dead press killed its own recovery inside the grace window, and
    // the whole engine ledger holds no rescue.
    settleHealthy();
    wedgedWithKeyboardUp();
    tapDeadSpace();
    fire('click', { target: row });
    vi.advanceTimersByTime(700);
    expect(send.clicks).toBe(1);
  });

  it('stands down when the click landed on something that could take it', () => {
    settleHealthy();
    wedgedWithKeyboardUp();
    tapDeadSpace();
    fire('click', { target: send });
    vi.advanceTimersByTime(700);
    expect(send.clicks).toBe(0);
  });

  it('stands down when the click landed outside the row entirely', () => {
    settleHealthy();
    wedgedWithKeyboardUp();
    tapDeadSpace();
    fire('click', { target: elsewhere });
    vi.advanceTimersByTime(700);
    expect(send.clicks).toBe(0);
  });

  it('runs with the keyboard down, which the fourteenth report describes', () => {
    // The bound the previous round drew, and the state the caught episode was
    // in: `keyboardActive false`, the row resting at the foot of the screen.
    settleHealthy();
    keyboardDownAndWedged();
    tapDeadSpace();
    vi.advanceTimersByTime(700);
    expect(send.clicks).toBe(1);
    expect(verdicts()).toContain('activated');
  });

  it('runs for a press just outside the face, as the caught one was', () => {
    // 9px above Send's top edge, on the strip of bare row above it. Where in
    // the row the finger fell changes nothing: the reporter refused a bound on
    // it, and every round that read this as aim has been wrong.
    settleHealthy();
    // The row's height comes from its icon cluster, so the commit face is
    // shorter and rests on the row's foot with bare row above it.
    send.box = { left: 330, right: 374, top: 419, bottom: 444 };
    keyboardDownAndWedged();
    fire('touchstart', touch(row, 352, 410));
    fire('touchend', touch(row, 352, 410));
    vi.advanceTimersByTime(700);
    expect(send.clicks).toBe(1);
    expect(lines().find((l) => l.verdict === 'missed')?.missedBy?.px).toBe(9);
  });

  it('stands down under a cover the app raised in the meantime', () => {
    // A synthetic click ignores the inert behind an overlay, so the rescue has
    // to ask. Under one, the composer is unreachable by design.
    settleHealthy();
    wedgedWithKeyboardUp();
    tapDeadSpace();
    root().hasAttribute = (name: string) => name === 'data-overlay-open'
      || name === 'data-keyboard-active';
    vi.advanceTimersByTime(700);
    expect(send.clicks).toBe(0);
  });

  it('stands down when the finger travelled', () => {
    settleHealthy();
    wedgedWithKeyboardUp();
    fire('touchstart', touch(row, 10, 420));
    fire('touchmove', touch(row, 10, 500));
    fire('touchend', touch(row, 10, 500));
    vi.advanceTimersByTime(700);
    expect(send.clicks).toBe(0);
  });

  it('lets a pending rescue fire even though the next gesture travelled', () => {
    // Tapping and then swiping is what a user does to a dead-feeling button.
    // The swipe is judged in front of the grace window, so it cannot cancel
    // the rescue the tap before it already earned.
    settleHealthy();
    wedgedWithKeyboardUp();
    tapDeadSpace();
    vi.advanceTimersByTime(200);
    fire('touchstart', touch(row, 10, 420));
    fire('touchmove', touch(row, 10, 500));
    fire('touchend', touch(row, 10, 500));
    vi.advanceTimersByTime(700);
    expect(send.clicks).toBe(1);
  });

  it('never reads its own click as the page taking clicks with no touch', () => {
    // The rescue dispatches at touchend + 600ms, so a finger held past 900ms
    // puts that click outside the touch-behind window. Unflagged, the probe
    // writes `click-no-touch` about a click it made itself, which is the
    // verdict meaning the touch pipeline is dead.
    settleHealthy();
    wedgedWithKeyboardUp();
    fire('touchstart', touch(row, 10, 420));
    vi.advanceTimersByTime(1000);
    fire('touchend', touch(row, 10, 420));
    vi.advanceTimersByTime(700);
    expect(send.clicks).toBe(1);
    expect(verdicts()).not.toContain('click-no-touch');
    expect(verdicts()).toContain('activated');
  });

  describe('and a refusal it could have answered is written down', () => {
    // Two of the three bounds used to refuse in silence, so a ledger with no
    // rescue in it could not say which one had. A refusal is only worth a line
    // where there was a face to run, which is the rare case.

    function declined() {
      return lines().find((l) => l.verdict === 'rescue-stood-down');
    }

    it('names the claim that took the press', () => {
      settleHealthy();
      wedgedWithKeyboardUp();
      tapDeadSpace();
      fire('click', { target: send });
      vi.advanceTimersByTime(700);
      expect(declined()?.standDown).toBe('claimed');
      expect(declined()?.face).toBe('Send message');
    });

    it('names the cover that went up inside the grace window', () => {
      settleHealthy();
      wedgedWithKeyboardUp();
      tapDeadSpace();
      root().hasAttribute = (name: string) => name === 'data-overlay-open'
        || name === 'data-keyboard-active';
      vi.advanceTimersByTime(700);
      expect(declined()?.standDown).toBe('covered');
    });

    it('names the travel that took the gesture', () => {
      settleHealthy();
      wedgedWithKeyboardUp();
      fire('touchstart', touch(row, 10, 420));
      fire('touchmove', touch(row, 10, 500));
      fire('touchend', touch(row, 10, 500));
      vi.advanceTimersByTime(700);
      expect(declined()?.standDown).toBe('traveled');
    });

    it('says nothing when there was no commit face to run', () => {
      // Most taps in this row. 68 of the 69 the caught ledger holds.
      settleHealthy();
      wedgedWithKeyboardUp();
      send.label = 'Cancel';
      tapDeadSpace();
      vi.advanceTimersByTime(700);
      expect(verdicts()).not.toContain('rescue-stood-down');
    });
  });

  describe('and in answer mode it runs the Submit', () => {
    // The mode the thirteenth episode was in. `PromptInput` draws the answer
    // control INSTEAD of the morph, so the rescue used to find no face and
    // relayout alone. The user's typed answer sat unsent for two and three
    // quarter minutes.
    const SUBMIT_BOX: Box = { left: 300, right: 374, top: 400, bottom: 444 };
    let submit: FakeEl;

    /** Answer mode as the DOM holds it: no morph, one confirm Submit. */
    function answering(label = 'Submit answer') {
      submit = new FakeEl(label, SUBMIT_BOX, ['action-btn', 'action-btn-confirm'], row);
      faces = [submit];
      // The face answers at its own centre throughout, which is the wedge this
      // rescue is for: the finger reaches nothing while the layout is healthy.
      atPointNear = (x: number) => (
        x >= SUBMIT_BOX.left && x <= SUBMIT_BOX.right ? submit : row
      );
      root().hasAttribute = (name: string) => name === 'data-keyboard-active';
    }

    it('submits the typed answer, and says so', () => {
      settleHealthy();
      answering();
      tapDeadSpace();
      vi.advanceTimersByTime(700);
      expect(submit.clicks).toBe(1);
      expect(verdicts()).toContain('activated');
      expect(showToast).toHaveBeenCalledWith(
        expect.stringContaining('Submit was run for you'),
        'warning',
      );
    });

    it('names the face it ran, so the ledger says which one', () => {
      settleHealthy();
      answering();
      tapDeadSpace();
      vi.advanceTimersByTime(700);
      expect(lines().find((l) => l.verdict === 'activated')?.face).toBe('Submit answer');
    });

    it('refuses a Submit the app disabled, such as a multi-select at zero', () => {
      settleHealthy();
      answering();
      submit.disabled = true;
      tapDeadSpace();
      vi.advanceTimersByTime(700);
      expect(submit.clicks).toBe(0);
    });

    it('refuses any other confirm face, Apply above all', () => {
      // Apply wears the same green. Running it on a tap nobody saw land would
      // merge a change the user never approved.
      settleHealthy();
      answering('Apply');
      tapDeadSpace();
      vi.advanceTimersByTime(700);
      expect(submit.clicks).toBe(0);
      expect(verdicts()).not.toContain('activated');
    });
  });
});

describe('every press line carries the one reading layout cannot fake', () => {
  // `screenX/Y` is physical and `clientX/Y` is what the page hit-tests with.
  // Their difference is a constant while the mapping is sane, so a jump in it
  // is the fault stated rather than inferred (ADR 0183).
  it('records it for a press that reached a face', () => {
    fire('touchstart', touch(send, 350, 420, 1, 0, 59));
    fire('touchend', touch(send, 350, 420, 1, 0, 59));
    vi.advanceTimersByTime(1000);
    expect(lines()[0].screenOff).toEqual({ x: 59, y: 59 });
  });

  it('records it for a press that reached nothing', () => {
    atPoint = row;
    fire('touchstart', touch(row, 10, 420, 1, 0, 59));
    expect(lines().find((l) => l.verdict === 'missed')?.screenOff).toEqual({ x: 59, y: 59 });
  });
});

describe('every line brackets the silence before it', () => {
  it('carries the gap, and what the checks saw across it', () => {
    // The recovery input is the one event guaranteed to arrive, so it is the
    // one moment a deaf window can be measured from.
    settleHealthy();
    tapSend();
    vi.advanceTimersByTime(1000);
    postClientLog.mockClear();
    atPoint = row;
    vi.advanceTimersByTime(30000);
    atPoint = send;
    tapSend();
    vi.advanceTimersByTime(1000);
    const press = lines().find((l) => l.verdict === 'dead' || l.verdict === 'clicked');
    expect(press?.quiet?.ms).toBeGreaterThanOrEqual(30000);
    expect(press?.quiet?.checks).toBeGreaterThan(0);
    expect(press?.quiet?.unreachable).toBeGreaterThan(0);
  });

  it('does not let a synthetic click erase the silence its own tap ended', () => {
    // The paired click lands about 50ms after the `touchstart`, and the press
    // records 600ms later still. Resetting on that click handed the press
    // which ENDED a silence a 50ms window, losing the whole reading.
    settleHealthy();
    tapSend();
    vi.advanceTimersByTime(1000);
    postClientLog.mockClear();
    vi.advanceTimersByTime(30000);
    fire('touchstart', touch(send, 350, 420));
    fire('touchend', touch(send, 350, 420));
    fire('click', { target: send });
    vi.advanceTimersByTime(1000);
    const press = lines().find((l) => l.verdict === 'clicked');
    expect(press?.quiet?.ms).toBeGreaterThanOrEqual(30000);
  });

  it('reports a quiet stretch the checks found healthy as exactly that', () => {
    settleHealthy();
    tapSend();
    vi.advanceTimersByTime(1000);
    postClientLog.mockClear();
    vi.advanceTimersByTime(30000);
    tapSend();
    vi.advanceTimersByTime(1000);
    const press = lines().find((l) => l.verdict === 'dead' || l.verdict === 'clicked');
    expect(press?.quiet?.checks).toBeGreaterThan(0);
    expect(press?.quiet?.unreachable).toBe(0);
  });
});

describe('a missed press says why no face took it', () => {
  it('names an excluded action face under the finger', () => {
    // The distinction the ledger never carried. A tap on empty row space and a
    // tap on a dead action face used to write the same line.
    settleHealthy();
    send.disabled = true;
    atPoint = send;
    fire('touchstart', touch(row, 350, 420));
    const [line] = lines();
    expect(line.under).toBe('disabled-face');
    expect(line.underFace).toBe('Send message');
    expect(line.faceCount).toBe(1);
    expect(line.watchableCount).toBe(0);
  });

  it('calls the invisible placeholder by its own name, not disabled', () => {
    settleHealthy();
    send.disabled = true;
    send.classes = ['action-btn', 'send-cancel-morph', 'morph-placeholder'];
    atPoint = send;
    fire('touchstart', touch(row, 350, 420));
    expect(lines()[0].under).toBe('placeholder-face');
  });

  it('says nothing was under a tap on empty row space', () => {
    settleHealthy();
    atPoint = row;
    fire('touchstart', touch(row, 10, 420));
    const missed = lines().find((l) => l.verdict === 'missed');
    expect(missed?.under).toBe('nothing');
    expect(missed?.underFace).toBeNull();
  });

  it('measures how far outside the nearest face the finger fell', () => {
    // A tap just right of Send, which is the strip the row's padding leaves
    // beside it. Both readings together are what tell this apart from a tap
    // nowhere near a target.
    settleHealthy();
    atPoint = row;
    fire('touchstart', touch(row, 382, 420));
    const missed = lines().find((l) => l.verdict === 'missed');
    expect(missed?.point).toEqual({ x: 382, y: 420 });
    // The face's own box travels with the distance. A missed line's `faceRect`
    // is null by construction, so without this the reader has to reconstruct
    // the geometry from the row's.
    expect(missed?.missedBy).toEqual({ face: 'Send message', px: 8, dx: 8, dy: 0, rect: SEND_BOX });
  });

  it('measures a press nowhere near a face by the same yardstick', () => {
    settleHealthy();
    atPoint = row;
    fire('touchstart', touch(row, 10, 420));
    expect(lines().find((l) => l.verdict === 'missed')?.missedBy?.px).toBe(320);
  });
});

describe('every line carries the geometry a report used to only imply', () => {
  it('records the row and face boxes on a watched press', () => {
    tapSend();
    vi.advanceTimersByTime(1000);
    const [line] = lines();
    expect(line.rowRect).toEqual(ROW_BOX);
    expect(line.faceRect).toEqual(SEND_BOX);
  });

  it('stays well inside the engine’s 4KB cap on its richest shape', () => {
    atPoint = row;
    fire('touchstart', touch(row, 10, 90));
    const [line] = lines();
    expect(JSON.stringify(line).length).toBeLessThan(4096);
  });

  it('keeps the two newest shapes inside it too, and free of what was typed', () => {
    const root = (globalThis.document as unknown as { documentElement: Record<string, unknown> })
      .documentElement;
    const prior = root.hasAttribute;
    root.hasAttribute = (name: string) => name === 'data-overlay-open';
    atPoint = row;
    fire('touchstart', touch(row, 350, 420));
    root.hasAttribute = (name: string) => name === 'data-keyboard-active';
    vi.advanceTimersByTime(2000);
    fire('click', { target: elsewhere, clientX: 40, clientY: 120 });
    root.hasAttribute = prior;
    for (const line of lines()) {
      expect(JSON.stringify(line).length).toBeLessThan(4096);
    }
    expect(verdicts()).toEqual(['covered', 'stray-click']);
  });
});

describe('a press line describes one instant, not two', () => {
  // The fourteenth episode's readings looked like two composers. Rects came
  // from touchdown and the viewport came from 600ms after the lift. By then
  // the submitted answer had dismissed the keyboard and regrown the shell.
  // Both readings belong to the press now (see `PressContext`).
  let props: Record<string, string>;
  let priorStyle: unknown;

  function docEl(): Record<string, unknown> {
    return (globalThis.document as unknown as { documentElement: Record<string, unknown> })
      .documentElement;
  }

  beforeEach(() => {
    priorStyle = docEl().style;
    props = { '--app-height': '476px' };
    docEl().style = {
      setProperty: (k: string, v: string) => { props[k] = v; },
      getPropertyValue: (k: string) => props[k] ?? '',
      removeProperty: (k: string) => { delete props[k]; },
    };
  });

  afterEach(() => { docEl().style = priorStyle; });

  it('carries the viewport the press saw, not the one the ruling saw', () => {
    settleHealthy();
    fire('touchstart', touch(send, 350, 420));
    fire('touchend', touch(send, 350, 420));
    // The keyboard goes down while the press settles, exactly as a submitted
    // answer takes it down.
    props['--app-height'] = '852px';
    vi.advanceTimersByTime(1000);
    expect(lines()[0].viewport?.appHeight).toBe('476px');
  });

  it('carries the morph the press saw, not the one the ruling saw', () => {
    // `absent` on the press and `disabled` two seconds later is what gave the
    // split away, and only by accident.
    settleHealthy();
    fire('touchstart', touch(send, 350, 420));
    fire('touchend', touch(send, 350, 420));
    send.label = 'Cancel';
    vi.advanceTimersByTime(1000);
    expect(lines()[0].morph).toBe('send');
  });

  it('quotes the press’s viewport in the TOAST too, not the ruling’s', () => {
    // The reporter screenshots the toast. A toast quoting the ruling's layout
    // beside a line quoting the press's is the same two-clocks confusion in
    // the other output channel.
    settleHealthy();
    fire('touchstart', touch(send, 350, 420));
    fire('touchend', touch(send, 350, 420));
    props['--app-height'] = '852px';
    vi.advanceTimersByTime(1000);
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('app 476px'), 'warning');
  });

  it('carries the silence the press ended, not a later one', () => {
    settleHealthy();
    vi.advanceTimersByTime(5000);
    fire('touchstart', touch(send, 350, 420));
    fire('touchend', touch(send, 350, 420));
    vi.advanceTimersByTime(1000);
    expect(lines()[0].quiet?.ms ?? 0).toBeGreaterThanOrEqual(5000);
  });
});

// LAST in the file, and that is load-bearing. These are the only cases that
// type, and the keystroke clock is module state shared by every case. A case
// running after one of them would see a composer waiting to be tapped.
describe('the recovery a composer nobody can touch still gets', () => {
  // The fourteenth PWA episode. The page took no touch and no click for 18.5
  // seconds while the draft sat unsent. Six scheduled checks found the Send
  // face reachable, and every keystroke reached the box. Both of ADR 0183's
  // answers wait to be touched, so neither could fire.
  //
  // Full reconstruction:
  // docs/plans/2026-09-17-the-composer-recovers-without-being-touched.md

  let props: Record<string, string>;
  let priorStyle: unknown;

  function root(): Record<string, unknown> {
    return (globalThis.document as unknown as { documentElement: Record<string, unknown> })
      .documentElement;
  }

  let priorHasAttribute: unknown;

  beforeEach(() => {
    priorStyle = root().style;
    props = { '--app-height': '476px' };
    root().style = {
      setProperty: (k: string, v: string) => { props[k] = v; },
      getPropertyValue: (k: string) => props[k] ?? '',
      removeProperty: (k: string) => { delete props[k]; },
    };
    priorHasAttribute = root().hasAttribute;
  });

  afterEach(() => {
    root().style = priorStyle;
    root().hasAttribute = priorHasAttribute;
    // Close the typing clock, so the next case starts from a touched page.
    atPoint = send;
    fire('touchstart', touch(elsewhere, 10, 90));
  });

  /** A page that has just been touched, with every face answering.
   *
   *  The 1ms is what makes the ordering expressible. One fake clock serves the
   *  whole file, so a touch and a keystroke fired back to back share a
   *  millisecond. "The user typed since the page was touched" then cannot be
   *  said at all. A real device never delivers the two in one instant. */
  function settleTouched() {
    atPoint = send;
    fire('touchstart', touch(elsewhere, 10, 90));
    vi.advanceTimersByTime(1);
    postClientLog.mockClear();
    showToast.mockClear();
  }

  /** One character into the composer's textarea. */
  function type() {
    fire('input', { target: { dataset: { role: 'prompt-input' } } });
  }

  function untouched(): Line[] {
    return lines().filter((l) => l.verdict === 'untouched');
  }

  it('relayouts for a typed draft the page has taken no touch since', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(TICK * 2);
    expect(untouched()).not.toHaveLength(0);
    expect(untouched()[0].face).toBe('Send message');
    expect(untouched()[0].nudged).toBe(true);
    expect(untouched()[0].scheduled).toBe(true);
  });

  it('restores the height it nudged', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(TICK * 2);
    expect(props['--app-height']).toBe('476px');
  });

  it('never commits, because a silence is not a press', () => {
    // A dropped press is evidence the user reached for the button. Nothing
    // here is, so the relayout is the whole answer.
    settleTouched();
    const before = send.clicks;
    type();
    vi.advanceTimersByTime(TICK * 6);
    expect(send.clicks).toBe(before);
  });

  it('stays out of a healthy send, where the tap follows the typing', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(2000);
    expect(untouched()).toHaveLength(0);
  });

  it('stays quiet on a composer nobody has typed into', () => {
    settleTouched();
    vi.advanceTimersByTime(TICK * 4);
    expect(untouched()).toHaveLength(0);
  });

  it('stands down once a touch does reach the page', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(1000);
    settleTouched();
    vi.advanceTimersByTime(TICK * 4);
    expect(untouched()).toHaveLength(0);
  });

  it('has nothing to protect while a turn is running', () => {
    // The morph reads `cancel`, so the row holds a Stop rather than a commit
    // face. Relaying out for it would be a nudge on nobody's behalf.
    settleTouched();
    send.label = 'Cancel';
    type();
    vi.advanceTimersByTime(TICK * 4);
    expect(untouched()).toHaveLength(0);
  });

  it('stays quiet under a cover the app raised itself', () => {
    settleTouched();
    root().hasAttribute = (name: string) => name === 'data-overlay-open';
    type();
    vi.advanceTimersByTime(TICK * 4);
    expect(untouched()).toHaveLength(0);
  });

  it('keeps trying right through the stretch the user spends tapping', () => {
    // A count cap failed here. The wedge delivers nothing that reopens a quiet
    // window. So a budget spent while the user re-read the draft could never
    // come back for the taps that follow.
    settleTouched();
    type();
    vi.advanceTimersByTime(20_000);
    expect(untouched().length).toBeGreaterThan(4);
  });

  it('stops once the user has plainly put the phone down', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(TICK * 30);
    const spent = untouched().length;
    expect(spent).toBeGreaterThan(0);
    vi.advanceTimersByTime(TICK * 30);
    expect(untouched()).toHaveLength(spent);
  });

  it('stands aside while a reachability wedge is latched and repairing', () => {
    // `firstUnreachableFace` answers null for a face already reported, so from
    // the second tick a real wedge looks like a healthy row. `attemptRepair`
    // owns that episode and has spent its one attempt. Nudging over it would
    // poison the reading both recoveries are scored by.
    settleTouched();
    type();
    atPoint = row;                        // the face stops answering
    vi.advanceTimersByTime(TICK * 8);
    expect(verdicts()).toContain('unreachable');
    expect(untouched()).toHaveLength(0);
  });

  it('counts them onto the press that ends the silence', () => {
    // The only thing that can score this recovery. A press arriving with a
    // nudge behind it is what the next episode reads.
    settleTouched();
    type();
    vi.advanceTimersByTime(TICK * 2);
    postClientLog.mockClear();
    tapSend();
    vi.advanceTimersByTime(1000);
    const press = lines().find((l) => l.verdict === 'dead' || l.verdict === 'clicked');
    expect(press?.quiet?.nudges ?? 0).toBeGreaterThan(0);
  });

  it('starts the next silence at no nudges', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(TICK * 2);
    settleTouched();
    vi.advanceTimersByTime(1000);
    tapSend();
    vi.advanceTimersByTime(1000);
    const press = lines().find((l) => l.verdict === 'dead' || l.verdict === 'clicked');
    expect(press?.quiet?.nudges).toBe(0);
  });

  it('leaves the quiet window measuring touches, never keystrokes', () => {
    // Typing must not close the window. It is the one reading that brackets a
    // silence, and a keystroke resetting it would erase the episode.
    settleTouched();
    vi.advanceTimersByTime(4000);
    type();
    vi.advanceTimersByTime(4000);
    tapSend();
    vi.advanceTimersByTime(1000);
    const press = lines().find((l) => l.verdict === 'dead' || l.verdict === 'clicked');
    expect(press?.quiet?.ms ?? 0).toBeGreaterThanOrEqual(8000);
  });
});
