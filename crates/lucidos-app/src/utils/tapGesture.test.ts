import { describe, it, expect, vi } from 'vitest';
import {
  createTapGate, touchActivated, takePressOutcome, notePressOutcome,
  CANCELLED_TAP_SETTLE_MS, FACE_SEEN_MS, pressPredatesFace, type ScrollNode,
} from './tapGesture';
import { LONG_PRESS_DELAY_MS } from '../hooks/useLongPress';
import { TAP_MOVE_THRESHOLD_PX } from '@lucidos/tap-press';

interface Probe {
  screenX: number;
  screenY: number;
  clientX: number;
  clientY: number;
}

function at(x: number, y: number): Probe {
  return { screenX: x, screenY: y, clientX: x, clientY: y };
}

describe('createTapGate', () => {
  it('treats a click without a recorded press as a tap (keyboard activation)', () => {
    const gate = createTapGate();
    expect(gate.isTap()).toBe(true);
  });

  it('treats a press with no movement as a tap', () => {
    const gate = createTapGate();
    gate.down(at(100, 200));
    expect(gate.isTap()).toBe(true);
  });

  it('treats a press with sub-threshold wobble as a tap', () => {
    const gate = createTapGate();
    gate.down(at(100, 200));
    gate.move(at(105, 203));
    expect(gate.isTap()).toBe(true);
  });

  it('cancels the tap when movement exceeds the threshold (vertical scroll)', () => {
    const gate = createTapGate();
    gate.down(at(100, 200));
    gate.move(at(101, 230));
    expect(gate.isTap()).toBe(false);
  });

  it('cancels the tap when movement exceeds the threshold (horizontal swipe)', () => {
    const gate = createTapGate();
    gate.down(at(100, 200));
    gate.move(at(150, 200));
    expect(gate.isTap()).toBe(false);
  });

  it('stays canceled even if the finger drifts back to the start', () => {
    const gate = createTapGate();
    gate.down(at(100, 200));
    gate.move(at(100, 250));
    gate.move(at(100, 200));
    expect(gate.isTap()).toBe(false);
  });

  it('resets between presses so a canceled gesture does not poison the next', () => {
    const gate = createTapGate();
    gate.down(at(0, 0));
    gate.move(at(0, 100));
    expect(gate.isTap()).toBe(false);

    gate.down(at(50, 50));
    expect(gate.isTap()).toBe(true);
  });

  it('cancel() clears the in-flight press so a follow-up click is not auto-suppressed', () => {
    const gate = createTapGate();
    gate.down(at(0, 0));
    gate.move(at(0, 100));
    gate.cancel();
    expect(gate.isTap()).toBe(true);
  });
});

describe('createTapGate measures the finger, not the viewport', () => {
  it('holds a stationary finger to be a tap while the viewport scrolls under it', () => {
    const press: Probe = { screenX: 100, screenY: 700, clientX: 100, clientY: 300 };
    const settled: Probe = { screenX: 100, screenY: 700, clientX: 100, clientY: 340 };
    const gate = createTapGate();
    gate.down(press);
    gate.move(settled);
    expect(gate.isTap()).toBe(true);
  });

  it('cancels a finger that crossed the screen while the content travelled with it', () => {
    const press: Probe = { screenX: 100, screenY: 700, clientX: 100, clientY: 300 };
    const dragged: Probe = { screenX: 100, screenY: 620, clientX: 100, clientY: 302 };
    const gate = createTapGate();
    gate.down(press);
    gate.move(dragged);
    expect(gate.isTap()).toBe(false);
  });
});

