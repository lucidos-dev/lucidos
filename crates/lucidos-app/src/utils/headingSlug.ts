/** What a rendered markdown document puts in front of each heading slug, as
 *  GitHub does. The document renders into the host page, so a bare `tooltip`
 *  or `app` would duplicate one of the shell's own element ids. A `#slug` link
 *  still names the bare slug: `handlePreviewLinkClick` adds the prefix back. */
export const HEADING_ID_PREFIX = 'user-content-';

/** Every character github-slugger drops: anything but a letter, a mark, a
 *  number, a connector such as `_`, a hyphen or a space. */
const SLUG_DROPPED = /[^\p{L}\p{M}\p{N}\p{Pc} -]/gu;

/** A github-slugger: heading text in, GitHub's anchor slug out. One per
 *  document, because a repeat takes `-1`, `-2` against the slugs before it.
 *
 *  "Auth tier compatibility (API key vs subscription / OAuth)" becomes
 *  `auth-tier-compatibility-api-key-vs-subscription--oauth`: each space turns
 *  into a hyphen, so the space pair around the dropped `/` stays `--`. */
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
