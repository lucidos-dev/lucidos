import { describe, it, expect } from 'vitest';
import { computeFitsInOneRow } from './useFitsInOneRow';

// Pinning the actual width math the prompt uses to decide whether to lift the
// secondary button (the Diff button — in the banner or standalone while
// composing) to a row above. Each scenario walks a real-ish set of button
// widths the bottom row of PromptInput would carry on a phone screen.
describe('computeFitsInOneRow', () => {
  // 0.5rem at the default 16px root = 8px between items.
  const gap = 8;

  it('an empty row trivially fits', () => {
    expect(computeFitsInOneRow([], 320, gap)).toBe(true);
  });

  it('a single item fits when it is narrower than the container', () => {
    expect(computeFitsInOneRow([100], 320, gap)).toBe(true);
  });

  it('a single item that is wider than the container does not fit', () => {
    expect(computeFitsInOneRow([400], 320, gap)).toBe(false);
  });

  // [icons 36+36] [Save 70] [Diff 56] [Discard 84] [Apply 64] = 346px content
  // + 5 gaps of 8px = 386px total. A 393pt iPhone Pro at default zoom has
  // ~330px of usable inside the prompt-actions-area (after 0.5rem + 0.75rem
  // padding) — does NOT fit, must lift.
  it('the full banner row does not fit a phone-width container', () => {
    expect(computeFitsInOneRow([36, 36, 70, 56, 84, 64], 330, gap)).toBe(false);
  });

  // Same items in a desktop-width container clearly fit.
  it('the full banner row fits a desktop-width container', () => {
    expect(computeFitsInOneRow([36, 36, 70, 56, 84, 64], 800, gap)).toBe(true);
  });

  // [icons 36+36] [secondary 110] [Save 70] [Send 60] = 312px content
  // + 4 gaps of 8px = 344px. Still over 330px on a phone — must lift the
  // secondary button.
  it('a dense bottom row (icons + a wide secondary + Save + Send) overflows a phone', () => {
    expect(computeFitsInOneRow([36, 36, 110, 70, 60], 330, gap)).toBe(false);
  });

  // After lifting the secondary button, the bottom row is just icons + Save +
  // Send. The hook still measures every [data-row-item] (the lifted button is a
  // data-row-item too, just in a sibling row), so the sum is unchanged —
  // stays "does not fit" and stays stacked. Loop avoided.
  it('still reports "does not fit" with the same item set after lifting (stable across re-render)', () => {
    const widths = [36, 36, 110, 70, 60];
    expect(computeFitsInOneRow(widths, 330, gap)).toBe(false);
    // Same widths, container hasn't grown — same answer.
    expect(computeFitsInOneRow(widths, 330, gap)).toBe(false);
  });

  // Sub-pixel rounding tolerance: a sum that is 0.4px over still counts as
  // fitting (browsers round and a 0.5px epsilon prevents flicker).
  it('tolerates sub-pixel rounding within 0.5px', () => {
    expect(computeFitsInOneRow([100.4], 100, 0)).toBe(true);
    expect(computeFitsInOneRow([100.6], 100, 0)).toBe(false);
  });

  // Larger root font size (user accessibility setting) → larger gap → row
  // overflows where it would have fit at 16px. Caller passes the scaled
  // gap in, so the math just sees a larger gap.
  it('honors a larger gap when the user has scaled their font size up', () => {
    expect(computeFitsInOneRow([100, 100, 100], 320, 8)).toBe(true);
    expect(computeFitsInOneRow([100, 100, 100], 320, 16)).toBe(false);
  });
});
