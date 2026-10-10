/** The Copy button's two faces: the copy glyph, and the check it flashes once
 *  the copy lands. Its own module because startup's delegated copy handler
 *  needs them, and `markedConfig.ts` would bring the markdown renderer into
 *  the entry chunk with them. */

export const COPY_ICON = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="5.5" y="5.5" width="8" height="8" rx="1.5"/><path d="M3 10.5V3a1.5 1.5 0 0 1 1.5-1.5H10"/></svg>';
export const CHECK_ICON = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3.5 8.5 6.5 11.5 12.5 4.5"/></svg>';
