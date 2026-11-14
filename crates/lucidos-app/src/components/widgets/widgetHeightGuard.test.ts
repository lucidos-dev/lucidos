import { describe, expect, it } from 'vitest';
import {
  CREEP_LOCK_FRAMES,
  FLIP_LOCK_COUNT,
  FLIP_WINDOW_MS,
  decideHeight,
  initialHeightGuard,
  type HeightDecision,
} from './widgetHeightGuard';

const FRAME_MS = 16;

/** Feeds `reports` one per frame from `start`, and returns each decision. */
function run(reports: number[], from = 240, frameMs = FRAME_MS): HeightDecision[] {
  let state = initialHeightGuard(from);
  return reports.map((px, i) => {
    const next = decideHeight(state, px, i * frameMs);
    state = next.state;
    return next.decision;
  });
}

const kinds = (decisions: HeightDecision[]) => decisions.map((d) => d.kind);

describe('decideHeight', () => {
  it('ignores a change under a pixel', () => {
    expect(run([240.5, 239.2])).toEqual([{ kind: 'ignore' }, { kind: 'ignore' }]);
  });

  it('follows a widget that grows when its data lands, then shrinks', () => {
    expect(run([320, 480, 180])).toEqual([
      { kind: 'apply', height: 320 },
      { kind: 'apply', height: 480 },
      { kind: 'apply', height: 180 },
    ]);
  });

  it('follows an eased disclosure, open and shut', () => {
    const open = [252, 280, 318, 352, 376, 392, 400];
    const shut = [388, 360, 320, 284, 258, 244, 240];
    expect(kinds(run([...open, ...shut])).every((k) => k === 'apply')).toBe(true);
  });

  it('follows a disclosure toggled a few times a second apart', () => {
    expect(kinds(run([400, 240, 400, 240, 400], 240, FLIP_WINDOW_MS)).every((k) => k === 'apply')).toBe(true);
  });

  it('locks a frame that swings between two sizes, at the larger', () => {
    const decisions = run([300, 260, 300, 260, 300, 260]);
    const lock = decisions.findIndex((d) => d.kind === 'lock');
    expect(lock).toBe(FLIP_LOCK_COUNT);
    expect(decisions[lock]).toEqual({ kind: 'lock', height: 300, reason: 'oscillation' });
    expect(kinds(decisions.slice(lock + 1))).toEqual(['ignore']);
  });

  it('locks a 100vh body creeping by its margin each frame, back where it began', () => {
    const creep = Array.from({ length: 40 }, (_, i) => 240 + 16 * (i + 1));
    const decisions = run(creep);
    const lock = decisions.findIndex((d) => d.kind === 'lock');
    expect(lock).toBe(CREEP_LOCK_FRAMES - 1);
    expect(decisions[lock]).toEqual({ kind: 'lock', height: 240, reason: 'runaway-growth' });
    expect(kinds(decisions.slice(lock + 1)).every((k) => k === 'ignore')).toBe(true);
  });

  it('locks a creep whose reports take a slow device 150 ms each, back where it began', () => {
    const creep = Array.from({ length: 40 }, (_, i) => 240 + 16 * (i + 1));
    const decisions = run(creep, 240, 150);
    const lock = decisions.findIndex((d) => d.kind === 'lock');
    expect(lock).toBe(CREEP_LOCK_FRAMES - 1);
    expect(decisions[lock]).toEqual({ kind: 'lock', height: 240, reason: 'runaway-growth' });
  });

  it('follows equal steps that are not in consecutive frames', () => {
    const steps = Array.from({ length: 20 }, (_, i) => 240 + 16 * (i + 1));
    expect(kinds(run(steps, 240, 500)).every((k) => k === 'apply')).toBe(true);
  });
});
