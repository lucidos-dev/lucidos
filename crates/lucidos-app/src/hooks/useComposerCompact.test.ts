/** Which face set the composer row draws, as pure math (ADR 0576).
 *
 *  The hook around it reads the DOM, which jsdom does not lay out. What is
 *  worth pinning is the decision: the phone layout is always compact, and a
 *  desktop row is compact only under the threshold. */
import { describe, it, expect } from 'vitest';
import { COMPOSER_COMPACT_MAX_REM, isComposerCompact } from './useComposerCompact';

const REM = 16;
const THRESHOLD_PX = COMPOSER_COMPACT_MAX_REM * REM;

describe('the composer row picks its face set', () => {
  it('is compact on the phone layout, at any row width', () => {
    expect(isComposerCompact({ phone: true, rowWidthPx: 5000, remPx: REM })).toBe(true);
    expect(isComposerCompact({ phone: true, rowWidthPx: 100, remPx: REM })).toBe(true);
  });

  it('is compact on a desktop row narrower than the threshold', () => {
    expect(isComposerCompact({ phone: false, rowWidthPx: THRESHOLD_PX - 1, remPx: REM })).toBe(true);
  });

  it('is desktop at the threshold and above it', () => {
    expect(isComposerCompact({ phone: false, rowWidthPx: THRESHOLD_PX, remPx: REM })).toBe(false);
    expect(isComposerCompact({ phone: false, rowWidthPx: THRESHOLD_PX * 3, remPx: REM })).toBe(false);
  });

  it('keeps the threshold in step with the UI scale', () => {
    const bigRem = 20;
    expect(isComposerCompact({ phone: false, rowWidthPx: THRESHOLD_PX, remPx: bigRem })).toBe(true);
    expect(isComposerCompact({ phone: false, rowWidthPx: COMPOSER_COMPACT_MAX_REM * bigRem, remPx: bigRem })).toBe(false);
  });
});
