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
function harness(opts: { enabled?: () => boolean; gate?: { pass(): boolean; spend(): void } } = {}) {
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

/** The bug reported a THIRD time, after touch activation had landed and then
 *  been retuned: the composer's Send still did nothing on a phone with the
 *  keyboard up, wherever the finger pressed, until the keyboard was dismissed.
 *
 *  Both of the touch path's tests so far asked whether the finger was still on
 *  the button at the lift. Both cost a coordinate comparison across the span of
 *  a press, and the visual viewport settles under a stationary finger while the
 *  keyboard is up. Screen space did not escape that. Each shipped as a SILENT
 *  decline onto a click iOS was not going to send.
 *
 *  So the touch path now takes every press. These cases pin that, because it is
 *  the only path there is on the device that reported this. */
describe('touchActivated takes every press it is given', () => {
  it('runs the action however far the finger travelled', () => {
    // A press that slid off a constructive button now fires it. That is the
    // trade: the alternative measured something the platform corrupts, and paid
    // for a wrong answer with a dead button nobody could diagnose.
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
});

/** Where the scroll-vs-tap gate lives now. What it catches is the `click` iOS
 *  fires after a touch that was starting a scroll, so it belongs to the click
 *  path. In front of BOTH paths it also vetoed the touch path, on the same
 *  measurement described above, which turned the dead button into a toast. */
describe('the tap gate guards the click path alone', () => {
  /** A gate wired the way `morphActivationGate` wires it, plus a report spy. */
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
      spend: () => gate.cancel(),
    };
    return { gate, reported, activation };
  }

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
    // The gate holds ONE press. An unspent one would veto the next activation
    // arriving with no press of its own, such as a keyboard Enter on the same
    // button.
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
    // The twin check runs before the gate, so a browser that dispatches the
    // suppressed click anyway does not reach it at all.
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
