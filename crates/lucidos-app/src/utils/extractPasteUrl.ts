const ALLOWED_SCHEMES = ['https', 'http', 'ftp', 'file', 'mailto', 'thread'];
// URL char class excludes whitespace AND markdown link delimiters () [],
// so a crafted clipboard payload cannot break out of [title](url) and
// inject a second link pointing elsewhere.
const SCHEME_PATTERN = `(?:${ALLOWED_SCHEMES.join('|')}):[^\\s\\(\\)\\[\\]]+`;
const PLAIN_URL_RE = new RegExp(`^${SCHEME_PATTERN}$`);
const MARKDOWN_LINK_RE = new RegExp(`^\\[[^\\]]*\\]\\((${SCHEME_PATTERN})\\)$`);

/** If `text` is exclusively a URL or markdown link with an allowed scheme,
 *  return the URL. Otherwise null. Used by paste-substitution to decide
 *  whether to wrap the textarea selection as the link's title. */
export function extractPasteUrl(text: string): string | null {
  const trimmed = text.trim();
  if (!trimmed) return null;
  const md = trimmed.match(MARKDOWN_LINK_RE);
  if (md) return md[1];
  if (PLAIN_URL_RE.test(trimmed)) return trimmed;
  return null;
}

/** Escape characters that would otherwise break out of a markdown link's
 *  title — `]` would close it early, `\` could spawn an unintended escape. */
export function escapeMarkdownLinkText(text: string): string {
  return text.replace(/([\\\]])/g, '\\$1');
}