describe('createTapGate reports the press it discarded', () => {
  it('gives the screen-space distance of the press it rejects', () => {
    const gate = createTapGate();
    gate.down(at(100, 200));
    gate.move(at(100, 232));
    expect(gate.tapRejection()).toBe(32);
  });

  it('reports the furthest point reached, not the last one', () => {
    const gate = createTapGate();
    gate.down(at(100, 200));
    gate.move(at(100, 250));
    gate.move(at(100, 205));
    expect(gate.tapRejection()).toBe(50);
  });

  it('keeps measuring past the threshold that already doomed the press', () => {
    const gate = createTapGate();
    gate.down(at(100, 209));
    gate.move(at(100, 218));
    gate.move(at(100, 259));
    expect(gate.tapRejection()).toBe(50);
  });

  it('still rejects a press that wandered back inside the threshold', () => {
    const gate = createTapGate();
    gate.down(at(100, 200));
    gate.move(at(100, 250));
    gate.move(at(100, 201));
    expect(gate.tapRejection()).toBe(50);
  });

  it('returns null for a press it lets through', () => {
    const gate = createTapGate();
    gate.down(at(100, 200));
    gate.move(at(100, 205));
    expect(gate.tapRejection()).toBeNull();
  });

  it('consumes the press, so the next click is judged fresh', () => {
    const gate = createTapGate();
    gate.down(at(100, 200));
    gate.move(at(100, 240));
    expect(gate.tapRejection()).toBe(40);
    expect(gate.tapRejection()).toBeNull();
  });
});

function harness(opts: {
  enabled?: () => boolean;
  gate?: { pass(): boolean; spend(): void; aborted?(): boolean };
  destructive?: () => boolean;
  pageVisible?: () => boolean;
  shownAt?: () => number | null;
} = {}) {
  let t = 1_000;
  const action = vi.fn();
  const handlers = touchActivated(action, { ...opts, now: () => t });
  const touch = { preventDefault: vi.fn() };
  return { action, handlers, touch, advance: (ms: number) => { t += ms; } };
}

