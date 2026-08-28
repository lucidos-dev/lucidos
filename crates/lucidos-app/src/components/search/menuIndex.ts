import { MENU_ITEMS, MENU_ITEM_LABELS, type MenuItem } from '../../store/types';
import type { SearchResultItem } from '../../api/client';

/**
 * The menu items themselves, as Search Everywhere results.
 *
 * Sibling of `searchIndex.ts` (the Settings index) and resolved the same way:
 * entirely in the frontend, so the engine never sees the `menu` category.
 *
 * The two differ in one respect. A settings entry carries visibility flags,
 * because the row it lands on may not render. Every menu item always opens a
 * real content-pane view through `switchMenuItem`, so no result here can land
 * on nothing and none of them is gated.
 *
 * Names come from `MENU_ITEM_LABELS`, which the drawer and the header read too.
 * So a page cannot be called one thing in search and another where you land.
 */
interface MenuSearchEntry {
  /** The menu item to switch to, and the result id (also the recents key). */
  id: MenuItem;
  /** What the page holds, shown as the result subtitle. */
  subtitle: string;
  /** Extra free-text matched alongside the label. Not shown in the UI. */
  keywords: string;
}

/** Keyed by `MenuItem`, so a new page cannot ship unfindable. */
const MENU_SEARCH_DETAIL: Record<MenuItem, Omit<MenuSearchEntry, 'id'>> = {
  files: { subtitle: 'Your workspace files', keywords: 'files artifacts documents folder browse workspace' },
  apps: { subtitle: 'Your apps', keywords: 'apps app ui pinned open' },
  plugins: { subtitle: 'Installed plugins and marketplaces', keywords: 'plugins marketplace catalog install update extension' },
  triggers: { subtitle: 'Scheduled and event triggers', keywords: 'triggers schedule cron automation recurring webhook fire' },
  settings: { subtitle: 'Every setting', keywords: 'settings preferences configuration options' },
  changes: { subtitle: 'Pending and applied changes', keywords: 'changes diff apply revert pending review branch code' },
  notifications: { subtitle: 'Your notifications', keywords: 'notifications alerts unread bell push' },
};

/** In the order `MENU_ITEMS` declares, which is the order the Menu tab lists. */
const MENU_SEARCH_INDEX: MenuSearchEntry[] =
  MENU_ITEMS.map(id => ({ id, ...MENU_SEARCH_DETAIL[id] }));

function toResult(entry: MenuSearchEntry): SearchResultItem {
  return {
    id: entry.id,
    title: MENU_ITEM_LABELS[entry.id],
    subtitle: entry.subtitle,
    category: 'menu',
    score: 1.0,
  };
}

/** Filter the index by query (case-insensitive substring over label plus
 *  keywords) and return as SearchResultItems. An empty query lists every menu
 *  item, which is what the Menu tab opens on. */
export function getMenuSearchResults(query: string, limit: number): SearchResultItem[] {
  const q = query.trim().toLowerCase();
  const matches = q
    ? MENU_SEARCH_INDEX.filter(e => `${MENU_ITEM_LABELS[e.id]} ${e.keywords}`.toLowerCase().includes(q))
    : MENU_SEARCH_INDEX;
  return matches.slice(0, limit).map(toResult);
}

export function findMenuSearchEntry(id: string): MenuSearchEntry | undefined {
  return MENU_SEARCH_INDEX.find(e => e.id === id);
}
