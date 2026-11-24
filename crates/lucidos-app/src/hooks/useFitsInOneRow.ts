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

export function contentWidthOf(el: HTMLElement): number {
  const cs = getComputedStyle(el);
  const padding = (parseFloat(cs.paddingLeft) || 0) + (parseFloat(cs.paddingRight) || 0);
  return Math.max(0, el.clientWidth - padding);
}
