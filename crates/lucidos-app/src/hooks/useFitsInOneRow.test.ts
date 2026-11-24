import { describe, it, expect, afterEach } from 'vitest';
import { computeFitsInOneRow, contentWidthOf } from './useFitsInOneRow';

describe('computeFitsInOneRow', () => {
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

  it('the full banner row does not fit a phone-width container', () => {
    expect(computeFitsInOneRow([36, 36, 70, 56, 84, 64], 330, gap)).toBe(false);
  });

  it('the full banner row fits a desktop-width container', () => {
    expect(computeFitsInOneRow([36, 36, 70, 56, 84, 64], 800, gap)).toBe(true);
  });

  it('a dense bottom row (icons + a wide secondary + Save + Send) overflows a phone', () => {
    expect(computeFitsInOneRow([36, 36, 110, 70, 60], 330, gap)).toBe(false);
  });

  it('still reports "does not fit" with the same item set after lifting (stable across re-render)', () => {
    const widths = [36, 36, 110, 70, 60];
    expect(computeFitsInOneRow(widths, 330, gap)).toBe(false);
    expect(computeFitsInOneRow(widths, 330, gap)).toBe(false);
  });

  it('tolerates sub-pixel rounding within 0.5px', () => {
    expect(computeFitsInOneRow([100.4], 100, 0)).toBe(true);
    expect(computeFitsInOneRow([100.6], 100, 0)).toBe(false);
  });

  it('honors a larger gap when the user has scaled their font size up', () => {
    expect(computeFitsInOneRow([100, 100, 100], 320, 8)).toBe(true);
    expect(computeFitsInOneRow([100, 100, 100], 320, 16)).toBe(false);
  });

  it('charges only the gaps the caller says exist', () => {
    const widths = [40, 40, 40, 63, 98];
    expect(computeFitsInOneRow(widths, 300, 9, 1)).toBe(true);
    expect(computeFitsInOneRow(widths, 300, 9)).toBe(false);
  });

  it('charges nothing for a lone item, whatever the gap', () => {
    expect(computeFitsInOneRow([100], 100, 9, 0)).toBe(true);
  });
});

describe('contentWidthOf', () => {
  const realGetComputedStyle = globalThis.getComputedStyle;
  afterEach(() => { globalThis.getComputedStyle = realGetComputedStyle; });

  function padded(clientWidth: number, left: string, right: string): HTMLElement {
    globalThis.getComputedStyle = (() => (
      { paddingLeft: left, paddingRight: right }
    )) as unknown as typeof getComputedStyle;
    return { clientWidth } as HTMLElement;
  }

  it('subtracts the container\'s own horizontal padding', () => {
    expect(contentWidthOf(padded(347, '8px', '12px'))).toBe(327);
  });

  it('returns clientWidth when the container has no padding', () => {
    expect(contentWidthOf(padded(347, '0px', '0px'))).toBe(347);
  });

  it('reads an unresolvable padding as none rather than NaN', () => {
    expect(contentWidthOf(padded(347, '', ''))).toBe(347);
  });

  it('never goes negative', () => {
    expect(contentWidthOf(padded(10, '40px', '0px'))).toBe(0);
  });
});
