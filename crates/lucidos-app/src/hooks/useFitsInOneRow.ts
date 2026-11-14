/** Pure width math for a row that must fit on ONE line.
 *
 *  The hook that drove the composer's sub-row lift lived here and is gone with
 *  it: the row folds into its ⋯ menu instead, and never onto a second line.
 *  What survives is the arithmetic, which `usePromptActionCollapse` uses.
 *
 *  Sums each item's measured width plus `gapCount` gaps of `gapPx`, then
 *  compares to `containerWidth`, with a 0.5px sub-pixel rounding fudge. An
 *  empty list trivially fits.
 *
 *  `gapCount` defaults to one gap between every adjacent item, which is right
 *  for a row whose every member sits in the same gapped flex container. A row
 *  built from several clusters passes its own count, since a gap it never
 *  declares is width the row does not spend. */
export function computeFitsInOneRow(
  itemWidths: readonly number[],
  containerWidth: number,
  gapPx: number,
  gapCount: number = Math.max(0, itemWidths.length - 1),
): boolean {
  if (itemWidths.length === 0) return true;
  let total = 0;
  for (const w of itemWidths) total += w;
  total += gapCount * gapPx;
  return total <= containerWidth + 0.5;
}

/** The width the items actually have, which is the container's CONTENT box.
 *  `clientWidth` is the padding box, so a padded row reports space no item can
 *  stand on: the composer's row is padded `0 0.75rem 0.5rem 0.5rem` and
 *  overstated itself by 1.25rem. */
export function contentWidthOf(el: HTMLElement): number {
  const cs = getComputedStyle(el);
  const padding = (parseFloat(cs.paddingLeft) || 0) + (parseFloat(cs.paddingRight) || 0);
  return Math.max(0, el.clientWidth - padding);
}
