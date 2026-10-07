import type { SearchResultItem, TextSearchHit } from '../../api/client';
import type { Loadable } from '../../store/types';
import { bestFirst, NO_TITLE_MATCH, titleRank, type TitleRank } from './titleMatch';

/** Hits per section in the All tab, and so hits asked of each category. */
export const ALL_TAB_LIMIT = 5;

/** The All tab's order among sections whose best hits rank the same. `menu`
 *  last, because a search is nearly always for a thing rather than for the page
 *  it lives on. */
export const SECTION_ORDER = ['apps', 'files', 'settings', 'threads', 'triggers', 'changes', 'menu'];

/** A Text search line in the results list. Its `id` is `<path>:<line>`, so
 *  `category:id` keys it like every other row. */
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
  /** Index of the first item in the flattened list. */
  offset: number;
}

function bestRank(items: SearchResultItem[], query: string): TitleRank {
  return items
    .map(item => titleRank(item.title, query))
    .sort(bestFirst)[0] ?? NO_TITLE_MATCH;
}

/** The All tab's sections, each capped, ordered by the title rank of its best
 *  hit. So "settings" lists the Settings page above files that only mention
 *  it, whichever category answered first. Text lines come after every name
 *  section, whatever their rank (ADR 0383). */
export function rankedSections(
  results: Record<string, SearchResultItem[]>,
  query: string,
  textHits: TextHitItem[] = [],
): Section[] {
  const ranked: { section: string; items: PaletteItem[] }[] = SECTION_ORDER
    .filter(section => results[section]?.length)
    .map(section => {
      const items = results[section].slice(0, ALL_TAB_LIMIT);
      return { section, items, rank: bestRank(items, query) };
    })
    // Stable, so SECTION_ORDER breaks every tie.
    .sort((a, b) => bestFirst(a.rank, b.rank));
  // The engine's preview already holds the Text section's share of the tab.
  if (textHits.length) ranked.push({ section: 'text', items: textHits });
  let offset = 0;
  return ranked.map(({ section, items }) => {
    const placed = { section, items, offset };
    offset += items.length;
    return placed;
  });
}

/** What a category tab says about its hits. `unknown` while the category is
 *  still out or failed, so neither reads as "no hits". */
export type TabHits = 'unknown' | 'none' | 'some';

export function tabHits(state: Loadable<unknown[]>): TabHits {
  if (state.status !== 'loaded') return 'unknown';
  return state.data.length === 0 ? 'none' : 'some';
}

/** The dimming is visual, so a screen reader hears it here. */
export function tabAccessibleName(label: string, hits: TabHits): string | undefined {
  return hits === 'none' ? `${label}, no hits` : undefined;
}
