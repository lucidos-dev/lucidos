/**
 * How well a search hit's title holds the query: the frontend twin of the
 * engine's `engine/title_match.rs`. The engine ranks the categories it answers;
 * this ranks Settings and Menu, and orders the All tab's sections.
 * `titleMatch.test.ts` holds the two to the generated `title-match-fixture.json`.
 */

/** Weakest first, matching the Rust enum's declaration order. */
export const TITLE_MATCH_LEVELS = ['none', 'phrase', 'word-start', 'exact'] as const;
export type TitleMatch = typeof TITLE_MATCH_LEVELS[number];

export interface TitleRank {
  level: TitleMatch;
  /** Query length over title length, in characters. 0 when `level` is `none`. */
  coverage: number;
}

/** The rank of a hit that has no title match, and of an empty section. */
export const NO_TITLE_MATCH: TitleRank = { level: 'none', coverage: 0 };

/** Splits on the Unicode White_Space property, as Rust's `split_whitespace` does. */
function normalize(s: string): string {
  return s.split(/\p{White_Space}+/u).filter(Boolean).join(' ').toLowerCase();
}

/** Counts code points, as Rust's `chars()` does, not UTF-16 units. */
function charCount(s: string): number {
  return [...s].length;
}

/** Alphanumeric as Rust's `char::is_alphanumeric` reads it. camelCase is
 *  deliberately not a word boundary. */
function startsAWord(title: string, at: number): boolean {
  const before = [...title.slice(0, at)].pop();
  return before === undefined || !/[\p{Alphabetic}\p{N}]/u.test(before);
}

function level(title: string, query: string): TitleMatch {
  if (!query) return 'none';
  if (title === query) return 'exact';
  let found: TitleMatch = 'none';
  // Non-overlapping, like Rust's `match_indices`.
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

/** Best first, for `Array.prototype.sort`. */
export function bestFirst(a: TitleRank, b: TitleRank): number {
  return TITLE_MATCH_LEVELS.indexOf(b.level) - TITLE_MATCH_LEVELS.indexOf(a.level)
    || b.coverage - a.coverage;
}

/** Sort hits best first by their title, keeping the given order among equals. */
export function rankByTitle<T>(items: T[], query: string, titleOf: (item: T) => string): T[] {
  return items
    .map(item => ({ item, rank: titleRank(titleOf(item), query) }))
    .sort((a, b) => bestFirst(a.rank, b.rank))
    .map(({ item }) => item);
}
