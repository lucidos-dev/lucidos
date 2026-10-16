// What a hover tooltip shows for a link inside a rendered markdown DOCUMENT
// preview: where it goes, before the user clicks.
//
// Reuses `climb`/`folderOf`: the same primitives `markdownImageSource.ts`
// resolves an image against. So a relative link's shown target is the exact
// string a click opens. An over-climbing `..` is refused the same way an
// image's is. See docs/plans/2026-10-05-link-target-tooltips-in-preview.md.

import { climb, folderOf, type MarkdownDocumentLocation } from './markdownImageSource';
import { HEADING_ID_PREFIX } from './headingSlug';

/** What `markdownLinkTooltip` found for one link. `title` is the author's own
 *  markdown title (`[x](url "title")`), shown above `text` in the tooltip. */
export interface MarkdownLinkTooltip {
  title?: string;
  text: string;
}

const URL_SCHEME = /^[a-z][a-z0-9+.-]*:/i;

function decodeFragment(id: string): string {
  try {
    return decodeURIComponent(id);
  } catch {
    return id; // malformed escape: match the raw token rather than drop it
  }
}

/** The target heading's own text for a `#id` anchor, or `null` when no
 *  heading in `candidates` carries that id. Mirrors `fragmentTarget` in
 *  `previewIframeLinks.ts`: the prefixed form is tried first, since a
 *  stamped heading id always carries `HEADING_ID_PREFIX` while the link
 *  names the bare slug. `candidates` is collected once per document by the
 *  caller, rather than re-queried here for every fragment link. */
function fragmentHeadingText(candidates: readonly Element[], id: string): string | null {
  const match = candidates.find((el) => el.id === `${HEADING_ID_PREFIX}${id}`)
    ?? candidates.find((el) => el.id === id)
    ?? candidates.find((el) => el.getAttribute('name') === id);
  return match ? (match.textContent ?? '').trim() : null;
}

/** Resolve a scheme-less href's path part to root-relative segments, or
 *  `null` when it climbs above the repo checkout or the workspace data root.
 *  Mirrors the branching `resolvePreviewRelativePath` /
 *  `resolvePreviewRelativeRepoPath` use for a click. Unlike those two, it
 *  goes through `climb`, which reports an over-climb instead of clamping it. */
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

/** What a hover over `href` (an anchor's `href` ATTRIBUTE, raw markdown
 *  title included) should tell the reader, for a link inside `doc`.
 *  `fragmentCandidates` is every `[id]`/`[name]` element in the rendered
 *  document, collected once by the caller. It must already carry the
 *  stamped heading ids: run this after `stampHeadingIds`. `null` means:
 *  stamp no tooltip at all. */
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
    if (!id) return null; // a bare "#" (back-to-top) names nothing to show
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
