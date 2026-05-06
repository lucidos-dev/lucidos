import { describe, it, expect } from 'vitest';
import { createTapGate } from './tapGesture';

describe('createTapGate', () => {
  it('treats a click without a recorded press as a tap (keyboard activation)', () => {
    // Tab+Enter/Space on a focused button fires a synthetic `click` with no
    // preceding `pointerdown`. Suppressing those would break keyboard a11y.
    const gate = createTapGate();
    expect(gate.isTap()).toBe(true);
  });

  it('treats a press with no movement as a tap', () => {
    const gate = createTapGate();
    gate.down(100, 200);
    expect(gate.isTap()).toBe(true);
  });

  it('treats a press with sub-threshold wobble as a tap', () => {
    const gate = createTapGate();
    gate.down(100, 200);
    gate.move(105, 203); // 5,3 px — finger wobble during a real tap
    expect(gate.isTap()).toBe(true);
  });

  it('cancels the tap when movement exceeds the threshold (vertical scroll)', () => {
    const gate = createTapGate();
    gate.down(100, 200);
    gate.move(101, 230); // 30 px down — user starting a scroll
    expect(gate.isTap()).toBe(false);
  });

  it('cancels the tap when movement exceeds the threshold (horizontal swipe)', () => {
    const gate = createTapGate();
    gate.down(100, 200);
    gate.move(150, 200);
    expect(gate.isTap()).toBe(false);
  });

  it('stays canceled even if the finger drifts back to the start', () => {
    const gate = createTapGate();
    gate.down(100, 200);
    gate.move(100, 250);
    gate.move(100, 200);
    expect(gate.isTap()).toBe(false);
  });

  it('resets between presses so a canceled gesture does not poison the next', () => {
    const gate = createTapGate();
    gate.down(0, 0);
    gate.move(0, 100);
    expect(gate.isTap()).toBe(false);

    gate.down(50, 50);
    expect(gate.isTap()).toBe(true);
  });

  it('cancel() clears the in-flight press so a follow-up click is not auto-suppressed', () => {
    // pointercancel (iOS killing the gesture) should not poison the next
    // unrelated activation — once cleared, the gate behaves as if no press
    // happened, which means the next click is treated as a tap.
    const gate = createTapGate();
    gate.down(0, 0);
    gate.move(0, 100); // would have canceled
    gate.cancel();
    expect(gate.isTap()).toBe(true);
  });
});
