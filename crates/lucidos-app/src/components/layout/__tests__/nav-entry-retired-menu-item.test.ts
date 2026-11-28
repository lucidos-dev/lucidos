import { describe, it, expect } from 'vitest';
import type { NavEntry } from '../../../store/actions/navigation';
import { navEntryTitle } from '../headerHelpers';

describe('navEntryTitle for a retired menu item', () => {
  it('names a persisted menu item this build no longer has, rather than failing', () => {
    const retired = { menuItem: 'retired-menu-item' } as unknown as NavEntry;
    expect(navEntryTitle(retired)).toBe('retired-menu-item');
  });
});