describe('touchActivated', () => {
  it('runs the action on touchend, inside the gesture', () => {
    const { action, handlers, touch } = harness();
    handlers.onTouchEnd(touch);
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('cancels the synthetic click the touch would produce', () => {
    const { handlers, touch } = harness();
    handlers.onTouchEnd(touch);
    expect(touch.preventDefault).toHaveBeenCalledTimes(1);
  });

  it('ignores a click that arrives as the twin of a touch it already served', () => {
    const { action, handlers, touch, advance } = harness();
    handlers.onTouchEnd(touch);
    advance(50);
    handlers.onClick();
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('runs the action on a click with no touch behind it', () => {
    const { action, handlers } = harness();
    handlers.onClick();
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('serves a click long after the last touch', () => {
    const { action, handlers, touch, advance } = harness();
    handlers.onTouchEnd(touch);
    advance(5_000);
    handlers.onClick();
    expect(action).toHaveBeenCalledTimes(2);
  });

  it('serves a second real tap inside the twin window', () => {
    const { action, handlers, touch, advance } = harness();
    handlers.onTouchEnd(touch);
    advance(120);
    handlers.onTouchEnd(touch);
    expect(action).toHaveBeenCalledTimes(2);
  });

  it('eats one twin per touch, never a second click inside the window', () => {
    const { action, handlers, touch, advance } = harness();
    handlers.onTouchEnd(touch);
    advance(50);
    handlers.onClick();
    advance(50);
    handlers.onClick();
    expect(action).toHaveBeenCalledTimes(2);
  });
});

describe('touchActivated takes every press it is given', () => {
  it('runs the action however far the finger travelled', () => {
    const { action, handlers, touch } = harness();
    handlers.onTouchEnd(touch);
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('runs the action when the event carries no coordinates at all', () => {
    const action = vi.fn();
    const handlers = touchActivated(action);
    handlers.onTouchEnd({ preventDefault: vi.fn() });
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('stands the touch path down while disabled, leaving the click path whole', () => {
    const { action, handlers, touch } = harness({ enabled: () => false });
    handlers.onTouchEnd(touch);
    expect(action).not.toHaveBeenCalled();
    expect(touch.preventDefault).not.toHaveBeenCalled();
    handlers.onClick();
    expect(action).toHaveBeenCalledTimes(1);
  });
});

function contact(screenX: number, screenY: number, fingersDown: number) {
  const point = { identifier: 0, screenX, screenY };
  return { changedTouches: [point], touches: new Array(fingersDown).fill(point) };
}

describe('touchActivated serves a tap the system cancelled', () => {
  function cancelledTap(moveTo: number, heldMs: number, opts: Parameters<typeof harness>[0] = {}) {
    const h = harness(opts);
    h.handlers.onTouchStart(contact(100, 800, 1));
    h.handlers.onTouchMove(contact(100, 800 - moveTo, 1));
    h.advance(heldMs);
    h.handlers.onTouchCancel(contact(100, 800 - moveTo, 0));
    return h;
  }

  it('runs the action for a short press that barely moved', () => {
    const { action } = cancelledTap(6, 77);
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('claims the press, so the probe does not call it dead', () => {
    takePressOutcome(0);
    cancelledTap(6, 77);
    expect(takePressOutcome(1_000)).toBe('served');
  });

  it('leaves a cancelled scroll alone', () => {
    const { action } = cancelledTap(TAP_MOVE_THRESHOLD_PX + 1, 77);
    expect(action).not.toHaveBeenCalled();
  });

  it('leaves a cancelled hold alone', () => {
    const { action } = cancelledTap(0, LONG_PRESS_DELAY_MS + 1);
    expect(action).not.toHaveBeenCalled();
  });

  it('leaves a cancelled pinch alone', () => {
    const { action, handlers } = harness();
    handlers.onTouchStart(contact(100, 800, 2));
    handlers.onTouchCancel(contact(100, 800, 0));
    expect(action).not.toHaveBeenCalled();
  });

  it('fires a destructive face on a cancelled tap, once the settle window passes', () => {
    vi.useFakeTimers();
    try {
      const { action } = cancelledTap(5, 59, { destructive: () => true, pageVisible: () => true });
      expect(action).not.toHaveBeenCalled();
      vi.advanceTimersByTime(CANCELLED_TAP_SETTLE_MS);
      expect(action).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('drops a destructive cancelled tap when the page hides, as on a swipe home', () => {
    vi.useFakeTimers();
    try {
      let visible = true;
      const { action } = cancelledTap(5, 59, { destructive: () => true, pageVisible: () => visible });
      visible = false;
      vi.advanceTimersByTime(CANCELLED_TAP_SETTLE_MS);
      expect(action).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it('drops a destructive cancelled tap when the face turns back to Send', () => {
    vi.useFakeTimers();
    try {
      let cancelFace = true;
      const { action } = cancelledTap(5, 59, { destructive: () => cancelFace, pageVisible: () => true });
      cancelFace = false;
      vi.advanceTimersByTime(CANCELLED_TAP_SETTLE_MS);
      expect(action).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it('never fires a destructive face on a cancelled scroll', () => {
    const { action } = cancelledTap(TAP_MOVE_THRESHOLD_PX + 1, 50, { destructive: () => true });
    expect(action).not.toHaveBeenCalled();
  });

  it('stays down while the touch path is disabled', () => {
    const { action } = cancelledTap(0, 50, { enabled: () => false });
    expect(action).not.toHaveBeenCalled();
  });

  it('spends the gate, which the cancel left aborted', () => {
    const gate = { pass: vi.fn(() => true), spend: vi.fn(), aborted: () => true };
    const { action } = cancelledTap(0, 50, { gate });
    expect(gate.spend).toHaveBeenCalledTimes(1);
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('counts the travel the cancel event itself reports', () => {
    const { action, handlers } = harness();
    handlers.onTouchStart(contact(100, 800, 1));
    handlers.onTouchCancel(contact(100, 800 - TAP_MOVE_THRESHOLD_PX - 1, 0));
    expect(action).not.toHaveBeenCalled();
  });

  it('never runs a press an overlay dismiss swallowed', () => {
    takePressOutcome(0);
    const { action, handlers } = harness();
    handlers.onTouchStart(contact(100, 800, 1));
    notePressOutcome('swallowed', 1_000);
    handlers.onTouchCancel(contact(100, 800, 0));
    expect(action).not.toHaveBeenCalled();
    expect(takePressOutcome(Number.POSITIVE_INFINITY)).toBe('swallowed');
  });

  it('serves a click that follows a served cancel', () => {
    const { action, handlers } = cancelledTap(0, 50);
    handlers.onClick();
    expect(action).toHaveBeenCalledTimes(2);
  });

  it('serves a cancel only for a press it saw start', () => {
    const { action, handlers } = harness();
    handlers.onTouchCancel(contact(100, 800, 0));
    expect(action).not.toHaveBeenCalled();
  });

  it('forgets a press once its lift arrives', () => {
    const { action, handlers, touch } = harness();
    handlers.onTouchStart(contact(100, 800, 1));
    handlers.onTouchEnd(touch);
    handlers.onTouchCancel(contact(100, 800, 0));
    expect(action).toHaveBeenCalledTimes(1);
  });
});

function gated() {
  const gate = createTapGate();
  const reported = vi.fn();
  const activation = {
    pass: () => {
      const moved = gate.tapRejection();
      if (moved === null) return true;
      reported(moved);
      return false;
    },
    spend: () => gate.spend(),
    aborted: () => gate.wasAborted(),
  };
  return { gate, reported, activation };
}

describe('the tap gate guards the click path alone', () => {
  it('serves a moved press on touchend, and reports nothing', () => {
    const { gate, reported, activation } = gated();
    const { action, handlers, touch } = harness({ gate: activation });
    gate.down(at(100, 200));
    gate.move(at(100, 240));
    handlers.onTouchEnd(touch);
    expect(action).toHaveBeenCalledTimes(1);
    expect(reported).not.toHaveBeenCalled();
  });

  it('refuses the same moved press when it arrives as a click, and reports once', () => {
    const { gate, reported, activation } = gated();
    const { action, handlers } = harness({ gate: activation });
    gate.down(at(100, 200));
    gate.move(at(100, 240));
    handlers.onClick();
    expect(action).not.toHaveBeenCalled();
    expect(reported).toHaveBeenCalledTimes(1);
    expect(reported).toHaveBeenCalledWith(40);
  });

  it('spends the press the touch path served, so it cannot rule on the next one', () => {
    const { gate, reported, activation } = gated();
    const { action, handlers, touch, advance } = harness({ gate: activation });
    gate.down(at(100, 200));
    gate.move(at(100, 240));
    handlers.onTouchEnd(touch);
    advance(5_000);
    handlers.onClick();
    expect(action).toHaveBeenCalledTimes(2);
    expect(reported).not.toHaveBeenCalled();
  });

  it('does not let a twin click rule on the press the touch path spent', () => {
    const asked = vi.fn(() => true);
    const { handlers, touch, advance } = harness({ gate: { pass: asked, spend: () => {} } });
    handlers.onTouchEnd(touch);
    advance(50);
    handlers.onClick();
    expect(asked).not.toHaveBeenCalled();
  });

  it('activates on a click with no gate configured', () => {
    const { action, handlers } = harness();
    handlers.onClick();
    expect(action).toHaveBeenCalledTimes(1);
  });
});

describe('a destructive face asks the gate rather than spending it', () => {
  it('serves a stationary press inside the gesture, which is the whole fix', () => {
    const { gate, reported, activation } = gated();
    const { action, handlers, touch } = harness({ gate: activation, destructive: () => true });
    gate.down(at(100, 200));
    handlers.onTouchStart(contact(100, 200, 1));
    handlers.onTouchEnd(touch);
    expect(action).toHaveBeenCalledTimes(1);
    expect(reported).not.toHaveBeenCalled();
  });

  it('refuses a moved press, and says so once', () => {
    const { gate, reported, activation } = gated();
    const { action, handlers, touch } = harness({ gate: activation, destructive: () => true });
    gate.down(at(100, 200));
    handlers.onTouchStart(contact(100, 200, 1));
    gate.move(at(100, 240));
    handlers.onTouchEnd(touch);
    expect(action).not.toHaveBeenCalled();
    expect(reported).toHaveBeenCalledWith(40);
  });

  it('cancels the twin of a refused press, so no path serves it', () => {
    const { gate, reported, activation } = gated();
    const { action, handlers, touch, advance } = harness({ gate: activation, destructive: () => true });
    gate.down(at(100, 200));
    handlers.onTouchStart(contact(100, 200, 1));
    gate.move(at(100, 240));
    handlers.onTouchEnd(touch);
    expect(touch.preventDefault).toHaveBeenCalledTimes(1);
    advance(50);
    handlers.onClick();
    expect(action).not.toHaveBeenCalled();
    expect(reported).toHaveBeenCalledTimes(1);
  });

  it('records the outcome of a press it served, so the probe stops calling it dead', () => {
    takePressOutcome(0);
    const { gate, activation } = gated();
    const { handlers, touch } = harness({ gate: activation, destructive: () => true });
    gate.down(at(100, 200));
    handlers.onTouchStart(contact(100, 200, 1));
    handlers.onTouchEnd(touch);
    expect(takePressOutcome(5_000)).toBe('served');
  });

  it('records no outcome for a press it refused', () => {
    takePressOutcome(0);
    const { gate, activation } = gated();
    const { handlers, touch } = harness({ gate: activation, destructive: () => true });
    gate.down(at(100, 200));
    handlers.onTouchStart(contact(100, 200, 1));
    gate.move(at(100, 240));
    handlers.onTouchEnd(touch);
    expect(takePressOutcome(5_000)).toBeNull();
  });

  it('stands down while disabled, leaving the click path whole', () => {
    const { gate, activation } = gated();
    const { action, handlers, touch } = harness({
      gate: activation,
      destructive: () => true,
      enabled: () => false,
    });
    gate.down(at(100, 200));
    handlers.onTouchStart(contact(100, 200, 1));
    handlers.onTouchEnd(touch);
    expect(action).not.toHaveBeenCalled();
    expect(touch.preventDefault).not.toHaveBeenCalled();
  });

  it('leaves the constructive path spending the gate, never asking it', () => {
    const { gate, reported, activation } = gated();
    const { action, handlers, touch } = harness({ gate: activation, destructive: () => false });
    gate.down(at(100, 200));
    handlers.onTouchStart(contact(100, 200, 1));
    gate.move(at(100, 240));
    handlers.onTouchEnd(touch);
    expect(action).toHaveBeenCalledTimes(1);
    expect(reported).not.toHaveBeenCalled();
  });

  it('refuses a lift the system already took the gesture for', () => {
    const { gate, reported, activation } = gated();
    const { action, handlers, touch } = harness({ gate: activation, destructive: () => true });
    gate.down(at(100, 200));
    handlers.onTouchStart(contact(100, 200, 1));
    gate.move(at(100, 240));
    gate.cancel();
    handlers.onTouchEnd(touch);
    expect(action).not.toHaveBeenCalled();
    expect(reported).not.toHaveBeenCalled();
  });

  it('refuses even a stationary lift after the system took the gesture', () => {
    const { gate, activation } = gated();
    const { action, handlers, touch } = harness({ gate: activation, destructive: () => true });
    gate.down(at(100, 200));
    handlers.onTouchStart(contact(100, 200, 1));
    gate.cancel();
    handlers.onTouchEnd(touch);
    expect(action).not.toHaveBeenCalled();
  });

  it('claims a lift the system took as aborted, so the probe never calls it dead', () => {
    takePressOutcome(0);
    const { gate, activation } = gated();
    const { handlers, touch } = harness({ gate: activation, destructive: () => true });
    gate.down(at(100, 200));
    handlers.onTouchStart(contact(100, 200, 1));
    gate.cancel();
    handlers.onTouchEnd(touch);
    expect(takePressOutcome(5_000)).toBe('aborted');
  });

  it('lets the next press through, so one abort cannot wedge the button', () => {
    const { gate, activation } = gated();
    const { action, handlers, touch } = harness({ gate: activation, destructive: () => true });
    gate.down(at(100, 200));
    handlers.onTouchStart(contact(100, 200, 1));
    gate.cancel();
    handlers.onTouchEnd(touch);
    gate.down(at(100, 200));
    handlers.onTouchStart(contact(100, 200, 1));
    handlers.onTouchEnd(touch);
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('never raises the abort flag for a press the touch path served', () => {
    const { gate, activation } = gated();
    const { action, handlers, touch } = harness({ gate: activation, destructive: () => false });
    gate.down(at(100, 200));
    handlers.onTouchStart(contact(100, 200, 1));
    handlers.onTouchEnd(touch);
    expect(gate.wasAborted()).toBe(false);
    handlers.onTouchEnd(touch);
    expect(action).toHaveBeenCalledTimes(2);
  });

  it('serves a keyboard activation, which has no press and no abort', () => {
    const { activation } = gated();
    const { action, handlers } = harness({ gate: activation, destructive: () => true });
    handlers.onClick();
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('reads the flag per press, since one node serves Send and Cancel', () => {
    let destructive = false;
    const { gate, activation } = gated();
    const { action, handlers, touch } = harness({ gate: activation, destructive: () => destructive });
    gate.down(at(100, 200));
    handlers.onTouchStart(contact(100, 200, 1));
    gate.move(at(100, 240));
    handlers.onTouchEnd(touch);
    expect(action).toHaveBeenCalledTimes(1);
    destructive = true;
    gate.down(at(100, 200));
    handlers.onTouchStart(contact(100, 200, 1));
    gate.move(at(100, 240));
    handlers.onTouchEnd(touch);
    expect(action).toHaveBeenCalledTimes(1);
  });
});

describe('a press serves only the face it started on, once', () => {
  function morph() {
    const face = { destructive: false };
    const { action, handlers, touch, advance } = harness({ destructive: () => face.destructive });
    const lift = { ...contact(100, 200, 0), ...touch };
    return { face, action, handlers, lift, advance };
  }

  it('a lift that finds Send turned into Stop stops nothing', () => {
    const { face, action, handlers, lift, advance } = morph();
    handlers.onTouchStart(contact(100, 200, 1));
    face.destructive = true;
    advance(1_900);
    handlers.onTouchEnd(lift);
    expect(action).not.toHaveBeenCalled();
    expect(lift.preventDefault).toHaveBeenCalledTimes(1);
  });

  it('a press its click already served is not served again at its lift', () => {
    const { action, handlers, lift, advance } = morph();
    handlers.onTouchStart(contact(100, 200, 1));
    handlers.onClick();
    advance(1_400);
    handlers.onTouchEnd(lift);
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('a lift that finds Stop turned back into Send sends nothing', () => {
    const { face, action, handlers, lift } = morph();
    face.destructive = true;
    handlers.onTouchStart(contact(100, 200, 1));
    face.destructive = false;
    handlers.onTouchEnd(lift);
    expect(action).not.toHaveBeenCalled();
  });

  it('a Stop lift with no press behind it stops nothing', () => {
    const { face, action, handlers, lift } = morph();
    face.destructive = true;
    handlers.onTouchEnd(lift);
    expect(action).not.toHaveBeenCalled();
  });

  it('a cancelled press that began on Send stops nothing when the face is now Stop', () => {
    vi.useFakeTimers();
    try {
      const { face, action, handlers } = morph();
      handlers.onTouchStart(contact(100, 200, 1));
      face.destructive = true;
      handlers.onTouchCancel(contact(100, 200, 0));
      vi.advanceTimersByTime(CANCELLED_TAP_SETTLE_MS);
      expect(action).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it('a refused lift spends the gate and is claimed, so the probe never calls it dead', () => {
    takePressOutcome(0);
    const face = { destructive: false };
    const spend = vi.fn();
    const { action, handlers, touch } = harness({
      gate: { pass: () => true, spend },
      destructive: () => face.destructive,
    });
    handlers.onTouchStart(contact(100, 200, 1));
    face.destructive = true;
    handlers.onTouchEnd({ ...contact(100, 200, 0), ...touch });
    expect(action).not.toHaveBeenCalled();
    expect(spend).toHaveBeenCalledTimes(1);
    expect(takePressOutcome(5_000)).toBe('refused');
  });

  it('a held Stop still stops', () => {
    const { face, action, handlers, lift, advance } = morph();
    face.destructive = true;
    handlers.onTouchStart(contact(100, 200, 1));
    advance(1_900);
    handlers.onTouchEnd(lift);
    expect(action).toHaveBeenCalledTimes(1);
  });
});

describe('a press that scrolled a container above the button serves nothing', () => {
  function insideScroller() {
    const root: ScrollNode = { scrollTop: 0, scrollLeft: 0, parentElement: null };
    const scroller: ScrollNode = { scrollTop: 0, scrollLeft: 0, parentElement: root };
    const row: ScrollNode = { scrollTop: 0, scrollLeft: 0, parentElement: scroller };
    const { action, handlers, touch } = harness();
    const press = { ...contact(100, 200, 1), currentTarget: row };
    const liftAt = (x: number, y: number) => ({ ...contact(x, y, 0), ...touch });
    return { scroller, root, action, handlers, press, liftAt };
  }

  it('a lift after the menu scrolled runs nothing, and leaves no click behind', () => {
    const { scroller, action, handlers, press, liftAt } = insideScroller();
    handlers.onTouchStart(press);
    scroller.scrollTop = 140;
    const lift = liftAt(100, 60);
    handlers.onTouchEnd(lift);
    expect(action).not.toHaveBeenCalled();
    expect(lift.preventDefault).toHaveBeenCalledTimes(1);
    handlers.onClick();
    expect(action).not.toHaveBeenCalled();
  });

  it('a sideways scroll counts too', () => {
    const { scroller, action, handlers, press, liftAt } = insideScroller();
    handlers.onTouchStart(press);
    scroller.scrollLeft = 60;
    handlers.onTouchEnd(liftAt(40, 200));
    expect(action).not.toHaveBeenCalled();
  });

  it('a tap with nothing scrolled still serves', () => {
    const { action, handlers, press, liftAt } = insideScroller();
    handlers.onTouchStart(press);
    handlers.onTouchEnd(liftAt(100, 200));
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('the app scrolling the container under a still finger is not a scroll', () => {
    const { scroller, action, handlers, press, liftAt } = insideScroller();
    handlers.onTouchStart(press);
    scroller.scrollTop = 140;
    handlers.onTouchEnd(liftAt(100, 200 + TAP_MOVE_THRESHOLD_PX));
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('the document scrolling is not a scroll of the menu', () => {
    const { root, action, handlers, press, liftAt } = insideScroller();
    handlers.onTouchStart(press);
    root.scrollTop = 300;
    handlers.onTouchEnd(liftAt(100, 60));
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('a cancelled press that scrolled the menu serves nothing', () => {
    const { scroller, action, handlers, press } = insideScroller();
    handlers.onTouchStart(press);
    scroller.scrollTop = 40;
    handlers.onTouchCancel(contact(100, 160, 0));
    expect(action).not.toHaveBeenCalled();
  });
});

describe('a destructive face refuses a press that began before it was shown', () => {
  function stamped(timeStamp: number) {
    return { ...contact(100, 200, 1), timeStamp };
  }

  it('refuses a queued tap whose touch predates the paint, and claims it', () => {
    takePressOutcome(0);
    const { action, handlers, touch } = harness({ destructive: () => true, shownAt: () => 5_000 });
    handlers.onTouchStart(stamped(4_900));
    handlers.onTouchEnd(touch);
    expect(action).not.toHaveBeenCalled();
    expect(touch.preventDefault).toHaveBeenCalledTimes(1);
    expect(takePressOutcome(5_000)).toBe('refused');
  });

  it('refuses a tap faster than anyone could react to the new face', () => {
    const { action, handlers, touch } = harness({ destructive: () => true, shownAt: () => 5_000 });
    handlers.onTouchStart(stamped(5_000 + FACE_SEEN_MS - 1));
    handlers.onTouchEnd(touch);
    expect(action).not.toHaveBeenCalled();
  });

  it('refuses a tap while the face is not painted yet', () => {
    const { action, handlers, touch } = harness({ destructive: () => true, shownAt: () => null });
    handlers.onTouchStart(stamped(4_900));
    handlers.onTouchEnd(touch);
    expect(action).not.toHaveBeenCalled();
  });

  it('serves a tap on the face once the user has seen it', () => {
    const { action, handlers, touch } = harness({ destructive: () => true, shownAt: () => 5_000 });
    handlers.onTouchStart(stamped(5_000 + FACE_SEEN_MS));
    handlers.onTouchEnd(touch);
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('refuses a queued tap the system cancelled, too', () => {
    const { action, handlers } = harness({
      destructive: () => true,
      shownAt: () => 5_000,
      pageVisible: () => true,
    });
    vi.useFakeTimers();
    try {
      handlers.onTouchStart(stamped(4_900));
      handlers.onTouchCancel(contact(100, 200, 0));
      vi.advanceTimersByTime(CANCELLED_TAP_SETTLE_MS);
      expect(action).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it('leaves a constructive face alone: Send serves a queued tap', () => {
    const { action, handlers, touch } = harness({ destructive: () => false, shownAt: () => 5_000 });
    handlers.onTouchStart(stamped(4_900));
    handlers.onTouchEnd(touch);
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('judges a touch with no timeStamp as before', () => {
    expect(pressPredatesFace(null, 5_000)).toBe(false);
    expect(pressPredatesFace(null, null)).toBe(false);
  });
});

describe('a face standing down claims the press', () => {
  it('claims a lift on a disabled touch path as refused, so the probe stays quiet', () => {
    takePressOutcome(0);
    const { handlers, touch } = harness({ enabled: () => false });
    handlers.onTouchEnd(touch);
    expect(takePressOutcome(5_000)).toBe('refused');
  });
});
