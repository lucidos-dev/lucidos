
import { describe, it, expect } from 'vitest';
import { archivePaginationAllowed, sentinelInView } from './ThreadDrawer';

describe('archivePaginationAllowed', () => {
  it('allows pagination when nothing is collapsed', () => {
    expect(archivePaginationAllowed(new Set())).toBe(true);
  });

  it('allows pagination when only non-archive sections are collapsed', () => {
    expect(archivePaginationAllowed(new Set(['current', 'saved']))).toBe(true);
  });

  it('blocks pagination when archive is collapsed', () => {
    expect(archivePaginationAllowed(new Set(['archive']))).toBe(false);
  });

  it('blocks pagination when archive is collapsed alongside other sections', () => {
    expect(archivePaginationAllowed(new Set(['archive', 'current']))).toBe(false);
  });
});

describe('sentinelInView', () => {
  const root = { top: 100, bottom: 800 };

  it('is true when the sentinel sits inside the viewport (page underfilled)', () => {
    expect(sentinelInView({ top: 500, bottom: 532 }, root)).toBe(true);
  });

  it('is true when the sentinel straddles the bottom edge', () => {
    expect(sentinelInView({ top: 790, bottom: 822 }, root)).toBe(true);
  });

  it('is false when the sentinel is pushed below the fold (viewport filled)', () => {
    expect(sentinelInView({ top: 900, bottom: 932 }, root)).toBe(false);
  });

  it('is false when the sentinel is scrolled above the viewport top', () => {
    expect(sentinelInView({ top: 40, bottom: 72 }, root)).toBe(false);
  });
});
