import { describe, it, expect, vi } from 'vitest';

const mocks = vi.hoisted(() => ({ lingering: false }));
vi.mock('../../../hooks/useDelayedLoading', () => ({
  useLingeringFlag: () => mocks.lingering,
  useMinimumVisibleFlag: (active: boolean) => active,
}));

import { LoadingFade } from '../LoadingFade';
import { vnodeToText } from '../../chat/__tests__/vnodeToText';

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

    expect(text).toContain('class="loading-fade"><div class="loading-fade-content">');
    expect((text.match(/loading-fade-content/g) ?? []).length).toBe(1);

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

    expect(text).toContain('class="loading-fade-content">');
    expect(text).toContain('class="loading-fade-skeleton">');
    expect(text).toMatch(/loading-fade-content">.*<\/div><div class="loading-fade-skeleton">/s);
  });
});
