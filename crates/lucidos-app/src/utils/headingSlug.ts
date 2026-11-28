export const HEADING_ID_PREFIX = 'user-content-';

const SLUG_DROPPED = /[^\p{L}\p{M}\p{N}\p{Pc} -]/gu;

export function createHeadingSlugger(): (text: string) => string {
  const repeats = new Map<string, number>();
  return (text) => {
    const base = text.toLowerCase().replace(SLUG_DROPPED, '').replace(/ /g, '-');
    let slug = base;
    while (repeats.has(slug)) {
      const n = (repeats.get(base) ?? 0) + 1;
      repeats.set(base, n);
      slug = `${base}-${n}`;
    }
    repeats.set(slug, 0);
    return slug;
  };
}
