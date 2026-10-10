// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { anchorSpacer, setAnchorSpacer, splitAnchorCorrection } from './anchorCorrection';

describe('splitAnchorCorrection', () => {
  // Layout lengths are multiples of 1/64 px in both engines, so sample those,
  // plus a spread of arbitrary doubles for the float edges.
  const samples = [
    0, 1, 2377, 2377.8, 2498.8, 0.015625, 911.984375, 12.5, 1e-9, 3 - 1e-9,
    ...Array.from({ length: 200 }, (_, i) => (i * 37.3) / 64),
  ];

  it('writes a whole pixel, keeps the spacer in [0, 1), and loses nothing', () => {
    for (const exact of samples) {
      const { scrollTop, spacer } = splitAnchorCorrection(exact);
      expect(Number.isInteger(scrollTop), `exact=${exact}`).toBe(true);
      expect(spacer, `exact=${exact}`).toBeGreaterThanOrEqual(0);
      expect(spacer, `exact=${exact}`).toBeLessThan(1);
      expect(scrollTop - spacer, `exact=${exact}`).toBeCloseTo(exact, 9);
    }
  });

  it('needs no spacer when the correction is already whole', () => {
    expect(splitAnchorCorrection(640)).toEqual({ scrollTop: 640, spacer: 0 });
  });
});

describe('the spacer on the container', () => {
  it('reads what it wrote, and reads nothing as zero', () => {
    const el = document.createElement('div');
    expect(anchorSpacer(el)).toBe(0);
    setAnchorSpacer(el, 0.328125);
    expect(anchorSpacer(el)).toBe(0.328125);
    setAnchorSpacer(el, 0);
    expect(anchorSpacer(el)).toBe(0);
  });
});
