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
  click() { this.clicks += 1; }
  contains(other: unknown) { return other === this; }
  closest(sel: string) {
    if (sel !== '.prompt-actions-row') return null;
    if (this.classes.includes('prompt-actions-row')) return this;
    return this.row;
  }
}

const ROW_BOX: Box = { left: 0, right: 390, top: 400, bottom: 444 };
const SEND_BOX: Box = { left: 330, right: 374, top: 400, bottom: 444 };

let row: FakeEl;
let send: FakeEl;
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
    if (sel === '.prompt-actions-row .action-btn') return [send];
    return [];
  };
  doc.elementFromPoint = (x: number, y: number) => (atPointNear ? atPointNear(x, y) : atPoint);
  doc.querySelector = (sel: string) => (
    sel === '.prompt-actions-row .send-cancel-morph' ? send : null
  );
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
  missedBy?: { face: string; px: number } | null;
  /** The silence that ended at the input this line belongs to. */
  quiet?: { ms: number; checks: number; unreachable: number } | null;
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

  it('stays quiet for a click that is nowhere near a composer face', () => {
    noRecentTouch();
    fire('click', { target: elsewhere });
    expect(verdicts()).toEqual([]);
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

  it('does not report a tap that landed on the blocker', () => {
    // The blocker takes pointer events, so it answers at the composer's own
    // pixels. The landing report would name it on every frustrated tap.
    settleHealthy();
    blockUi(true);
    atPoint = row;
    fire('touchstart', touch(row, 350, 420));
    expect(verdicts()).toEqual([]);
    expect(showToast).not.toHaveBeenCalled();
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

  it('stands down when a click arrives, since the press was not dead', () => {
    settleHealthy();
    wedgedWithKeyboardUp();
    tapDeadSpace();
    fire('click', { target: row });
    vi.advanceTimersByTime(700);
    expect(send.clicks).toBe(0);
  });

  it('stands down with the keyboard down, which no report describes', () => {
    settleHealthy();
    atPointNear = (x: number, y: number) => (
      x >= SEND_BOX.left && x <= SEND_BOX.right && y >= SEND_BOX.top && y <= SEND_BOX.bottom
        ? send
        : row
    );
    root().hasAttribute = () => false;
    tapDeadSpace();
    vi.advanceTimersByTime(700);
    expect(send.clicks).toBe(0);
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
    expect(missed?.missedBy).toEqual({ face: 'Send message', px: 8 });
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
});
