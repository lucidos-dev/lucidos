const ALLOWED_SCHEMES = ['https', 'http', 'ftp', 'file', 'mailto', 'thread'];
const SCHEME_PATTERN = `(?:${ALLOWED_SCHEMES.join('|')}):[^\\s\\(\\)\\[\\]]+`;
const PLAIN_URL_RE = new RegExp(`^${SCHEME_PATTERN}$`);
const MARKDOWN_LINK_RE = new RegExp(`^\\[[^\\]]*\\]\\((${SCHEME_PATTERN})\\)$`);

export function extractPasteUrl(text: string): string | null {
  const trimmed = text.trim();
  if (!trimmed) return null;
  const md = trimmed.match(MARKDOWN_LINK_RE);
  if (md) return md[1];
  if (PLAIN_URL_RE.test(trimmed)) return trimmed;
  return null;
}

export function escapeMarkdownLinkText(text: string): string {
  return text.replace(/([\\\]])/g, '\\$1');
}
