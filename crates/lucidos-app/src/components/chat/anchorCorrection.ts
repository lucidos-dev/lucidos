export function splitAnchorCorrection(exact: number): { scrollTop: number; spacer: number } {
  const scrollTop = Math.ceil(exact) + 0;
  return { scrollTop, spacer: scrollTop - exact };
}

const ANCHOR_SUBPIXEL = '--anchor-subpixel';

export function anchorSpacer(container: HTMLElement): number {
  return parseFloat(container.style.getPropertyValue(ANCHOR_SUBPIXEL)) || 0;
}

export function setAnchorSpacer(container: HTMLElement, px: number): void {
  if (px === 0) container.style.removeProperty(ANCHOR_SUBPIXEL);
  else container.style.setProperty(ANCHOR_SUBPIXEL, `${px}px`);
}
