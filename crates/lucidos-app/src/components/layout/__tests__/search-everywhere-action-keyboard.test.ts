// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';

const openWhenFocused: boolean[] = [];
vi.mock('../../search/searchEverywhereActions', async () => {
  const { searchEverywhereOpen } = await import('../../../store/store');
  return { focusSearchInput: vi.fn(() => { openWhenFocused.push(searchEverywhereOpen.value); }) };
});

import { searchEverywhereOpen } from '../../../store/store';
import { threadHeaderActions } from '../ThreadHeaderActions';

function pressSearch(): void {
  const action = threadHeaderActions().find((a) => a.key === 'search-everywhere');
  const button = document.createElement('button');
  button.className = 'icon-btn';
  action!.onClick!({ currentTarget: button } as unknown as MouseEvent);
}

// focusSearchInput does nothing once the palette is open. So it must run
// while the palette is still closed, or a tap never raises the keyboard.
describe('the Search everywhere header action', () => {
  beforeEach(() => {
    openWhenFocused.length = 0;
    searchEverywhereOpen.value = false;
  });

  it('holds the keyboard before it opens the palette', () => {
    pressSearch();
    expect(searchEverywhereOpen.value).toBe(true);
    expect(openWhenFocused).toEqual([false]);
  });
});
