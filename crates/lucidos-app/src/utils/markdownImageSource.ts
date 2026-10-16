import { lucidos } from '@lucidos/sdk';
import { changeFileUrl, repoFileUrl } from '../api/client/fileUrls';
import { DATA_PATH_PREFIXES } from './dataPathPrefixes';

/** Where a rendered markdown DOCUMENT lives, so a relative image source can
 *  resolve against its folder. A workspace file is named by its data path. A
 *  repository file is named by its checkout, its path, and the revision it was
 *  read at: a git `ref`, or the Change whose end state it shows. */
export type MarkdownDocumentLocation =
  | { kind: 'workspace'; path: string }
  | { kind: 'repo'; repoId: string; path: string; ref?: string; changeId?: string };

/** What becomes of one image source. `keep` leaves the authored value, `serve`
 *  replaces it, and `refuse` means it must never be fetched. */
export type ImageSourceVerdict =
  | { kind: 'keep' }
  | { kind: 'serve'; src: string }
  | { kind: 'refuse' };

const KEEP: ImageSourceVerdict = { kind: 'keep' };
const REFUSE: ImageSourceVerdict = { kind: 'refuse' };

/** The workspace's top-level directories. A relative source starting with one
 *  is root-relative wherever it is written, which is the form
 *  `lucidos data write` prints.
 *
 *  Derived from `DATA_PATH_PREFIXES`, the single source of truth for the same
 *  list. A hand-kept copy gave a new sub-tree its links but not its images,
 *  and that miss shows up as an `<img>` served the SPA fallback. */
const WORKSPACE_DATA_DIRS = new Set(DATA_PATH_PREFIXES.map((p) => p.slice(0, -1)));

/** The served `src` for an image in rendered markdown.
 *
 *  Workspace files are served under the `/data` mount, so a bare
 *  `artifacts/x.png` resolves against the SPA base instead, which no route
 *  owns: the fallback answers with `index.html` and the `<img>` breaks.
 *  `lucidos.data.url` builds the URL for every topology at once: the gateway's
 *  `/<slug>` prefix or none, and `system-knowhow/` on its API endpoint.
 *
 *  With no `document` (chat), only a source naming a workspace directory is
 *  rewritten. A relative path that means something else in its own context is
 *  never silently redirected at the workspace. With one, a relative source
 *  resolves against the document's folder, and one that climbs out of the data
 *  root or the checkout is refused. */
export function markdownImageSource(src: string, doc?: MarkdownDocumentLocation): ImageSourceVerdict {
  // A scheme (`https:`, `data:`, `blob:`) or a protocol-relative `//host/…`
  // addresses something the browser resolves without help.
  if (/^[a-z][a-z0-9+.-]*:/i.test(src) || src.startsWith('//')) return KEEP;
  // A query or fragment is not part of the file name. Re-attached afterwards,
  // `?v=2` stays a query, rather than the path encoder folding it into the name
  // as a file called `x.png%3Fv%3D2`.
  const cut = src.search(/[?#]/);
  const rawPath = cut === -1 ? src : src.slice(0, cut);
  const suffix = cut === -1 ? '' : src.slice(cut);
  if (rawPath === '') return KEEP;
  if (doc?.kind === 'repo') return repoImageSource(rawPath, doc);
  if (rawPath.startsWith('/')) return KEEP;
  const segments = decodedSegments(rawPath);
  if (!doc) {
    if (!segments || !WORKSPACE_DATA_DIRS.has(segments[0]) || segments.includes('..')) return KEEP;
    return serveData(segments, suffix);
  }
  if (!segments) return REFUSE;
  const from = WORKSPACE_DATA_DIRS.has(segments[0]) ? [] : folderOf(doc.path);
  const resolved = climb(from, segments);
  return resolved ? serveData(resolved, suffix) : REFUSE;
}

/** A repository image. A leading `/` names the checkout root, as it does for a
 *  link. The query and fragment are dropped: the path already rides in the
 *  file URL's own query, and `?raw=true` means nothing to it. */
function repoImageSource(
  rawPath: string,
  doc: Extract<MarkdownDocumentLocation, { kind: 'repo' }>,
): ImageSourceVerdict {
  const fromRoot = rawPath.startsWith('/');
  const segments = decodedSegments(fromRoot ? rawPath.slice(1) : rawPath);
  const resolved = segments && climb(fromRoot ? [] : folderOf(doc.path), segments);
  if (!resolved || resolved.length === 0) return REFUSE;
  const path = resolved.join('/');
  return {
    kind: 'serve',
    src: doc.changeId ? changeFileUrl(doc.changeId, path) : repoFileUrl(doc.repoId, path, doc.ref),
  };
}

/** Plain text, not attribute-ready: `setAttribute` writes it, and the
 *  serializer escapes on the way out. Escaping here too would double it. */
function serveData(segments: string[], suffix: string): ImageSourceVerdict {
  return { kind: 'serve', src: `${lucidos.data.url(segments.join('/'))}${suffix}` };
}

/** The path's segments, decoded, or `null` for a malformed escape.
 *
 *  Decoded before anything inspects them. marked percent-encodes the src and
 *  the URL builders encode each segment again, so a space would round-trip to
 *  `%2520` and miss the file. Decoding also turns an obfuscated `%2e%2e` into
 *  the `..` a climb check can see.
 *
 *  Re-split AFTER joining, on both separators. A decoded segment can itself
 *  hold one: `%2e%2e%2f%2e%2e` is one innocent-looking segment until a builder
 *  splits it back into two real ones. */
function decodedSegments(rawPath: string): string[] | null {
  try {
    return rawPath.split('/').map(decodeURIComponent).join('/').split(/[/\\]/);
  } catch {
    return null;
  }
}

/** The folder segments of a document path.
 *
 *  Exported for `markdownLinkTooltip.ts`, which resolves a LINK's target
 *  against the same folder an image source resolves against. */
export function folderOf(path: string): string[] {
  return path.split('/').slice(0, -1).filter((s) => s !== '');
}

/** Walk `segments` from `from`, or `null` when a `..` would step above the
 *  root. Never clamped: a source that climbs out names a file outside, and
 *  pinning it to the root would show a different file than the one meant.
 *
 *  Exported so `markdownLinkTooltip.ts` detects the same outside-root case
 *  for a LINK, rather than growing a second climb algorithm. */
export function climb(from: string[], segments: string[]): string[] | null {
  const out = [...from];
  for (const segment of segments) {
    if (segment === '' || segment === '.') continue;
    if (segment !== '..') out.push(segment);
    else if (out.pop() === undefined) return null;
  }
  return out;
}
