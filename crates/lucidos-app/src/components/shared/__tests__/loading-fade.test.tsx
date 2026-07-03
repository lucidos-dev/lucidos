import { describe, it, expect, vi } from 'vitest';

// LoadingFade calls useLingeringFlag (a render-time hook) at its top level, so it
// can't be invoked as a plain function outside a real render. Stub the hook —
// the bug under test is purely structural (how `children` is placed in the grid),
// independent of the linger timing. `lingering` lets us drive whether the
// skeleton is mounted.
const mocks = vi.hoisted(() => ({ lingering: false }));
vi.mock('../../../hooks/useDelayedLoading', () => ({
  useLingeringFlag: () => mocks.lingering,
}));

import { LoadingFade } from '../LoadingFade';
import { vnodeToText } from '../../chat/__tests__/vnodeToText';

// A trivial stand-in skeleton — these tests assert only LoadingFade's structural
// placement (content wrapper vs skeleton sibling), not the skeleton's contents.
const testSkeleton = <div class="test-skeleton" />;

const fragmentChildren = (
  <>
    <div class="row-a" />
    <div class="row-b" />
    <div class="row-c" />
  </>
);

describe('LoadingFade', () => {
  it('wraps a multi-child fragment in ONE content element so it does not flatten into many grid children', () => {
    mocks.lingering = false;
    const text = vnodeToText(
      LoadingFade({ showSkeleton: false, skeleton: testSkeleton, children: fragmentChildren }),
    );

    // The grid's immediate child must be the single content wrapper — NOT the
    // fragment's rows. `.loading-fade > * { grid-area: 1/1 }` stacks every direct
    // child into one cell, so N direct rows would overlap (the Changes-panel
    // double-text bug). Exactly one wrapper must exist.
    expect(text).toContain('class="loading-fade"><div class="loading-fade-content">');
    expect((text.match(/loading-fade-content/g) ?? []).length).toBe(1);

    // All rows live INSIDE the wrapper, and none is a direct grid child.
    expect(text).toContain('class="row-a"');
    expect(text).toContain('class="row-b"');
    expect(text).toContain('class="row-c"');
    expect(text).not.toContain('class="loading-fade"><div class="row-a"');
  });

  it('renders the skeleton as a SIBLING of the content wrapper while mounted (the two stacking grid children)', () => {
    mocks.lingering = true;
    const text = vnodeToText(
      LoadingFade({ showSkeleton: true, skeleton: testSkeleton, children: fragmentChildren }),
    );

    // Content wrapper and skeleton are both direct children of .loading-fade, so
    // they share the stacking cell and crossfade against each other.
    expect(text).toContain('class="loading-fade-content">');
    expect(text).toContain('class="loading-fade-skeleton">');
    // The skeleton wrapper closes the content wrapper first, then opens — i.e.
    // they're siblings, not nested.
    expect(text).toMatch(/loading-fade-content">.*<\/div><div class="loading-fade-skeleton">/s);
  });
});
