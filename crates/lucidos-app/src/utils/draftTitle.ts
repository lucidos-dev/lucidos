/** Maximum visible character length of a derived draft title (including the
 *  ellipsis when truncated). Counted in code points, not bytes — emoji and
 *  other multi-byte characters count as one. */
export const DRAFT_TITLE_MAX = 40;

/** Title shown when a draft has no text content (only images, or freshly
 *  created). Matches the ComposeDraftRow placeholder in ThreadDrawer. */
export const DRAFT_FALLBACK_TITLE = 'New thread';

const ELLIPSIS = '…';

/**
 * Derive a Drawer-row title from raw draft text. Takes the first non-empty
 * line, trims surrounding whitespace, and truncates to DRAFT_TITLE_MAX code
 * points. Returns DRAFT_FALLBACK_TITLE when the text contains no visible
 * content.
 */
export function draftTitle(text: string): string {
  // Normalize CR / CRLF so the first-line split treats every separator the same.
  const normalized = text.replace(/\r\n?/g, '\n');
  const firstLine = normalized.split('\n').map(l => l.trim()).find(l => l.length > 0);
  if (!firstLine) return DRAFT_FALLBACK_TITLE;

  // Iterate by code point so multi-byte characters are not split mid-glyph.
  const chars = [...firstLine];
  if (chars.length <= DRAFT_TITLE_MAX) return firstLine;
  return chars.slice(0, DRAFT_TITLE_MAX - 1).join('') + ELLIPSIS;
}
