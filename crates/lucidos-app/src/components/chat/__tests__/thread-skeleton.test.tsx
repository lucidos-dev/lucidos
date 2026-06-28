import { describe, it, expect } from 'vitest';
import { ThreadSkeleton } from '../ThreadSkeleton';
import { vnodeToText } from './vnodeToText';

describe('ThreadSkeleton', () => {
  it('renders the skeleton container with placeholder exchanges', () => {
    const text = vnodeToText(ThreadSkeleton());
    expect(text).toContain('class="thread-skeleton"');
    // Three placeholder exchanges, each with a user block + shimmer lines.
    expect((text.match(/thread-skeleton-exchange/g) ?? []).length).toBe(3);
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
