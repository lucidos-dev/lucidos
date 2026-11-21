export const RECENTS_KEY = 'lucidos-search-recents';

/** Drop one Search Everywhere recent, for an entity the source of truth deleted. */
export function pruneRecents(id: string, category: string): void {
  try {
    const raw = localStorage.getItem(RECENTS_KEY);
    if (!raw) return;
    const recents: Array<{ id: string; category: string }> = JSON.parse(raw);
    const filtered = recents.filter(r => !(r.id === id && r.category === category));
    if (filtered.length < recents.length) {
      localStorage.setItem(RECENTS_KEY, JSON.stringify(filtered));
    }
  } catch { /* corrupted localStorage */ }
}

