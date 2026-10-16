import { describe, it, expect } from 'vitest';
import { MENU_ITEMS, MENU_ITEM_LABELS } from '../../store/types';
import { getMenuSearchResults, findMenuSearchEntry } from './menuIndex';

describe('menu search: every page is findable', () => {
  it('finds each menu item by the name the UI gives it', () => {
    for (const id of MENU_ITEMS) {
      const hits = getMenuSearchResults(MENU_ITEM_LABELS[id], 20);
      expect(hits.some(r => r.id === id), `"${MENU_ITEM_LABELS[id]}" finds nothing`).toBe(true);
    }
  });

  it('lists every page for an empty query, in MENU_ITEMS order', () => {
    // The Menu tab opens on this, so it doubles as "where can I go".
    expect(getMenuSearchResults('', 50).map(r => r.id)).toEqual([...MENU_ITEMS]);
  });

  it('matches keywords the label does not carry', () => {
    expect(getMenuSearchResults('cron', 20).map(r => r.id)).toContain('triggers');
    expect(getMenuSearchResults('marketplace', 20).map(r => r.id)).toContain('plugins');
    expect(getMenuSearchResults('unread', 20).map(r => r.id)).toContain('notifications');
    expect(getMenuSearchResults('artifacts', 20).map(r => r.id)).toContain('files');
    expect(getMenuSearchResults('diff', 20).map(r => r.id)).toContain('changes');
  });

  it('ignores case and surrounding space', () => {
    expect(getMenuSearchResults('  PLUG ', 20).map(r => r.id)).toContain('plugins');
  });

  it('returns nothing for a query no page answers', () => {
    expect(getMenuSearchResults('zzzznope', 20)).toEqual([]);
  });

  it('honours the limit, which the All tab caps at five', () => {
    expect(getMenuSearchResults('', 3)).toHaveLength(3);
  });
});

describe('menu search: the row a result renders', () => {
  it('takes its title from the one label map, so search and destination agree', () => {
    const apps = getMenuSearchResults('apps', 20).find(r => r.id === 'apps');
    expect(apps?.title).toBe(MENU_ITEM_LABELS.apps);
  });

  it('marks every row with the menu category and a subtitle', () => {
    for (const row of getMenuSearchResults('', 50)) {
      expect(row.category).toBe('menu');
      expect(row.subtitle.trim(), `${row.id} has no subtitle`).not.toBe('');
    }
  });
});

describe('findMenuSearchEntry', () => {
  it('resolves every menu item', () => {
    for (const id of MENU_ITEMS) {
      expect(findMenuSearchEntry(id)?.id, `${id} does not resolve`).toBe(id);
    }
  });

  it('resolves nothing for a retired id, so a stale recents row is dropped', () => {
    // `validateRecents` reads this: recents are persisted verbatim, so an id a
    // later build no longer knows would close the palette and go nowhere.
    expect(findMenuSearchEntry('pinned')).toBeUndefined();
  });
});
