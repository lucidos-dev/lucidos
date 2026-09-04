import { describe, it, expect } from 'vitest';
import {
  SPEECH_GATE_DEFAULTS,
  SPEECH_GATE_SHUT,
  type SpeechGateState,
  frameEnergy,
  stepSpeechGate,
} from './speechGate';

const LOUD = SPEECH_GATE_DEFAULTS.threshold * 2;
const QUIET = SPEECH_GATE_DEFAULTS.threshold / 2;

/** Feed a run of frames and report whether the gate was open after each. */
function run(rmss: number[], from: SpeechGateState = SPEECH_GATE_SHUT): boolean[] {
  let state = from;
  const open: boolean[] = [];
  for (const rms of rmss) {
    state = stepSpeechGate(state, rms);
    open.push(state.open);
  }
  return open;
}

function frames(rms: number, count: number): number[] {
  return Array.from({ length: count }, () => rms);
}

/** A gate already open, reached the only way there is. */
function opened(): SpeechGateState {
  let state = SPEECH_GATE_SHUT;
  for (const rms of frames(LOUD, SPEECH_GATE_DEFAULTS.framesToOpen)) {
    state = stepSpeechGate(state, rms);
  }
  return state;
}

describe('frame energy', () => {
  it('is zero for silence and for an empty frame', () => {
    expect(frameEnergy(new Float32Array(64))).toBe(0);
    expect(frameEnergy(new Float32Array(0))).toBe(0);
  });

  it('is the root mean square, so sign does not cancel', () => {
    expect(frameEnergy(new Float32Array([0.5, -0.5, 0.5, -0.5]))).toBeCloseTo(0.5, 10);
  });
});

describe('opening the gate', () => {
  it('needs a run of loud frames, not one', () => {
    const open = run(frames(LOUD, SPEECH_GATE_DEFAULTS.framesToOpen));
    expect(open.slice(0, -1).every((o) => !o)).toBe(true);
    expect(open[open.length - 1]).toBe(true);
  });

  it('forgets a run that a quiet frame breaks', () => {
    const open = run([
      ...frames(LOUD, SPEECH_GATE_DEFAULTS.framesToOpen - 1),
      QUIET,
      ...frames(LOUD, SPEECH_GATE_DEFAULTS.framesToOpen - 1),
    ]);
    expect(open.some((o) => o)).toBe(false);
  });

  it('stays open for as long as the caller keeps talking', () => {
    const open = run(frames(LOUD, SPEECH_GATE_DEFAULTS.framesToOpen + 20));
    expect(open[open.length - 1]).toBe(true);
  });
});

describe('shutting the gate', () => {
  it('rides out the gap between two words', () => {
    const open = run(frames(QUIET, SPEECH_GATE_DEFAULTS.framesToClose - 1), opened());
    expect(open.every((o) => o)).toBe(true);
  });

  it('shuts once the quiet run is long enough', () => {
    const open = run(frames(QUIET, SPEECH_GATE_DEFAULTS.framesToClose), opened());
    expect(open[open.length - 1]).toBe(false);
  });

  it('forgets a quiet run that a loud frame breaks', () => {
    const open = run(
      [
        ...frames(QUIET, SPEECH_GATE_DEFAULTS.framesToClose - 1),
        LOUD,
        ...frames(QUIET, SPEECH_GATE_DEFAULTS.framesToClose - 1),
      ],
      opened(),
    );
    expect(open.every((o) => o)).toBe(true);
  });
});

/** An edge is what the reducer reads, so a frame that moves nothing must cost
 *  nothing. Twenty-five of them arrive every second. */
describe('a frame that changes nothing', () => {
  it('hands back the same state, so a caller can test for an edge by identity', () => {
    const shut = stepSpeechGate(SPEECH_GATE_SHUT, QUIET);
    expect(shut).toBe(SPEECH_GATE_SHUT);
    const open = opened();
    expect(stepSpeechGate(open, LOUD)).toBe(open);
  });
});

describe('tuning', () => {
  it('takes its threshold from the caller, so a noisy room can be tuned', () => {
    const settings = { threshold: 0.5, framesToOpen: 1, framesToClose: 1 };
    expect(stepSpeechGate(SPEECH_GATE_SHUT, LOUD, settings).open).toBe(false);
    expect(stepSpeechGate(SPEECH_GATE_SHUT, 0.6, settings).open).toBe(true);
  });
});
