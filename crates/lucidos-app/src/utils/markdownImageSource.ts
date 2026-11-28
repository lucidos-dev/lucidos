import { lucidos } from '@lucidos/sdk';
import { changeFileUrl, repoFileUrl } from '../api/client/fileUrls';
import { DATA_PATH_PREFIXES } from './dataPathPrefixes';

export type MarkdownDocumentLocation =
  | { kind: 'workspace'; path: string }
  | { kind: 'repo'; repoId: string; path: string; ref?: string; changeId?: string };

export type ImageSourceVerdict =
  | { kind: 'keep' }
  | { kind: 'serve'; src: string }
  | { kind: 'refuse' };

const KEEP: ImageSourceVerdict = { kind: 'keep' };
const REFUSE: ImageSourceVerdict = { kind: 'refuse' };

const WORKSPACE_DATA_DIRS = new Set(DATA_PATH_PREFIXES.map((p) => p.slice(0, -1)));

export function markdownImageSource(src: string, doc?: MarkdownDocumentLocation): ImageSourceVerdict {
  if (/^[a-z][a-z0-9+.-]*:/i.test(src) || src.startsWith('//')) return KEEP;
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

function serveData(segments: string[], suffix: string): ImageSourceVerdict {
  return { kind: 'serve', src: `${lucidos.data.url(segments.join('/'))}${suffix}` };
}

function decodedSegments(rawPath: string): string[] | null {
  try {
    return rawPath.split('/').map(decodeURIComponent).join('/').split(/[/\\]/);
  } catch {
    return null;
  }
}

export function folderOf(path: string): string[] {
  return path.split('/').slice(0, -1).filter((s) => s !== '');
}

export function climb(from: string[], segments: string[]): string[] | null {
  const out = [...from];
  for (const segment of segments) {
    if (segment === '' || segment === '.') continue;
    if (segment !== '..') out.push(segment);
    else if (out.pop() === undefined) return null;
  }
  return out;
}
