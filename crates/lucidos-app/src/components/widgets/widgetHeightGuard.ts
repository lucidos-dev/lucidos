/**
 * The *widget height guard*: how a widget frame's height follows the sizes its
 * widget reports (docs/glossary.md). A pure decision, so `WidgetFrame` owns only
 * the wiring: it coalesces reports to one per animation frame and asks here.
 *
 * Two shapes never settle, and each locks the frame for the rest of its mount:
 * - reports that flip direction again and again, as a layout feedback loop does;
 * - growth by the same step in consecutive frames, as a body sized `100vh` plus
 *   a margin does, since its content height follows the frame's.
 */

/** A change smaller than this is layout noise, never applied. */
export const HEIGHT_JITTER_PX = 1;

/** Direction flips inside {@link FLIP_WINDOW_MS} that lock the frame. */
export const FLIP_LOCK_COUNT = 4;
export const FLIP_WINDOW_MS = 1000;

/** Equal growth steps in consecutive frames that lock the frame. An animated
 *  disclosure eases and rounds, so it never repeats one step this many times. */
export const CREEP_LOCK_FRAMES = 10;

/** Two applies this close together came from one growth loop. Each report
 *  crosses a postMessage and an animation frame. A loaded phone or a CI
 *  runner takes well over one 16 ms frame per round. A tighter bound reads a
 *  slow loop as separate user steps, restarts the count, and lets the frame
 *  settle far above where the growth began. A person adding the same step ten
 *  times in a row is never this fast. */
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

/** What to do with one coalesced report at frame time `now` (ms). */
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
  // The larger of the two sizes it swings between, so nothing is clipped.
  if (flips.length >= FLIP_LOCK_COUNT) return lock(Math.max(state.height, reported), 'oscillation');
  // Back where the growth began: every step after it was the loop.
  if (creepSteps >= CREEP_LOCK_FRAMES) return lock(creepFrom, 'runaway-growth');

  return {
    state: { height: reported, locked: false, lastDelta: delta, lastAt: now, flips, creepSteps, creepFrom },
    decision: { kind: 'apply', height: reported },
  };
}
