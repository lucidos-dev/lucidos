
import { climb, folderOf, type MarkdownDocumentLocation } from './markdownImageSource';
import { HEADING_ID_PREFIX } from './headingSlug';

export interface MarkdownLinkTooltip {
  title?: string;
  text: string;
}

const URL_SCHEME = /^[a-z][a-z0-9+.-]*:/i;

function decodeFragment(id: string): string {
  try {
    return decodeURIComponent(id);
  } catch {
    return id;
  }
}

function fragmentHeadingText(candidates: readonly Element[], id: string): string | null {
  const match = candidates.find((el) => el.id === `${HEADING_ID_PREFIX}${id}`)
    ?? candidates.find((el) => el.id === id)
    ?? candidates.find((el) => el.getAttribute('name') === id);
  return match ? (match.textContent ?? '').trim() : null;
}

function resolveLinkSegments(doc: MarkdownDocumentLocation, pathPart: string): string[] | null {
  if (doc.kind === 'repo') {
    if (pathPart.startsWith('/')) return climb([], pathPart.slice(1).split('/'));
    return climb(folderOf(doc.path), pathPart.split('/'));
  }
  if (pathPart.startsWith('/data/')) return climb([], pathPart.slice('/data/'.length).split('/'));
  if (pathPart.startsWith('data/')) return climb([], pathPart.slice('data/'.length).split('/'));
  if (pathPart.startsWith('/')) return climb([], pathPart.slice(1).split('/'));
  return climb(folderOf(doc.path), pathPart.split('/'));
}

export function markdownLinkTooltip(
  href: string,
  title: string,
  doc: MarkdownDocumentLocation,
  fragmentCandidates: readonly Element[],
): MarkdownLinkTooltip | null {
  const trimmed = href.trim();
  if (!trimmed) return null;
  const withTitle = (text: string): MarkdownLinkTooltip => (title ? { title, text } : { text });

  if (trimmed.startsWith('#')) {
    const id = decodeFragment(trimmed.slice(1));
    if (!id) return null;
    const headingText = fragmentHeadingText(fragmentCandidates, id);
    return withTitle(headingText ?? `No "${id}" section in ${doc.path}`);
  }

  if (URL_SCHEME.test(trimmed) || trimmed.startsWith('//')) {
    return withTitle(trimmed);
  }

  const hashAt = trimmed.indexOf('#');
  const pathPart = trimmed.split(/[?#]/, 1)[0];
  const fragment = hashAt === -1 ? '' : trimmed.slice(hashAt);
  const resolved = resolveLinkSegments(doc, pathPart);
  if (resolved === null) return withTitle(`${trimmed} (not reachable)`);
  return withTitle(`${resolved.join('/')}${fragment}`);
}
