// Pins the Archive-collapsed pagination guard. Without it, collapsing Archive
// shrinks the list, pulls the infinite-scroll sentinel into view, and fires
// loadOlderThreads — silently bloating the count badge on every toggle.

import { describe, it, expect } from 'vitest';
import { shouldLoadOlderOnIntersection } from './ThreadDrawer';

describe('infinite-scroll archive-collapsed guard', () => {
  it('allows pagination when nothing is collapsed', () => {
    expect(shouldLoadOlderOnIntersection(new Set())).toBe(true);
  });

  it('allows pagination when only non-archive sections are collapsed', () => {
    expect(shouldLoadOlderOnIntersection(new Set(['active', 'review', 'new', 'saved']))).toBe(true);
  });

  it('blocks pagination when archive is collapsed', () => {
    expect(shouldLoadOlderOnIntersection(new Set(['archive']))).toBe(false);
  });

  it('blocks pagination when archive is collapsed alongside other sections', () => {
    expect(shouldLoadOlderOnIntersection(new Set(['archive', 'review']))).toBe(false);
  });
});
