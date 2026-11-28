
export const PHONE_MAX_WIDTH_PX = 768;
export const PHONE_LANDSCAPE_MAX_HEIGHT_PX = 500;

const NARROW = `(max-width: ${PHONE_MAX_WIDTH_PX}px)`;
const WIDE = `(min-width: ${PHONE_MAX_WIDTH_PX + 1}px)`;

export const PHONE_LAYOUT_QUERY =
  `${NARROW}, (max-height: ${PHONE_LANDSCAPE_MAX_HEIGHT_PX}px) and (pointer: coarse)`;

export const DESKTOP_LAYOUT_QUERY = [
  `${WIDE} and (min-height: ${PHONE_LANDSCAPE_MAX_HEIGHT_PX + 1}px)`,
  `${WIDE} and (pointer: fine)`,
  `${WIDE} and (pointer: none)`,
].join(', ');

export const LAYOUT_MEDIA: Readonly<Record<string, string>> = {
  '(--phone-layout)': PHONE_LAYOUT_QUERY,
  '(--desktop-layout)': DESKTOP_LAYOUT_QUERY,
};

export function expandLayoutMedia(params: string): string {
  const trimmed = params.trim();
  const query = LAYOUT_MEDIA[trimmed];
  if (query) return query;
  if (/\(--(phone|desktop)-layout\)/.test(trimmed)) {
    throw new Error(`@media ${trimmed}: a layout query must stand alone`);
  }
  return params;
}

export function isPhoneLayout(viewport: { width: number; height: number; coarsePointer: boolean }): boolean {
  return viewport.width <= PHONE_MAX_WIDTH_PX
    || (viewport.height <= PHONE_LANDSCAPE_MAX_HEIGHT_PX && viewport.coarsePointer);
}

export function hasCoarsePointer(): boolean {
  return typeof window !== 'undefined' && (window.matchMedia?.('(pointer: coarse)').matches ?? false);
}

export function viewportGetsPhoneLayout(): boolean {
  if (typeof window === 'undefined') return false;
  return isPhoneLayout({ width: window.innerWidth, height: window.innerHeight, coarsePointer: hasCoarsePointer() });
}
