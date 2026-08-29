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
  private label: string | null;
  /** Public and mutable, so a case can move a face BETWEEN two taps and tell
   *  the resulting lines apart by the box each press snapshotted. */
  box: Box;
  private row: FakeEl | null;

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

function installDom() {
  const doc = globalThis.document as unknown as Record<string, unknown>;
  doc.querySelectorAll = (sel: string) => {
    if (sel === '.prompt-actions-row') return [row];
    if (sel === '.prompt-actions-row .action-btn') return [send];
    return [];
  };
  doc.elementFromPoint = () => atPoint;
  (globalThis as unknown as Record<string, unknown>).MutationObserver = class {
    observe() { /* the row is never mutated in these cases */ }
    disconnect() { /* nothing to release */ }
  };
}

/** `id` is the finger. The probe binds a press to one identifier, so a second
 *  finger's lift or travel must not settle or move the first finger's press. */
function touch(el: unknown, x: number, y: number, fingers = 1, id = 0) {
  const point = { clientX: x, clientY: y, screenX: x, screenY: y, identifier: id };
  return { target: el, changedTouches: [point], touches: new Array(fingers).fill(point) };
}

function fire(type: string, event: Record<string, unknown>) {
  (globalThis.document as unknown as { dispatchEvent(e: unknown): void })
    .dispatchEvent({ type, ...event });
}

/** Every `composer-press` line written so far, newest last. */
function lines(): Array<{ face: string; verdict: string; rowRect?: Box; faceRect?: Box }> {
  return postClientLog.mock.calls
    .filter((c) => c[0] === 'composer-press')
    .map((c) => c[2] as { face: string; verdict: string; rowRect?: Box; faceRect?: Box });
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
    expect(verdicts()).toEqual(['unreachable']);
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
    expect(verdicts()).toEqual(['unreachable']);
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
