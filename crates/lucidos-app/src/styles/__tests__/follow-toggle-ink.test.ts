/**
 * The follow toggle lands the ink the rest of the composer row lands.
 *
 * That row's stroke glyphs each paint 0.750 of their own viewBox. `--run-ink`
 * divides the three agent marks down to the same fraction (see
 * `prompt-mark-optical-size.test.ts` and `.commands-btn` in
 * chat/input-messages.css). The toggle is not a `.commands-btn`, so that test
 * never looked at it and nothing held the fraction. A redraw duly reached
 * review at 0.667, a ninth smaller than everything beside it.
 *
 * **0.750 is the GEOMETRIC extent, stroke excluded.** Include the stroke and
 * every glyph in the run reads 0.833 instead. A check on the other basis
 * rejects a correct glyph and accepts a wrong one. Deriving it by hand off the
 * path data is what this file does, deliberately. The shape is axis-aligned
 * segments plus semicircles on horizontal chords, and their extremes are all
 * on-path, so the bbox is exact.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const icons = readFileSync(resolve(here, '../../components/shared/icons.tsx'), 'utf8');
const css = readFileSync(resolve(here, '../chat/input-messages.css'), 'utf8');

/** Geometric bbox of one path, stroke excluded.
 *
 *  Handles only the commands this glyph uses. An arc must be a semicircle on a
 *  horizontal chord, and that is asserted rather than assumed. A redraw
 *  reaching for a general ellipse fails here instead of being mismeasured. */
function bounds(d: string): { xs: number[]; ys: number[] } {
  const xs: number[] = [];
  const ys: number[] = [];
  let x = 0;
  let y = 0;
  let startX = 0;
  let startY = 0;
  const steps = d.match(/[MHVAZmhvaz][^MHVAZmhvaz]*/g) ?? [];
  expect(steps.join(''), `unhandled path command in "${d}"`).toBe(d);

  for (const step of steps) {
    const args = (step.slice(1).match(/-?\d*\.?\d+/g) ?? []).map(Number);
    switch (step[0]) {
      case 'M':
        [x, y] = args;
        startX = x;
        startY = y;
        break;
      case 'h':
        x += args[0];
        break;
      case 'H':
        [x] = args;
        break;
      case 'v':
        y += args[0];
        break;
      case 'V':
        [y] = args;
        break;
      case 'a': {
        const [rx, ry, , , sweep, dx, dy] = args;
        expect(dy, 'arc chord is not horizontal').toBe(0);
        expect(rx, 'arc is not circular').toBe(ry);
        expect(Math.abs(dx) / 2, 'arc is not a semicircle').toBeCloseTo(rx, 6);
        // Sweep 1 left to right bulges up in SVG's y-down space, and each
        // reversal flips it. The apex is the only extreme off the endpoints.
        ys.push((sweep === 1) === dx > 0 ? y - rx : y + rx);
        x += dx;
        break;
      }
      case 'Z':
      case 'z':
        x = startX;
        y = startY;
        break;
      default:
        throw new Error(`unhandled command ${step[0]}`);
    }
    xs.push(x);
    ys.push(y);
  }
  return { xs, ys };
}

/** A glyph's painted fraction of its own box, per axis. */
function inkFraction(ds: string[], side: number): { x: number; y: number } {
  const xs: number[] = [];
  const ys: number[] = [];
  for (const d of ds) {
    const b = bounds(d);
    xs.push(...b.xs);
    ys.push(...b.ys);
  }
  return {
    x: (Math.max(...xs) - Math.min(...xs)) / side,
    y: (Math.max(...ys) - Math.min(...ys)) / side,
  };
}

/** The `d` of every path the named icon draws, plus its square viewBox side. */
function glyph(name: string): { ds: string[]; side: number } {
  const body = new RegExp(`export function ${name}\\(\\) \\{([\\s\\S]*?)\\n\\}`).exec(icons);
  expect(body, `icons.tsx no longer exports ${name}()`).not.toBeNull();
  const view = /viewBox="0 0 (\d+) \1"/.exec(body![1]);
  expect(view, `${name} is not on a square viewBox`).not.toBeNull();
  const ds = [...body![1].matchAll(/<path d="([^"]+)"/g)].map(m => m[1]);
  expect(ds.length, `${name} draws no paths`).toBeGreaterThan(0);
  return { ds, side: Number(view![1]) };
}

describe('the ink measurement itself', () => {
  it('measures an axis-aligned box', () => {
    const ink = inkFraction(['M3 3h18v18h-18z'], 24);
    expect(ink.x).toBeCloseTo(0.75, 6);
    expect(ink.y).toBeCloseTo(0.75, 6);
  });

  it('puts a semicircle\'s apex above the chord when it bulges up', () => {
    // From (4,12) rightwards, sweep 1: apex at y=4, so the height is 8 and the
    // width is the 16 chord.
    const ink = inkFraction(['M4 12a8 8 0 0 1 16 0'], 24);
    expect(ink.x).toBeCloseTo(16 / 24, 6);
    expect(ink.y).toBeCloseTo(8 / 24, 6);
  });

  it('puts it below when the sweep reverses', () => {
    expect(bounds('M4 12a8 8 0 0 0 16 0').ys).toContain(20);
    expect(inkFraction(['M4 12a8 8 0 0 0 16 0'], 24).y).toBeCloseTo(8 / 24, 6);
  });
});

describe('the follow toggle is drawn to the composer row\'s ink', () => {
  it('takes the fraction from --run-ink, so retuning the row retunes this', () => {
    // Read rather than hardcoded: the sibling test pins the whole declaration,
    // this one only needs the number inside it.
    const run = /--run-ink:\s*calc\(var\(--icon-size-lg\)\s*\*\s*([\d.]+)\)/.exec(css);
    expect(run, 'input-messages.css no longer declares --run-ink').not.toBeNull();
    expect(Number(run![1])).toBe(0.75);
  });

  it('paints that fraction of its box on both axes', () => {
    const { ds, side } = glyph('FollowLiveEdgeIcon');
    const ink = inkFraction(ds, side);
    expect(ink.x, 'width drifted off the run').toBeCloseTo(0.75, 3);
    expect(ink.y, 'height drifted off the run').toBeCloseTo(0.75, 3);
  });
});
