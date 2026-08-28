import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import { MENU_ITEMS } from '../../store/types';
import { searchResultDestinationPane, searchResultIconCategory } from './searchEverywhereActions';

describe('searchResultDestinationPane', () => {
  it('routes thread results to the conversation (thread) pane', () => {
    expect(searchResultDestinationPane('threads')).toBe('thread');
  });

  it('routes every other category to the content pane', () => {
    for (const category of ['apps', 'files', 'settings', 'triggers', 'changes', 'menu']) {
      expect(searchResultDestinationPane(category)).toBe('content');
    }
  });
});

describe('searchResultIconCategory', () => {
  it('marks an ordinary result by its category', () => {
    expect(searchResultIconCategory({ category: 'apps', id: 'habit-tracker' })).toBe('apps');
    expect(searchResultIconCategory({ category: 'settings', id: 'models:chat' })).toBe('settings');
  });

  it('marks a menu result by the page it opens, not by "menu"', () => {
    // A destination wears the mark of where it lands. Reading the category
    // would draw one glyph for all seven pages. `CategoryIcon` has no `menu`
    // arm, so that glyph would be the fallback circle.
    expect(searchResultIconCategory({ category: 'menu', id: 'plugins' })).toBe('plugins');
    expect(searchResultIconCategory({ category: 'menu', id: 'settings' })).toBe('settings');
  });
});

/**
 * `searchResultIconCategory` hands `CategoryIcon` a menu item's id and trusts
 * it to be a glyph key. A menu item with no arm there draws the default circle,
 * which reads as "unknown kind" beside six pages that each show their own mark.
 *
 * A source scan rather than a rendered comparison: what is being pinned is that
 * the arm is WRITTEN, and two arms drawing the same shape render identically.
 */
describe('CategoryIcon marks every menu item', () => {
  const src: string = readFileSync(new URL('../shared/CategoryIcon.tsx', import.meta.url), 'utf8');

  it('carries an arm per menu item', () => {
    for (const id of MENU_ITEMS) {
      expect(src, `CategoryIcon has no \`case '${id}'\` arm`).toContain(`case '${id}':`);
    }
  });
});
