/**
 * Which hrefs leave the page for the web. The host's click handler and an
 * app frame's share this answer. So neither lets a link fall through to the
 * browser's own navigation, which can push a history entry for the back
 * gesture to land on.
 */

/** The page's own origin stands in for "no origin named": a relative href
 *  resolves onto it, an href that names a host does not. */
const PLACEHOLDER_HOST = 'relative.invalid';

/** The absolute http(s) URL `href` opens, or null when it stays on this page
 *  or names another scheme. The parser decides, never a pattern, and the
 *  result is the parser's own href. So `HTTPS:`, `//host` and `\\host` all come
 *  back absolute. A protocol-relative href takes `pageProtocol`, as the
 *  browser would give it. */
export function externalWebUrl(href: string, pageProtocol: string = window.location.protocol): string | null {
  const protocol = pageProtocol === 'http:' ? 'http:' : 'https:';
  let parsed: URL;
  try {
    parsed = new URL(href, `${protocol}//${PLACEHOLDER_HOST}`);
  } catch {
    return null;
  }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return null;
  if (parsed.hostname === PLACEHOLDER_HOST) return null;
  return parsed.href;
}
