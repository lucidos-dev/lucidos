// Pins the Archive-collapsed pagination guard. Without it, collapsing Archive
// shrinks the list, pulls the infinite-scroll sentinel into view, and fires
// loadOlderThreads — silently bloating the count badge on every toggle.
//
// The guard yields to an active filter: when the user has narrowed to a
// trigger/repo/app whose matches are all archived, those threads can ONLY
// land in the Archive section, so blocking pagination there would strand the
// user on "No threads" forever. `filterActive` overrides the collapse block.

import { describe, it, expect } from 'vitest';
import { shouldLoadOlderOnIntersection } from './ThreadDrawer';

describe('infinite-scroll archive-collapsed guard', () => {
  it('allows pagination when nothing is collapsed', () => {
    expect(shouldLoadOlderOnIntersection(new Set(), false)).toBe(true);
  });

  it('allows pagination when only non-archive sections are collapsed', () => {
    expect(shouldLoadOlderOnIntersection(new Set(['active', 'review', 'new', 'saved']), false)).toBe(true);
  });

  it('blocks pagination when archive is collapsed and no filter is active', () => {
    expect(shouldLoadOlderOnIntersection(new Set(['archive']), false)).toBe(false);
  });

  it('blocks pagination when archive is collapsed alongside other sections (no filter)', () => {
    expect(shouldLoadOlderOnIntersection(new Set(['archive', 'review']), false)).toBe(false);
  });

  it('allows pagination when a filter is active even though archive is collapsed', () => {
    // A repo/app/trigger filter whose matches are all archived: the user
    // explicitly asked for those threads, so the collapse guard must yield.
    expect(shouldLoadOlderOnIntersection(new Set(['archive']), true)).toBe(true);
  });

  it('allows pagination when a filter is active and nothing is collapsed', () => {
    expect(shouldLoadOlderOnIntersection(new Set(), true)).toBe(true);
  });
});
