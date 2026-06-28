import { describe, it, expect } from 'vitest';
import { ListSkeleton } from '../ListSkeleton';
import { vnodeToText } from '../../chat/__tests__/vnodeToText';

describe('ListSkeleton', () => {
  it('renders the default number of shimmer rows', () => {
    const text = vnodeToText(ListSkeleton({}));
    expect(text).toContain('class="list-skeleton"');
    expect((text.match(/list-skeleton-row/g) ?? []).length).toBe(5);
  });

  it('honors a custom row count', () => {
    const text = vnodeToText(ListSkeleton({ rows: 3 }));
    expect((text.match(/list-skeleton-row/g) ?? []).length).toBe(3);
  });
});
