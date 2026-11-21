/** Escapes text for an HTML attribute value or text content. DOM-free, so the
 *  entry chunk and a test with no DOM can both use it. */
export function escapeHtmlAttr(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}
