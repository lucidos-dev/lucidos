import { describe, it, expect, vi } from 'vitest';
import { createTapGate, touchActivated } from './tapGesture';

/** A pointer carrying BOTH coordinate spaces, so a test can move one and hold
 *  the other still. That is the whole distinction the gate rests on. */
interface Probe {
  screenX: number;
  screenY: number;
  clientX: number;
  clientY: number;
}

/** Finger on the screen at (x, y), page viewport aligned with the screen. */
function at(x: number, y: number): Probe {
  return { screenX: x, screenY: y, clientX: x, clientY: y };
}

describe('createTapGate', () => {
  it('treats a click without a recorded press as a tap (keyboard activation)', () => {
    // Tab+Enter/Space on a focused button fires a synthetic `click` with no
    // preceding `pointerdown`. Suppressing those would break keyboard a11y.
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
    gate.move(at(105, 203)); // 5,3 px of finger wobble during a real tap
    expect(gate.isTap()).toBe(true);
  });

  it('cancels the tap when movement exceeds the threshold (vertical scroll)', () => {
    const gate = createTapGate();
    gate.down(at(100, 200));
    gate.move(at(101, 230)); // 30 px down, the user starting a scroll
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
    // pointercancel (iOS killing the gesture) should not poison the next
    // unrelated activation. Once cleared, the gate behaves as if no press
    // happened, which means the next click is treated as a tap.
    const gate = createTapGate();
    gate.down(at(0, 0));
    gate.move(at(0, 100)); // would have canceled
    gate.cancel();
    expect(gate.isTap()).toBe(true);
  });
});

/** The reported bug: on an iOS PWA with the keyboard up, the composer's Submit
 *  did nothing tap after tap, while the ungated icon buttons beside it worked.
 *  Only the gated buttons were dead, and the gate measured the finger against
 *  the page viewport instead of against the screen. */
describe('createTapGate measures the finger, not the viewport', () => {
  it('holds a stationary finger to be a tap while the viewport scrolls under it', () => {
    const press: Probe = { screenX: 100, screenY: 700, clientX: 100, clientY: 300 };
    // The visual viewport settles by 40 px with the keyboard up. The finger
    // never moved, so its screen position is unchanged.
    const settled: Probe = { screenX: 100, screenY: 700, clientX: 100, clientY: 340 };
    const gate = createTapGate();
    gate.down(press);
    gate.move(settled);
    expect(gate.isTap()).toBe(true);
  });

  it('cancels a finger that crossed the screen while the content travelled with it', () => {
    // The mirror case. During a content scroll the page moves under the
    // finger, so the client position barely changes across a real swipe.
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
    // A swipe crosses 8 px early and travels on. Reporting the crossing
    // instead of the travel makes every rejection read as a hair over the
    // threshold. That is the one thing the number has to tell apart.
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
    // Both entry points settle the same in-flight press, which is what lets a
    // caller pick one without the other leaving a verdict behind.
    const gate = createTapGate();
    gate.down(at(100, 200));
    gate.move(at(100, 240));
    expect(gate.tapRejection()).toBe(40);
    expect(gate.tapRejection()).toBeNull();
  });
});

/** The reported bug: the composer's Send did nothing on a phone with the
 *  keyboard up. Nothing on screen said why, because the click WebKit dropped
 *  never reached the handler that would have toasted. */
