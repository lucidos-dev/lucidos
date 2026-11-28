// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from 'vitest';
import { threadHeaderActions } from '../ThreadHeaderActions';
import { searchEverywhereAnchor } from '../../../store/store';

function openSearchFrom(el: HTMLElement): void {
  const action = threadHeaderActions().find((a) => a.key === 'search-everywhere');
  action!.onClick!({ currentTarget: el } as unknown as MouseEvent);
}

describe('the thread header search action', () => {
  beforeEach(() => {
    searchEverywhereAnchor.value = null;
  });

  it('anchors the palette to its own header button', () => {
    const button = document.createElement('button');
    button.classList.add('icon-btn');
    openSearchFrom(button);
    expect(searchEverywhereAnchor.value).toBe(button);
  });

  it('leaves the palette unanchored when a menu row opens it', () => {
    const row = document.createElement('button');
    row.className = 'brand-menu-item';
    openSearchFrom(row);
    expect(searchEverywhereAnchor.value).toBeNull();
  });
});
