import { normalizeDataPath } from './artifacts';
import { parseRepoPath, normalizeLineRange } from '../store';
import { previewMediaKind } from '../../components/files/previewExts';

export type RepoFileLocator = NonNullable<ReturnType<typeof parseRepoPath>>;

export type FileTargetView = 'as-encoded' | 'file';

export interface FileTarget {
  path: string;
  repo: RepoFileLocator | null;
  range: { start: number; end: number } | null;
}

function canPreviewLines(path: string, repo: RepoFileLocator | null, view: FileTargetView): boolean {
  if (view === 'as-encoded' && repo?.mode === 'diff') return false;
  const filePath = repo?.path ?? path;
  return previewMediaKind(filePath.split('.').pop()?.toLowerCase() || '') === 'text';
}

export function resolveFileTarget(
  filePath: string,
  line?: unknown,
  lineEnd?: unknown,
  view: FileTargetView = 'as-encoded',
): FileTarget {
  const path = normalizeDataPath(filePath);
  const repo = parseRepoPath(path);
  const range = canPreviewLines(path, repo, view) ? normalizeLineRange(line, lineEnd) : null;
  return { path, repo, range };
}