/** A clock the test moves by hand, plus a countable event. */
function harness(opts: { enabled?: () => boolean } = {}) {
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
    // preventDefault on touchend is what suppresses the compatibility mouse
    // events. Without it both paths would fire and the message would go twice.
    const { handlers, touch } = harness();
    handlers.onTouchEnd(touch);
    expect(touch.preventDefault).toHaveBeenCalledTimes(1);
  });

  it('ignores a click that arrives as the twin of a touch it already served', () => {
    // The belt to preventDefault's suspenders: a browser that dispatches the
    // click anyway must not send a second message.
    const { action, handlers, touch, advance } = harness();
    handlers.onTouchEnd(touch);
    advance(50);
    handlers.onClick();
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('runs the action on a click with no touch behind it', () => {
    // Desktop, and keyboard activation, which fires a click and no touch.
    const { action, handlers } = harness();
    handlers.onClick();
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('serves a click long after the last touch', () => {
    // A hybrid device: a finger, then a trackpad. A sticky handled-by-touch
    // flag would eat this click, because the suppressed twin never arrives to
    // clear it.
    const { action, handlers, touch, advance } = harness();
    handlers.onTouchEnd(touch);
    advance(5_000);
    handlers.onClick();
    expect(action).toHaveBeenCalledTimes(2);
  });

  it('serves a second real tap inside the twin window', () => {
    // The window guards the click path only. Two quick taps are two actions.
    const { action, handlers, touch, advance } = harness();
    handlers.onTouchEnd(touch);
    advance(120);
    handlers.onTouchEnd(touch);
    expect(action).toHaveBeenCalledTimes(2);
  });

  it('eats one twin per touch, never a second click inside the window', () => {
    // A touch has one synthetic twin. Swallowing every click for the whole
    // window would eat a real press that followed it, which is a live hazard on
    // the morph button: it shares one handler pair across Send and Cancel.
    const { action, handlers, touch, advance } = harness();
    handlers.onTouchEnd(touch);
    advance(50);
    handlers.onClick();
    advance(50);
    handlers.onClick();
    expect(action).toHaveBeenCalledTimes(2);
  });
});

/** A touchend carrying where the finger lifted, against a 40x40 button whose
 *  top-left sits at (100, 200). */
function liftAt(x: number, y: number) {
  return {
    preventDefault: vi.fn(),
    changedTouches: [{ clientX: x, clientY: y }],
    currentTarget: { getBoundingClientRect: () => ({ left: 100, right: 140, top: 200, bottom: 240 }) },
  };
}

/** A `touchend` is dispatched to the element the touch STARTED on, wherever it
 *  ends. So a press that slid off the button reaches the handler looking like a
 *  tap. Touch activation stands in for the click, so it must reject exactly
 *  what the click would have rejected. Found twice in review, once here and
 *  once by Codex, against the multi-select Submit, which carries no tap gate. */
describe('touchActivated only fires where a click would have', () => {
  it('runs the action when the finger lifted on the button', () => {
    const action = vi.fn();
    const handlers = touchActivated(action);
    handlers.onTouchEnd(liftAt(120, 220));
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('runs it at the very edge of the button', () => {
    const action = vi.fn();
    const handlers = touchActivated(action);
    handlers.onTouchEnd(liftAt(140, 240));
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('declines a press that slid off before lifting', () => {
    // A scroll that began on the button. The finger left, so no click would
    // have fired, and the touch path must not submit on its own.
    const action = vi.fn();
    const handlers = touchActivated(action);
    const lift = liftAt(120, 400);
    handlers.onTouchEnd(lift);
    expect(action).not.toHaveBeenCalled();
  });

  it('leaves the click alive after declining, rather than cancelling it', () => {
    // Nothing was activated, so nothing may be suppressed. A cancelled default
    // here would silently kill a click the browser was right to send.
    const action = vi.fn();
    const handlers = touchActivated(action);
    const lift = liftAt(400, 220);
    handlers.onTouchEnd(lift);
    expect(lift.preventDefault).not.toHaveBeenCalled();
    handlers.onClick();
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('runs the action when the event cannot say where the finger was', () => {
    // Mirrors the gate treating a click with no press behind it as a tap. A
    // real browser always fills both fields; a synthetic dispatch may not.
    const action = vi.fn();
    const handlers = touchActivated(action);
    handlers.onTouchEnd({ preventDefault: vi.fn() });
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('runs the action when the target is gone but a point is given', () => {
    const action = vi.fn();
    const handlers = touchActivated(action);
    handlers.onTouchEnd({ preventDefault: vi.fn(), changedTouches: [{ clientX: 1, clientY: 2 }], currentTarget: null });
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('stands the touch path down while disabled, leaving the click path whole', () => {
    // The morph button is one node that turns destructive. While it reads
    // Cancel the touch path must not fire it early, AND must not swallow the
    // click that still has to reach it.
    const { action, handlers, touch } = harness({ enabled: () => false });
    handlers.onTouchEnd(touch);
    expect(action).not.toHaveBeenCalled();
    expect(touch.preventDefault).not.toHaveBeenCalled();
    handlers.onClick();
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('lets a gate rejection kill both paths, from one settle', () => {
    // How the two compose: the gate check lives inside the action, so one
    // press is settled once whichever path fires. A scroll that starts on the
    // button must send nothing and report once.
    const gate = createTapGate();
    const sent = vi.fn();
    const reported = vi.fn();
    const handlers = touchActivated(() => {
      const moved = gate.tapRejection();
      if (moved !== null) { reported(moved); return; }
      sent();
    });
    gate.down(at(100, 200));
    gate.move(at(100, 240));
    handlers.onTouchEnd({ preventDefault: vi.fn() });
    handlers.onClick();
    expect(sent).not.toHaveBeenCalled();
    expect(reported).toHaveBeenCalledTimes(1);
    expect(reported).toHaveBeenCalledWith(40);
  });
});
