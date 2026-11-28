import { describe, it, expect, beforeAll, afterAll, beforeEach, afterEach, vi } from 'vitest';


const showToast = vi.hoisted(() => vi.fn());
const postClientLog = vi.hoisted(() => vi.fn());
vi.mock('../../../store/store', () => ({ showToast }));
vi.mock('../../../utils/clientLog', () => ({ postClientLog }));

import {
  installDeadPressProbe,
  _resetNudgeStateForTesting,
  LIFT_DEADLINE_MS,
  SCHEDULED_CHECK_MS,
  UNTOUCHED_QUIET_MS,
} from '../deadPressProbe';
import {
  noteViewportResize,
  resetKeyboardCloseState,
} from '../../layout/keyboardCloseRelayout';
import { notePressOutcome } from '../../../utils/tapGesture';

interface Box { left: number; right: number; top: number; bottom: number }

class FakeEl {
  disabled = false;
  isConnected = true;
  textContent = '';
  classes: string[];
  label: string | null;
  box: Box;
  private row: FakeEl | null;

  clicks = 0;
  threadAction = false;

  constructor(label: string | null, box: Box, classes: string[] = ['action-btn'], row: FakeEl | null = null) {
    this.label = label;
    this.box = box;
    this.classes = classes;
    this.row = row;
  }

