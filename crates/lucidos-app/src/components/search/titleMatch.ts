
export const TITLE_MATCH_LEVELS = ['none', 'phrase', 'word-start', 'exact'] as const;
export type TitleMatch = typeof TITLE_MATCH_LEVELS[number];

export interface TitleRank {
  level: TitleMatch;
  coverage: number;
}

export const NO_TITLE_MATCH: TitleRank = { level: 'none', coverage: 0 };

function normalize(s: string): string {
  return s.split(/\p{White_Space}+/u).filter(Boolean).join(' ').toLowerCase();
}

function charCount(s: string): number {
  return [...s].length;
}

function startsAWord(title: string, at: number): boolean {
  const before = [...title.slice(0, at)].pop();
  return before === undefined || !/[\p{Alphabetic}\p{N}]/u.test(before);
}

function level(title: string, query: string): TitleMatch {
  if (!query) return 'none';
  if (title === query) return 'exact';
  let found: TitleMatch = 'none';
  for (let at = title.indexOf(query); at >= 0; at = title.indexOf(query, at + query.length)) {
    if (startsAWord(title, at)) return 'word-start';
    found = 'phrase';
  }
  return found;
}

export function titleRank(title: string, query: string): TitleRank {
  const t = normalize(title);
  const q = normalize(query);
  const l = level(t, q);
  return { level: l, coverage: l === 'none' ? 0 : charCount(q) / charCount(t) };
}

export function bestFirst(a: TitleRank, b: TitleRank): number {
  return TITLE_MATCH_LEVELS.indexOf(b.level) - TITLE_MATCH_LEVELS.indexOf(a.level)
    || b.coverage - a.coverage;
}

export function rankByTitle<T>(items: T[], query: string, titleOf: (item: T) => string): T[] {
  return items
    .map(item => ({ item, rank: titleRank(titleOf(item), query) }))
    .sort((a, b) => bestFirst(a.rank, b.rank))
    .map(({ item }) => item);
}
