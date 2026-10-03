import type { SearchResultItem } from '../../api/client';
import type { Loadable } from '../../store/types';
import { bestFirst, NO_TITLE_MATCH, titleRank, type TitleRank } from './titleMatch';

/** Hits per section in the All tab. */
export const ALL_TAB_LIMIT = 5;

/** Hits asked of each category for the tab counts and the All tab. The one
 *  past `ALL_TAB_LIMIT` is what tells "5" from "5+". */
export const OVERVIEW_LIMIT = ALL_TAB_LIMIT + 1;

/** The All tab's order among sections whose best hits rank the same. `menu`
 *  last, because a search is nearly always for a thing rather than for the page
 *  it lives on. */
export const SECTION_ORDER = ['apps', 'files', 'settings', 'threads', 'triggers', 'changes', 'menu'];

export interface Section {
  section: string;
  items: SearchResultItem[];
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
 *  it, whichever category answered first. */
export function rankedSections(results: Record<string, SearchResultItem[]>, query: string): Section[] {
  const ranked = SECTION_ORDER
    .filter(section => results[section]?.length)
    .map(section => {
      const items = results[section].slice(0, ALL_TAB_LIMIT);
      return { section, items, rank: bestRank(items, query) };
    })
    // Stable, so SECTION_ORDER breaks every tie.
    .sort((a, b) => bestFirst(a.rank, b.rank));
  let offset = 0;
  return ranked.map(({ section, items }) => {
    const placed = { section, items, offset };
    offset += items.length;
    return placed;
  });
}

/** What a category tab says about its hits. `unknown` while the category is
 *  still out or failed, so neither reads as "no hits". */
export type TabHits =
  | { kind: 'unknown' }
  | { kind: 'none' }
  | { kind: 'some'; count: string };

export function tabHits(state: Loadable<SearchResultItem[]>): TabHits {
  if (state.status !== 'loaded') return { kind: 'unknown' };
  const n = state.data.length;
  if (n === 0) return { kind: 'none' };
  return { kind: 'some', count: n > ALL_TAB_LIMIT ? `${ALL_TAB_LIMIT}+` : String(n) };
}

/** The count and the dimming are visual, so a screen reader hears them here. */
export function tabAccessibleName(label: string, hits: TabHits): string | undefined {
  if (hits.kind === 'none') return `${label}, no hits`;
  if (hits.kind === 'some') return `${label}, ${hits.count} ${hits.count === '1' ? 'hit' : 'hits'}`;
  return undefined;
}
