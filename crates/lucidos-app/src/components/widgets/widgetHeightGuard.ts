
export const HEIGHT_JITTER_PX = 1;

export const FLIP_LOCK_COUNT = 4;
export const FLIP_WINDOW_MS = 1000;

export const CREEP_LOCK_FRAMES = 10;

export const CONSECUTIVE_FRAME_MS = 250;

export type HeightLockReason = 'oscillation' | 'runaway-growth';

export interface HeightGuardState {
  height: number;
  locked: boolean;
  lastDelta: number;
  lastAt: number | null;
  flips: readonly number[];
  creepSteps: number;
  creepFrom: number;
}

export type HeightDecision =
  | { kind: 'ignore' }
  | { kind: 'apply'; height: number }
  | { kind: 'lock'; height: number; reason: HeightLockReason };

export function initialHeightGuard(height: number): HeightGuardState {
  return { height, locked: false, lastDelta: 0, lastAt: null, flips: [], creepSteps: 0, creepFrom: height };
}

export function decideHeight(
  state: HeightGuardState,
  reported: number,
  now: number,
): { state: HeightGuardState; decision: HeightDecision } {
  const delta = reported - state.height;
  if (state.locked || Math.abs(delta) < HEIGHT_JITTER_PX) return { state, decision: { kind: 'ignore' } };

  const consecutive = state.lastAt !== null && now - state.lastAt <= CONSECUTIVE_FRAME_MS;
  const flipped = state.lastDelta !== 0 && Math.sign(delta) !== Math.sign(state.lastDelta);
  const flips = [...state.flips.filter((at) => now - at < FLIP_WINDOW_MS), ...(flipped ? [now] : [])];
  const creeping = delta > 0 && delta === state.lastDelta && consecutive;
  const creepSteps = delta > 0 ? (creeping ? state.creepSteps + 1 : 1) : 0;
  const creepFrom = creeping ? state.creepFrom : state.height;

  const lock = (height: number, reason: HeightLockReason) => ({
    state: { ...state, height, locked: true },
    decision: { kind: 'lock' as const, height, reason },
  });
  if (flips.length >= FLIP_LOCK_COUNT) return lock(Math.max(state.height, reported), 'oscillation');
  if (creepSteps >= CREEP_LOCK_FRAMES) return lock(creepFrom, 'runaway-growth');

  return {
    state: { height: reported, locked: false, lastDelta: delta, lastAt: now, flips, creepSteps, creepFrom },
    decision: { kind: 'apply', height: reported },
  };
}
