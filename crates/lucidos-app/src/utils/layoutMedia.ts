/** Which layout a viewport gets: the phone layout (one swipe pane at a time) or
 *  the desktop split. The single definition, shared by JS and CSS.
 *
 *  A viewport gets the phone layout when it is narrow, or when it is a phone in
 *  landscape: short, and driven by a finger. A tablet is never that short, and
 *  a short desktop window or a touchscreen laptop has a fine primary pointer.
 *  Why landscape keeps the phone layout: docs/adr/0342.
 *
 *  CSS writes `@media (--phone-layout)` or `@media (--desktop-layout)`, and
 *  `expandLayoutMedia` (a PostCSS plugin in vite.config.ts) swaps in the
 *  queries below. JS reads `isPhoneLayout`. Both are built from the same two
 *  numbers, so the mounted layout and the CSS it gets cannot disagree. */

export const PHONE_MAX_WIDTH_PX = 768;
export const PHONE_LANDSCAPE_MAX_HEIGHT_PX = 500;

const NARROW = `(max-width: ${PHONE_MAX_WIDTH_PX}px)`;
const WIDE = `(min-width: ${PHONE_MAX_WIDTH_PX + 1}px)`;

/** Narrow, or short and driven by a finger. */
export const PHONE_LAYOUT_QUERY =
  `${NARROW}, (max-height: ${PHONE_LANDSCAPE_MAX_HEIGHT_PX}px) and (pointer: coarse)`;

/** The exact complement of `PHONE_LAYOUT_QUERY`: wide, and tall or not a finger.
 *  Spelled out per pointer value so no browser has to parse a nested `not`. */
export const DESKTOP_LAYOUT_QUERY = [
  `${WIDE} and (min-height: ${PHONE_LANDSCAPE_MAX_HEIGHT_PX + 1}px)`,
  `${WIDE} and (pointer: fine)`,
  `${WIDE} and (pointer: none)`,
].join(', ');

export const LAYOUT_MEDIA: Readonly<Record<string, string>> = {
  '(--phone-layout)': PHONE_LAYOUT_QUERY,
  '(--desktop-layout)': DESKTOP_LAYOUT_QUERY,
};

/** The `@media` params with a layout name swapped for its query. Throws on a
 *  layout name combined with anything else, since `and` cannot distribute
 *  over a query list. */
export function expandLayoutMedia(params: string): string {
  const trimmed = params.trim();
  const query = LAYOUT_MEDIA[trimmed];
  if (query) return query;
  if (/\(--(phone|desktop)-layout\)/.test(trimmed)) {
    throw new Error(`@media ${trimmed}: a layout query must stand alone`);
  }
  return params;
}

/** The JS side of `PHONE_LAYOUT_QUERY`, from live viewport metrics. */
export function isPhoneLayout(viewport: { width: number; height: number; coarsePointer: boolean }): boolean {
  return viewport.width <= PHONE_MAX_WIDTH_PX
    || (viewport.height <= PHONE_LANDSCAPE_MAX_HEIGHT_PX && viewport.coarsePointer);
}
