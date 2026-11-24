
const PLACEHOLDER_HOST = 'relative.invalid';

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
