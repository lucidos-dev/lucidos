// Pins the Archive infinite-scroll primitives:
//
// - `archivePaginationAllowed` — the collapsed-Archive guard. Collapsing Archive
//   shrinks the list, pulls the sentinel into view, and would otherwise let the
//   fill loop pull the whole archive into memory for rows the user can't see.
//   There is NO filter-active bypass anymore: the badge reads a server-sourced
//   count (`refreshArchivedCount`), so a filter's matches are counted while
//   collapsed without eager-loading hidden rows.
//
// - `sentinelInView` — the pure rect overlap the fill loop polls after each page
//   to decide whether the freshly-loaded rows pushed the sentinel below the fold.
//   This is the fix for "pagination only advanced on collapse/expand": an
//   IntersectionObserver fires only on transitions, so a page that doesn't
//   refill the viewport leaves the sentinel intersecting with no new event.

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
    // Sentinel visible just below the last loaded row → keep loading.
    expect(sentinelInView({ top: 500, bottom: 532 }, root)).toBe(true);
  });

  it('is true when the sentinel straddles the bottom edge', () => {
    expect(sentinelInView({ top: 790, bottom: 822 }, root)).toBe(true);
  });

  it('is false when the sentinel is pushed below the fold (viewport filled)', () => {
    // Freshly-loaded rows pushed the sentinel out → stop; user scrolls to resume.
    expect(sentinelInView({ top: 900, bottom: 932 }, root)).toBe(false);
  });

  it('is false when the sentinel is scrolled above the viewport top', () => {
    expect(sentinelInView({ top: 40, bottom: 72 }, root)).toBe(false);
  });
});
