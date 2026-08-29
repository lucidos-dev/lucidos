import { describe, it, expect } from 'vitest';
import { BARGE_IN_DEFAULTS, BARGE_IN_IDLE, frameEnergy, stepBargeIn } from './bargeIn';

const LOUD = BARGE_IN_DEFAULTS.threshold * 2;
const QUIET = BARGE_IN_DEFAULTS.threshold / 2;

/** Feed a run of frames and report every step that fired. */
function run(frames: { rms: number; talkerSpeaking: boolean }[]): boolean[] {
  let state = BARGE_IN_IDLE;
  const fires: boolean[] = [];
  for (const frame of frames) {
    const step = stepBargeIn(state, frame);
    state = step.state;
    fires.push(step.fire);
  }
  return fires;
}

function speaking(rms: number, count: number) {
  return Array.from({ length: count }, () => ({ rms, talkerSpeaking: true }));
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

describe('the barge-in gate', () => {
  it('stays shut while the talker is silent, however loud the caller is', () => {
    const fires = run([
      { rms: LOUD, talkerSpeaking: false },
      { rms: LOUD, talkerSpeaking: false },
      { rms: LOUD, talkerSpeaking: false },
      { rms: LOUD, talkerSpeaking: false },
    ]);
    expect(fires).toEqual([false, false, false, false]);
  });

  it('needs a run of loud frames, not one', () => {
    const fires = run(speaking(LOUD, BARGE_IN_DEFAULTS.framesToFire));
    expect(fires.slice(0, -1).every((f) => !f)).toBe(true);
    expect(fires[fires.length - 1]).toBe(true);
  });

  it('forgets a run that a quiet frame breaks', () => {
    const fires = run([
      ...speaking(LOUD, BARGE_IN_DEFAULTS.framesToFire - 1),
      { rms: QUIET, talkerSpeaking: true },
      ...speaking(LOUD, BARGE_IN_DEFAULTS.framesToFire - 1),
    ]);
    expect(fires.some((f) => f)).toBe(false);
  });

  it('fires once per talker turn, however long the caller keeps talking', () => {
    const fires = run(speaking(LOUD, BARGE_IN_DEFAULTS.framesToFire + 10));
    expect(fires.filter((f) => f)).toHaveLength(1);
  });

  it('re-arms once the talker speaks again', () => {
    const turn = speaking(LOUD, BARGE_IN_DEFAULTS.framesToFire);
    const fires = run([...turn, { rms: QUIET, talkerSpeaking: false }, ...turn]);
    expect(fires.filter((f) => f)).toHaveLength(2);
  });

  it('takes its threshold from the caller, so a noisy room can be tuned', () => {
    const settings = { threshold: 0.5, framesToFire: 1 };
    const step = stepBargeIn(BARGE_IN_IDLE, { rms: LOUD, talkerSpeaking: true }, settings);
    expect(step.fire).toBe(false);
  });
});