  private listeners: { type: string; fn: (e: unknown) => void; signal?: AbortSignal }[] = [];
  addEventListener(type: string, fn: (e: unknown) => void, opts?: { signal?: AbortSignal }) {
    this.listeners.push({ type, fn, signal: opts?.signal });
  }
  dispatch(e: { type: string }) {
    for (const l of this.listeners) if (l.type === e.type && !l.signal?.aborted) l.fn(e);
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
  click() {
    this.clicks += 1;
    fire('click', { target: this });
  }
  contains(other: unknown) { return other === this; }
  closest(sel: string) {
    if (sel === '[data-thread-action]') return this.threadAction ? this : null;
    if (sel.startsWith('button,')) {
      return this.classes.some((c) => c === 'action-btn' || c === 'icon-btn') ? this : null;
    }
    if (sel === '.prompt-box') {
      if (this.classes.includes('prompt-actions-row') || this.classes.includes('prompt-textarea')) {
        return this;
      }
      return this.row;
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
let faces: FakeEl[];
let elsewhere: FakeEl;
let textarea: FakeEl;
let atPoint: unknown = null;
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
    observe() {  }
    disconnect() {  }
  };
}

function touch(el: unknown, x: number, y: number, fingers = 1, id = 0, screenOff = 0) {
  const point = {
    clientX: x, clientY: y, screenX: x + screenOff, screenY: y + screenOff, identifier: id,
  };
  return { target: el, changedTouches: [point], touches: new Array(fingers).fill(point) };
}

function fire(type: string, event: Record<string, unknown>) {
  const e: Record<string, unknown> = { type, ...event };
  if (type === 'touchend' || type === 'touchcancel') {
    if (Array.isArray(e.touches)) e.touches = (e.touches as unknown[]).slice(1);
  }
  (globalThis.document as unknown as { dispatchEvent(x: unknown): void }).dispatchEvent(e);
}

function fireOnDetached(el: FakeEl, type: string, event: Record<string, unknown>) {
  const e: Record<string, unknown> = { type, ...event };
  if (Array.isArray(e.touches)) e.touches = (e.touches as unknown[]).slice(1);
  el.dispatch(e as { type: string });
}

interface Line {
  face: string;
  verdict: string;
  rowRect?: Box;
  faceRect?: Box;
  scheduled?: boolean;
  under?: string;
  underFace?: string | null;
  dispatchedTo?: string | null;
  faceCount?: number;
  watchableCount?: number;
  point?: { x: number; y: number };
  missedBy?: { face: string; px: number; dx?: number; dy?: number; rect?: Box } | null;
  quiet?: {
    ms: number; checks: number; covered: number; nudges: number; closes: number;
  } | null;
  sinceKeyboardMs?: number;
  sinceInputMs?: number | null;
  closePath?: string | null;
  morph?: string;
  viewport?: { appHeight: string; keyboardActive: boolean };
  movedPx?: number;
  fingers?: number;
  fingersAtLift?: number;
  heldMs?: number;
  reachPx?: number | null;
  standDown?: string;
  claimedBy?: { target: string | null; onControl: boolean; sinceTouchMs: number | null };
  nudgesSinceKeystroke?: number;
  cover?: string;
  nudged?: boolean;
  nudgeTrigger?: string;
  connected?: boolean;
  screenOff?: { x: number; y: number };
}

function lines(): Line[] {
  return postClientLog.mock.calls
    .filter((c) => c[0] === 'composer-press')
    .map((c) => c[2] as Line);
}

function verdicts(): string[] {
  return lines().map((l) => l.verdict);
}

beforeAll(() => {
  vi.useFakeTimers();
  const g = globalThis as unknown as Record<string, unknown>;
  g.innerWidth = 390;
  g.innerHeight = 844;
  g.scrollY = 0;
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
  textarea = new FakeEl(null, { left: 0, right: 390, top: 340, bottom: 396 }, ['prompt-textarea']);
  atPoint = send;
  atPointNear = null;
  showToast.mockClear();
  postClientLog.mockClear();
});

afterEach(() => {
  vi.advanceTimersByTime(5000);
  _resetNudgeStateForTesting();
});

function tapSend() {
  fire('touchstart', touch(send, 350, 420));
  fire('touchend', touch(send, 350, 420));
}

describe('the press ledger keeps every press it watched', () => {
  it('writes a line for each of two taps 100ms apart', () => {
    tapSend();
    vi.advanceTimersByTime(100);
    tapSend();
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toEqual(['dead', 'dead']);
  });

  it('keeps each press with the claim it earned, not a neighbour’s', () => {
    fire('touchstart', touch(send, 350, 420));
    fire('touchend', touch(send, 350, 420));
    vi.advanceTimersByTime(0);
    vi.advanceTimersByTime(100);
    fire('touchstart', touch(send, 350, 420));
    fire('touchend', touch(send, 350, 420));
    notePressOutcome('served');
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toEqual(['dead', 'served']);
  });

  it('logs a still press held past a tap as a long hold, and does not toast', () => {
    const heldMs = 1500;
    fire('touchstart', touch(send, 350, 420));
    vi.advanceTimersByTime(heldMs);
    fire('touchend', touch(send, 350, 420));
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toEqual(['long-hold']);
    expect(lines()[0].heldMs).toBe(heldMs);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('logs a dead tap on a thread action, and does not toast', () => {
    const apply = new FakeEl('Apply', { left: 260, right: 322, top: 400, bottom: 444 }, ['action-btn'], row);
    apply.threadAction = true;
    faces = [apply, send];
    fire('touchstart', touch(apply, 290, 420));
    fire('touchend', touch(apply, 290, 420));
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toEqual(['dead']);
    expect(lines()[0].face).toBe('Apply');
    expect(showToast).not.toHaveBeenCalled();
  });

  it('logs a cancelled tap on a thread action, and does not toast', () => {
    const apply = new FakeEl('Apply', { left: 260, right: 322, top: 400, bottom: 444 }, ['action-btn'], row);
    apply.threadAction = true;
    faces = [apply, send];
    fire('touchstart', touch(apply, 290, 420));
    fire('touchcancel', touch(apply, 290, 420));
    vi.advanceTimersByTime(0);
    expect(verdicts()).toEqual(['canceled']);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('logs a cancelled tap the touch path served as served, and does not toast', () => {
    fire('touchstart', touch(send, 350, 420));
    fire('touchcancel', touch(send, 350, 420));
    notePressOutcome('served');
    vi.advanceTimersByTime(0);
    expect(verdicts()).toEqual(['served']);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('still toasts a dead tap on Cancel, which is a composer control', () => {
    send.label = 'Cancel';
    tapSend();
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toEqual(['dead']);
    expect(showToast).toHaveBeenCalledWith(
      expect.stringMatching(/^Cancel did not register/),
      'warning',
    );
  });

  it('still toasts a cancelled tap on Send', () => {
    fire('touchstart', touch(send, 350, 420));
    fire('touchcancel', touch(send, 350, 420));
    vi.advanceTimersByTime(0);
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('the system cancelled the touch'),
      'warning',
    );
  });

  it('still toasts a tap on Send the browser sent to the row', () => {
    atPoint = row;
    fire('touchstart', touch(row, 350, 420));
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('the tap was on the button'),
      'warning',
    );
  });

  it('still toasts a quick tap that nothing took', () => {
    tapSend();
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toEqual(['dead']);
    expect(lines()[0].heldMs).toBe(0);
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('reached the button and nothing ran'),
      'warning',
    );
  });

  it('reports an armed press whose lift never arrived', () => {
    fire('touchstart', touch(send, 350, 420));
    fire('touchstart', touch(row, 10, 420));
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toContain('no-lift');
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('the lift never arrived'),
      'warning',
    );
  });

  it('hears the lift on a face the page removed mid-press', () => {
    fire('touchstart', touch(send, 350, 420));
    send.isConnected = false;
    fireOnDetached(send, 'touchend', touch(send, 350, 420));
    notePressOutcome('served');
    vi.advanceTimersByTime(LIFT_DEADLINE_MS);
    expect(verdicts()).toEqual(['served']);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('settles a lift both the document and the node heard once', () => {
    fire('touchstart', touch(send, 350, 420));
    fire('touchend', touch(send, 350, 420));
    send.dispatch({ type: 'touchend', ...touch(send, 350, 420) });
    notePressOutcome('served');
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toEqual(['served']);
  });

  it('does not call a second finger a lost lift', () => {
    fire('touchstart', touch(send, 350, 420));
    fire('touchstart', touch(row, 10, 420, 2, 1));
    vi.advanceTimersByTime(1000);
    expect(verdicts()).not.toContain('no-lift');
  });

  it('still rules the first press when a second finger joined and then lifted', () => {
    fire('touchstart', touch(send, 350, 420));
    fire('touchstart', touch(row, 10, 420, 2, 1));
    fire('touchend', touch(send, 350, 420));
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toEqual(['multi-touch']);
    expect(lines()[0].fingers).toBe(2);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('ignores a second finger lifting first, which is not this press ending', () => {
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
    expect(verdicts()).toEqual(['multi-touch']);
    expect(lines()[0].movedPx).toBe(0);
  });

  it('reports a press that is never followed by anything at all', () => {
    fire('touchstart', touch(send, 350, 420));
    vi.advanceTimersByTime(1000);
    expect(verdicts()).toEqual([]);
    vi.advanceTimersByTime(LIFT_DEADLINE_MS);
    expect(verdicts()).toEqual(['no-lift']);
  });

  it('does not toast at the deadline, when the finger may still be down', () => {
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
  function noRecentTouch() {
    vi.advanceTimersByTime(2000);
    postClientLog.mockClear();
    showToast.mockClear();
  }

  function root(): Record<string, unknown> {
    return (globalThis.document as unknown as { documentElement: Record<string, unknown> })
      .documentElement;
  }

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
    noRecentTouch();
    keyboardUp();
    fire('click', { target: elsewhere, clientX: 40, clientY: 120, screenX: 40, screenY: 179 });
    expect(verdicts()).toEqual(['stray-click']);
    expect(lines()[0].face).toContain('thread-content');
    expect(lines()[0].point).toEqual({ x: 40, y: 120 });
    expect(lines()[0].screenOff).toEqual({ x: 0, y: 59 });
  });

  it('says nothing while the keyboard is down, which no report describes', () => {
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

function settleHealthy() {
  atPoint = send;
  vi.advanceTimersByTime(SCHEDULED_CHECK_MS);
  vi.advanceTimersByTime(200);
  postClientLog.mockClear();
  showToast.mockClear();
}

describe('the reading that does not wait to be touched', () => {

  it('stays quiet while no composer row is laid out', () => {
    settleHealthy();
    row.box = { left: 0, right: 0, top: 0, bottom: 0 };
    atPoint = null;
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS);
    expect(verdicts()).toEqual([]);
  });

  it('stays quiet while the document is hidden', () => {
    settleHealthy();
    const doc = globalThis.document as unknown as Record<string, unknown>;
    doc.visibilityState = 'hidden';
    atPoint = row;
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS);
    doc.visibilityState = 'visible';
    expect(verdicts()).toEqual([]);
  });

});

describe('a cover the app raised itself is not a wedge', () => {

  function root(): Record<string, unknown> {
    return (globalThis.document as unknown as { documentElement: Record<string, unknown> })
      .documentElement;
  }

  function blockUi(on: boolean) {
    root().hasAttribute = (name: string) => on && name === 'data-ui-blocked';
  }

  let priorHasAttribute: unknown;
  beforeEach(() => { priorHasAttribute = root().hasAttribute; });
  afterEach(() => { root().hasAttribute = priorHasAttribute; });

  it('stays quiet when the scheduled check runs under the blocker', () => {
    settleHealthy();
    blockUi(true);
    atPoint = row;
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS);
    expect(verdicts()).toEqual([]);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('does not JUDGE a tap that landed on the blocker, but does record it', () => {
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

  it('judges the cover as it stood at the pointerdown, not at the touchstart', () => {
    settleHealthy();
    const shell = new FakeEl(null, { left: 0, right: 390, top: 0, bottom: 844 }, ['app-shell']);
    root().hasAttribute = (name: string) => name === 'data-overlay-open';
    fire('pointerdown', { target: shell, isPrimary: true });
    root().hasAttribute = priorHasAttribute;
    atPoint = send;
    fire('touchstart', touch(shell, 350, 420));
    expect(verdicts()).toEqual(['covered']);
    expect(lines()[0].cover).toBe('data-overlay-open');
    expect(showToast).not.toHaveBeenCalled();
  });

  it('spends that reading on one touch, so the next does not inherit it', () => {
    settleHealthy();
    root().hasAttribute = (name: string) => name === 'data-overlay-open';
    fire('pointerdown', { target: row, isPrimary: true });
    root().hasAttribute = priorHasAttribute;
    atPoint = send;
    fire('touchstart', touch(elsewhere, 350, 420));
    fire('touchend', touch(elsewhere, 350, 420));
    fire('touchstart', touch(elsewhere, 350, 420));
    expect(verdicts()).toEqual(['covered', 'missed']);
    expect(showToast).toHaveBeenCalledTimes(1);
  });

  it('stays silent for a tap that reached neither the row nor a face', () => {
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
    settleHealthy();
    tapSend();
    blockUi(true);
    vi.advanceTimersByTime(1000);
    postClientLog.mockClear();
    vi.advanceTimersByTime(30000);
    root().hasAttribute = priorHasAttribute as () => boolean;
    tapSend();
    vi.advanceTimersByTime(1000);
    const press = lines().find((l) => l.verdict === 'dead' || l.verdict === 'clicked');
    expect(press?.quiet?.checks).toBeGreaterThan(0);
    expect(press?.quiet?.covered).toBe(press?.quiet?.checks);
  });

});

describe('a dead composer tap runs the recovery the user runs by hand', () => {
  let props: Record<string, string>;
  let writes: string[];
  let priorStyle: unknown;

  beforeEach(() => {
    const el = (globalThis.document as unknown as { documentElement: Record<string, unknown> })
      .documentElement;
    priorStyle = el.style;
    writes = [];
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

  function wedged() {
    atPointNear = (x: number, y: number) => (
      x >= SEND_BOX.left && x <= SEND_BOX.right && y >= SEND_BOX.top && y <= SEND_BOX.bottom
        ? send
        : row
    );
  }

  function tapDeadSpace() {
    wedged();
    fire('touchstart', touch(row, 10, 420));
    fire('touchend', touch(row, 10, 420));
  }

  it('relayouts the shell by the span the keyboard takes, then puts it back', () => {
    settleHealthy();
    tapDeadSpace();
    vi.advanceTimersByTime(700);
    expect(writes).toEqual(['108px', '476px']);
    expect(props['--app-height']).toBe('476px');
  });

  it('relayouts for a dead tap whose node the page removed mid-press', () => {
    settleHealthy();
    wedged();
    fire('touchstart', touch(row, 10, 420));
    row.isConnected = false;
    fireOnDetached(row, 'touchend', touch(row, 10, 420));
    vi.advanceTimersByTime(700);
    expect(writes).toEqual(['108px', '476px']);
  });

  it('spends it on the FIRST dead tap, not the tenth', () => {
    settleHealthy();
    tapDeadSpace();
    vi.advanceTimersByTime(700);
    expect(writes.length).toBeGreaterThan(0);
  });

  it('says nothing, because the missed line already said the press arrived', () => {
    settleHealthy();
    tapDeadSpace();
    vi.advanceTimersByTime(700);
    expect(verdicts().filter((v) => v !== 'missed')).toEqual([]);
  });

  it('never toasts, because nothing here can score the repair', () => {
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

  function tapSendDispatchedTo(target: FakeEl) {
    atPoint = send;
    fire('touchstart', touch(target, 350, 420));
    fire('touchend', touch(target, 350, 420));
  }

  it('relays out for a tap on live Send that the browser sent elsewhere', () => {
    settleHealthy();
    tapSendDispatchedTo(elsewhere);
    vi.advanceTimersByTime(700);
    expect(lines().find((l) => l.verdict === 'missed')?.under).toBe('live-face');
    expect(writes).toEqual(['108px', '476px']);
  });

  it('logs and toasts where the touch went, apart from what answers there', () => {
    settleHealthy();
    tapSendDispatchedTo(elsewhere);
    expect(lines().find((l) => l.verdict === 'missed')?.dispatchedTo).toBe('button.thread-content');
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('sent it to button.thread-content, while button.action-btn answers'),
      'warning',
    );
  });

  it('says so when the touch went to a node the page had already removed', () => {
    settleHealthy();
    elsewhere.isConnected = false;
    tapSendDispatchedTo(elsewhere);
    expect(lines().find((l) => l.verdict === 'missed')?.dispatchedTo)
      .toBe('button.thread-content (detached)');
  });

  it('leaves a press the app drops on purpose alone', () => {
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

  it('says how far it fell from the face it could have pressed', () => {
    settleHealthy();
    root().hasAttribute = (name: string) => name === 'data-keyboard-active';
    atPoint = elsewhere;
    fire('touchstart', touch(elsewhere, 40, 120));
    const stray = lines().find((l) => l.verdict === 'keyboard-touch');
    expect(stray?.missedBy?.face).toBe('Send message');
    expect(stray?.missedBy?.dx).toBe(-290);
    expect(stray?.missedBy?.dy).toBe(-280);
  });

  it('carries a null vector when the row holds no face to reach', () => {
    settleHealthy();
    root().hasAttribute = (name: string) => name === 'data-keyboard-active';
    atPoint = elsewhere;
    faces = [];
    fire('touchstart', touch(elsewhere, 40, 120));
    const stray = lines().find((l) => l.verdict === 'keyboard-touch');
    expect(stray?.verdict).toBe('keyboard-touch');
    expect(stray?.missedBy ?? null).toBeNull();
  });
});

describe('every press line carries the one reading layout cannot fake', () => {
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
  it('carries the gap, and how many checks ran across it', () => {
    settleHealthy();
    tapSend();
    vi.advanceTimersByTime(1000);
    postClientLog.mockClear();
    vi.advanceTimersByTime(30000);
    tapSend();
    vi.advanceTimersByTime(1000);
    const press = lines().find((l) => l.verdict === 'dead' || l.verdict === 'clicked');
    expect(press?.quiet?.ms).toBeGreaterThanOrEqual(30000);
    expect(press?.quiet?.checks).toBeGreaterThan(0);
  });

  it('does not let a synthetic click erase the silence its own tap ended', () => {
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

});

describe('a missed press says why no face took it', () => {
  it('names an excluded action face under the finger', () => {
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

  it('says nothing was under a tap on empty row space', () => {
    settleHealthy();
    atPoint = row;
    fire('touchstart', touch(row, 10, 420));
    const missed = lines().find((l) => l.verdict === 'missed');
    expect(missed?.under).toBe('nothing');
    expect(missed?.underFace).toBeNull();
  });

  it('measures how far outside the nearest face the finger fell', () => {
    settleHealthy();
    atPoint = row;
    fire('touchstart', touch(row, 382, 420));
    const missed = lines().find((l) => l.verdict === 'missed');
    expect(missed?.point).toEqual({ x: 382, y: 420 });
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
    props['--app-height'] = '852px';
    vi.advanceTimersByTime(1000);
    expect(lines()[0].viewport?.appHeight).toBe('476px');
  });

  it('carries the morph the press saw, not the one the ruling saw', () => {
    settleHealthy();
    fire('touchstart', touch(send, 350, 420));
    fire('touchend', touch(send, 350, 420));
    send.label = 'Cancel';
    vi.advanceTimersByTime(1000);
    expect(lines()[0].morph).toBe('send');
  });

  it('quotes the press’s viewport in the TOAST too, not the ruling’s', () => {
    settleHealthy();
    fire('touchstart', touch(send, 350, 420));
    fire('touchend', touch(send, 350, 420));
    props['--app-height'] = '852px';
    vi.advanceTimersByTime(1000);
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('app 476px'), 'warning');
  });

  it('carries the silence the press ended, not a later one', () => {
    settleHealthy();
    fire('touchstart', touch(elsewhere, 10, 90));
    vi.advanceTimersByTime(5000);
    fire('touchstart', touch(send, 350, 420));
    fire('touchend', touch(send, 350, 420));
    vi.advanceTimersByTime(1000);
    expect(lines()[0].quiet?.ms ?? 0).toBeGreaterThanOrEqual(5000);
  });
});

describe('the recovery a composer nobody can touch still gets', () => {

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
    atPoint = send;
    fire('touchstart', touch(textarea, 10, 350));
  });

  function settleTouched() {
    atPoint = send;
    fire('touchstart', touch(textarea, 10, 350));
    vi.advanceTimersByTime(1);
    postClientLog.mockClear();
    showToast.mockClear();
  }

  function type() {
    fire('input', { target: { dataset: { role: 'prompt-input' } } });
  }

  function untouched(): Line[] {
    return lines().filter((l) => l.verdict === 'untouched');
  }

  it('relayouts for a typed draft the page has taken no touch since', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 2);
    expect(untouched()).not.toHaveLength(0);
    expect(untouched()[0].face).toBe('Send message');
    expect(untouched()[0].nudged).toBe(true);
    expect(untouched()[0].scheduled).toBe(true);
  });

  it('restores the height it nudged', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 2);
    expect(props['--app-height']).toBe('476px');
  });

  it('never commits, because a silence is not a press', () => {
    settleTouched();
    const before = send.clicks;
    type();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 6);
    expect(send.clicks).toBe(before);
  });

  it('stays out of a healthy send, where the tap follows the typing', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(2000);
    expect(untouched()).toHaveLength(0);
  });

  it('reaches the pause at its bound, not a tick later', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(UNTOUCHED_QUIET_MS - 1);
    expect(untouched()).toHaveLength(0);
    vi.advanceTimersByTime(1);
    expect(untouched()).toHaveLength(1);
  });

  it('names the tick when the tick is what spent it', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 3);
    const byTick = untouched().filter((l) => l.nudgeTrigger === 'tick');
    expect(byTick).not.toHaveLength(0);
  });

  it('stays quiet on a composer nobody has typed into', () => {
    settleTouched();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 4);
    expect(untouched()).toHaveLength(0);
  });

  it('stands down once a touch does reach the composer', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(1000);
    settleTouched();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 4);
    expect(untouched()).toHaveLength(0);
  });

  it('keeps going when the touch reached the page and not the composer', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(1000);
    fire('touchstart', touch(elsewhere, 10, 90));
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 4);
    expect(untouched()).not.toHaveLength(0);
  });

  it('stands down for a CLICK on the composer, which a touch may not carry', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(1000);
    fire('click', { target: textarea });
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 4);
    expect(untouched()).toHaveLength(0);
  });

  it('has nothing to protect while a turn is running', () => {
    settleTouched();
    send.label = 'Cancel';
    type();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 4);
    expect(untouched()).toHaveLength(0);
  });

  it('stays quiet under a cover the app raised itself', () => {
    settleTouched();
    root().hasAttribute = (name: string) => name === 'data-overlay-open';
    type();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 4);
    expect(untouched()).toHaveLength(0);
  });

  it('keeps trying right through the stretch the user spends tapping', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(20_000);
    expect(untouched().length).toBeGreaterThan(4);
  });

  it('stops once the user has plainly put the phone down', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 30);
    const spent = untouched().length;
    expect(spent).toBeGreaterThan(0);
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 30);
    expect(untouched()).toHaveLength(spent);
  });

  it('counts them onto the press that ends the silence', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 2);
    postClientLog.mockClear();
    tapSend();
    vi.advanceTimersByTime(1000);
    const press = lines().find((l) => l.verdict === 'dead' || l.verdict === 'clicked');
    expect(press?.quiet?.nudges ?? 0).toBeGreaterThan(0);
  });

  it('starts the next silence at no nudges', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 2);
    settleTouched();
    vi.advanceTimersByTime(1000);
    tapSend();
    vi.advanceTimersByTime(1000);
    const press = lines().find((l) => l.verdict === 'dead' || l.verdict === 'clicked');
    expect(press?.quiet?.nudges).toBe(0);
  });

  it('carries a second count a touch does not reset, so the send is scored', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 2);
    fire('touchstart', touch(elsewhere, 10, 90));
    vi.advanceTimersByTime(1000);
    postClientLog.mockClear();
    tapSend();
    vi.advanceTimersByTime(1000);
    const press = lines().find((l) => l.verdict === 'dead' || l.verdict === 'clicked');
    expect(press?.quiet?.nudges ?? 0).toBeLessThan(press?.nudgesSinceKeystroke ?? 0);
    expect(press?.nudgesSinceKeystroke ?? 0).toBeGreaterThan(0);
  });

  it('starts that count over at the next keystroke', () => {
    settleTouched();
    type();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 2);
    type();
    postClientLog.mockClear();
    tapSend();
    vi.advanceTimersByTime(1000);
    const press = lines().find((l) => l.verdict === 'dead' || l.verdict === 'clicked');
    expect(press?.nudgesSinceKeystroke).toBe(0);
  });

  it('leaves the quiet window measuring touches, never keystrokes', () => {
    const gapMs = 4000;
    settleTouched();
    vi.advanceTimersByTime(gapMs);
    type();
    vi.advanceTimersByTime(gapMs);
    tapSend();
    vi.advanceTimersByTime(1000);
    const press = lines().find((l) => l.verdict === 'dead' || l.verdict === 'clicked');
    expect(press?.quiet?.ms ?? 0).toBeGreaterThanOrEqual(gapMs * 2);
  });
});

