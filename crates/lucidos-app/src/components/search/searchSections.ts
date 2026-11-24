import type { SearchResultItem, TextSearchHit } from '../../api/client';
import type { Loadable } from '../../store/types';
import { bestFirst, NO_TITLE_MATCH, titleRank, type TitleRank } from './titleMatch';

export const ALL_TAB_LIMIT = 5;

export const SECTION_ORDER = ['apps', 'files', 'settings', 'threads', 'triggers', 'changes', 'menu'];

export interface TextHitItem {
  category: 'text';
  id: string;
  hit: TextSearchHit;
}

export type PaletteItem = SearchResultItem | TextHitItem;

export function isTextHit(item: PaletteItem): item is TextHitItem {
  return 'hit' in item;
}

export function textHitItem(hit: TextSearchHit): TextHitItem {
  return { category: 'text', id: `${hit.path}:${hit.line}`, hit };
}

export interface Section {
  section: string;
  items: PaletteItem[];
  offset: number;
}

function bestRank(items: SearchResultItem[], query: string): TitleRank {
  return items
    .map(item => titleRank(item.title, query))
    .sort(bestFirst)[0] ?? NO_TITLE_MATCH;
}

export function rankedSections(
  results: Record<string, SearchResultItem[]>,
  query: string,
  textHits: TextHitItem[] = [],
  drawn: readonly string[] = [],
): Section[] {
  const drawnAt = (section: string) => {
    const at = drawn.indexOf(section);
    return at < 0 ? drawn.length : at;
  };
  const ranked: { section: string; items: PaletteItem[] }[] = SECTION_ORDER
    .filter(section => results[section]?.length)
    .map(section => {
      const items = results[section].slice(0, ALL_TAB_LIMIT);
      return { section, items, rank: bestRank(items, query) };
    })
    .sort((a, b) => drawnAt(a.section) - drawnAt(b.section) || bestFirst(a.rank, b.rank));
  if (textHits.length) ranked.push({ section: 'text', items: textHits });
  let offset = 0;
  return ranked.map(({ section, items }) => {
    const placed = { section, items, offset };
    offset += items.length;
    return placed;
  });
}

export type TabHits = 'unknown' | 'none' | 'some';

export function tabHits(state: Loadable<unknown[]>): TabHits {
  if (state.status !== 'loaded') return 'unknown';
  return state.data.length === 0 ? 'none' : 'some';
}

export function tabAccessibleName(label: string, hits: TabHits): string | undefined {
  return hits === 'none' ? `${label}, no hits` : undefined;
}
