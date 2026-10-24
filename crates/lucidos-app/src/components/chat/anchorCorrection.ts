/** A scroll offset holds only whole pixels, and layout is fractional. So the
 *  anchor correction writes the whole pixel at or above the exact offset, and
 *  carries the rest in a top spacer on the transcript. Scroll and spacer then
 *  cancel exactly, and a press moves nothing (ADR 0286, amending ADR 0078).
 *
 *  `exact` is the offset that would hold the anchor with no spacer. */
export function splitAnchorCorrection(exact: number): { scrollTop: number; spacer: number } {
  // `+ 0` turns a `-0` from a slightly negative correction into 0.
  const scrollTop = Math.ceil(exact) + 0;
  return { scrollTop, spacer: scrollTop - exact };
}

/** The custom property both top reserves of `.thread-content` add: the desktop
 *  padding and the mobile header spacer. */
const ANCHOR_SUBPIXEL = '--anchor-subpixel';

export function anchorSpacer(container: HTMLElement): number {
  return parseFloat(container.style.getPropertyValue(ANCHOR_SUBPIXEL)) || 0;
}

export function setAnchorSpacer(container: HTMLElement, px: number): void {
  if (px === 0) container.style.removeProperty(ANCHOR_SUBPIXEL);
  else container.style.setProperty(ANCHOR_SUBPIXEL, `${px}px`);
}
