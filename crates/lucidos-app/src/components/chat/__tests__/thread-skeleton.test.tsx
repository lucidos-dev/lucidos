import { describe, it, expect } from 'vitest';
import { ThreadSkeleton, THREAD_SKELETON_EXCHANGES } from '../ThreadSkeleton';
import { vnodeToText } from './vnodeToText';

describe('ThreadSkeleton', () => {
  it('renders the skeleton container with placeholder exchanges', () => {
    const text = vnodeToText(ThreadSkeleton());
    expect(text).toContain('class="thread-skeleton"');
    // A generous run of placeholder exchanges (each a user block + shimmer
    // lines) so the skeleton fills the full content height; the overlay clips
    // the excess (see THREAD_SKELETON_EXCHANGES).
    expect((text.match(/thread-skeleton-exchange/g) ?? []).length).toBe(THREAD_SKELETON_EXCHANGES);
    expect(text).toContain('thread-skeleton-user');
    expect(text).toContain('thread-skeleton-line');
    expect(text).toContain('thread-skeleton-line-short');
  });

  it('uses the shared shimmer block class on its bars', () => {
    const text = vnodeToText(ThreadSkeleton());
    // Every shimmering bar carries thread-skeleton-block (the gradient + shimmer
    // animation); without it the placeholder would render as flat boxes.
    expect((text.match(/thread-skeleton-block/g) ?? []).length).toBeGreaterThanOrEqual(3);
  });
});
