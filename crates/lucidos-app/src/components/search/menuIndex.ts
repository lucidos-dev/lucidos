import { MENU_ITEMS, MENU_ITEM_LABELS, type MenuItem } from '../../store/types';
import type { SearchResultItem } from '../../api/client';
import { rankByTitle } from './titleMatch';

interface MenuSearchEntry {
  id: MenuItem;
  subtitle: string;
  keywords: string;
}

const MENU_SEARCH_DETAIL: Record<MenuItem, Omit<MenuSearchEntry, 'id'>> = {
  files: { subtitle: 'Your workspace files', keywords: 'files artifacts documents folder browse workspace' },
  apps: { subtitle: 'Your apps', keywords: 'apps app ui pinned open' },
  plugins: { subtitle: 'Installed plugins and marketplaces', keywords: 'plugins marketplace catalog install update extension' },
  triggers: { subtitle: 'Scheduled and event triggers', keywords: 'triggers schedule cron automation recurring webhook fire' },
  settings: { subtitle: 'Every setting', keywords: 'settings preferences configuration options' },
  changes: { subtitle: 'Pending and applied changes', keywords: 'changes diff apply revert pending review branch code' },
  notifications: { subtitle: 'Your notifications', keywords: 'notifications alerts unread bell push' },
};

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

export function getMenuSearchResults(query: string, limit: number): SearchResultItem[] {
  const q = query.trim().toLowerCase();
  const matches = q
    ? rankByTitle(
        MENU_SEARCH_INDEX.filter(e => `${MENU_ITEM_LABELS[e.id]} ${e.keywords}`.toLowerCase().includes(q)),
        q,
        e => MENU_ITEM_LABELS[e.id],
      )
    : MENU_SEARCH_INDEX;
  return matches.slice(0, limit).map(toResult);
}

export function findMenuSearchEntry(id: string): MenuSearchEntry | undefined {
  return MENU_SEARCH_INDEX.find(e => e.id === id);
}