describe('the silence that follows a keyboard close', () => {

  const FULL = 852;
  const UP = { height: 476, layoutViewport: FULL };
  const DOWN = { height: FULL, layoutViewport: FULL };

  let props: Record<string, string>;
  let writes: string[];
  let priorStyle: unknown;

  function root(): Record<string, unknown> {
    return (globalThis.document as unknown as { documentElement: Record<string, unknown> })
      .documentElement;
  }

  beforeEach(() => {
    resetKeyboardCloseState();
    priorStyle = root().style;
    writes = [];
    props = { '--app-height': `${FULL}px` };
    root().style = {
      setProperty: (k: string, v: string) => { props[k] = v; if (k === '--app-height') writes.push(v); },
      getPropertyValue: (k: string) => props[k] ?? '',
      removeProperty: (k: string) => { delete props[k]; },
    };
  });

  afterEach(() => {
    root().style = priorStyle;
    resetKeyboardCloseState();
    fire('touchstart', touch(textarea, 10, 350));
    vi.advanceTimersByTime(1000);
  });

  function closeKeyboard() {
    noteViewportResize(UP, Date.now());
    noteViewportResize(DOWN, Date.now());
  }

  function silences(): Line[] {
    return lines().filter((l) => l.verdict === 'silent-since-keyboard');
  }

  it('writes the line the wedge has never had', () => {
    closeKeyboard();
    postClientLog.mockClear();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 2);
    expect(silences()).toHaveLength(1);
    expect(silences()[0].face).toBe('Send message');
    expect(silences()[0].scheduled).toBe(true);
  });

  it('says the close relaid the shell out, so the recovery is scored', () => {
    closeKeyboard();
    expect(writes).toEqual([`${FULL - 1}px`, `${FULL}px`]);
    postClientLog.mockClear();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 2);
    expect(silences()[0].nudged).toBe(true);
  });

  it('carries both anchors, so an unnamed input in the gap is visible', () => {
    closeKeyboard();
    vi.advanceTimersByTime(1000);
    fire('touchstart', touch(elsewhere, 10, 90));
    postClientLog.mockClear();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 2);
    const line = silences()[0];
    expect(line.sinceInputMs ?? 0).toBeLessThan(line.sinceKeyboardMs ?? 0);
  });

  it('measures the silence from the LAST input, not from the close', () => {
    closeKeyboard();
    for (let i = 0; i < 8; i++) {
      fire('touchstart', touch(elsewhere, 10, 90));
      vi.advanceTimersByTime(1000);
    }
    expect(silences()).toHaveLength(0);
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 2);
    expect(silences()).toHaveLength(1);
  });

  it('stays quiet for a user who closes the keyboard and taps straight away', () => {
    closeKeyboard();
    postClientLog.mockClear();
    vi.advanceTimersByTime(500);
    fire('touchstart', touch(textarea, 10, 350));
    vi.advanceTimersByTime(500);
    expect(silences()).toEqual([]);
  });

  it('stays quiet with nothing to send, which is most of an idle phone', () => {
    send.label = 'Cancel';
    closeKeyboard();
    postClientLog.mockClear();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 4);
    expect(silences()).toEqual([]);
  });

  it('stays quiet when no keyboard has closed at all', () => {
    postClientLog.mockClear();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 4);
    expect(silences()).toEqual([]);
  });

  it('names the path that saw the close', () => {
    closeKeyboard();
    postClientLog.mockClear();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 2);
    expect(silences()[0].closePath).toBe('resize');
  });

  it('stays quiet once the keys come back, rather than citing a stale close', () => {
    closeKeyboard();
    noteViewportResize(UP, Date.now());
    postClientLog.mockClear();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 4);
    expect(silences()).toEqual([]);
  });

  it('writes one line per close, not one per tick of the same silence', () => {
    closeKeyboard();
    postClientLog.mockClear();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 10);
    expect(silences()).toHaveLength(1);
  });

  it('writes again for the NEXT close, since a wedge can return', () => {
    closeKeyboard();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 2);
    closeKeyboard();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 2);
    expect(silences()).toHaveLength(2);
  });

  it('stays quiet under a cover the app raised itself', () => {
    const priorHasAttribute = root().hasAttribute;
    closeKeyboard();
    postClientLog.mockClear();
    root().hasAttribute = (name: string) => name === 'data-ui-blocked';
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 4);
    root().hasAttribute = priorHasAttribute;
    expect(silences()).toEqual([]);
  });

  it('never toasts, because the user cannot act on it', () => {
    closeKeyboard();
    showToast.mockClear();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 2);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('presses nothing, on the close or on the line', () => {
    closeKeyboard();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS * 2);
    expect(send.clicks).toBe(0);
  });

  it('counts the close onto the press that ends the silence', () => {
    tapSend();
    vi.advanceTimersByTime(1000);
    closeKeyboard();
    postClientLog.mockClear();
    vi.advanceTimersByTime(SCHEDULED_CHECK_MS);
    tapSend();
    vi.advanceTimersByTime(1000);
    const press = lines().find((l) => l.verdict === 'dead' || l.verdict === 'clicked');
    expect(press?.quiet?.closes).toBe(1);
    expect(press?.nudgesSinceKeystroke).toBe(0);
  });

  it('leaves the quiet window alone, since a close is not an input', () => {
    tapSend();
    vi.advanceTimersByTime(1000);
    closeKeyboard();
    vi.advanceTimersByTime(20000);
    tapSend();
    vi.advanceTimersByTime(1000);
    const press = lines().reverse().find((l) => l.verdict === 'dead' || l.verdict === 'clicked');
    expect(press?.quiet?.ms ?? 0).toBeGreaterThanOrEqual(20000);
  });
});
