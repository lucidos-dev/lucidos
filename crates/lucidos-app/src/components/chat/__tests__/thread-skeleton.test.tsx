import { describe, it, expect } from 'vitest';
import { skeletonOverlayClass } from '../ThreadView';
import { ThreadSkeleton, THREAD_SKELETON_EXCHANGES } from '../ThreadSkeleton';
import { vnodeToText } from './vnodeToText';

describe('skeletonOverlayClass', () => {
  it('is opaque while shown, so the loader the gate asked for is legible', () => {
    expect(skeletonOverlayClass(true)).toBe('thread-skeleton-overlay');
  });

  it('fades out to reveal the content underneath', () => {
    expect(skeletonOverlayClass(false)).toBe('thread-skeleton-overlay is-fading');
  });
});

describe('ThreadSkeleton', () => {
  it('renders the skeleton container with placeholder exchanges', () => {
    const text = vnodeToText(ThreadSkeleton());
    expect(text).toContain('class="thread-skeleton"');
    expect((text.match(/thread-skeleton-exchange/g) ?? []).length).toBe(THREAD_SKELETON_EXCHANGES);
    expect(text).toContain('thread-skeleton-user');
    expect(text).toContain('thread-skeleton-line');
    expect(text).toContain('thread-skeleton-line-short');
  });

  it('uses the shared shimmer block class on its bars', () => {
    const text = vnodeToText(ThreadSkeleton());
    expect((text.match(/thread-skeleton-block/g) ?? []).length).toBeGreaterThanOrEqual(3);
  });
});
