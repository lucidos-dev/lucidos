import { parseRepoPath } from '../store/store';
import { workspaceDataRoot } from './dataPathPrefixes';

export function previewFilePath(encoded: string): string {
  return parseRepoPath(encoded)?.path ?? encoded;
}

export function previewFileName(encoded: string): string {
  const repoRelative = previewFilePath(encoded);
  return repoRelative.split('/').pop() || repoRelative;
}

export function previewDiskPath(
  encoded: string,
  workspacePath: string,
  repositories: readonly { id: string; path: string }[],
): string | null {
  const repo = parseRepoPath(encoded);
  if (repo) {
    const clone = repositories.find(r => r.id === repo.repoId);
    return clone?.path ? joinPath(clone.path, repo.path) : null;
  }
  if (!workspacePath || encoded.startsWith('system-knowhow/')) return null;
  return `${workspaceDataRoot(workspacePath)}${encoded}`;
}

export function previewCopyPath(
  encoded: string,
  workspacePath: string,
  repositories: readonly { id: string; path: string }[],
): { path: string; absolute: boolean } {
  const disk = previewDiskPath(encoded, workspacePath, repositories);
  return disk ? { path: disk, absolute: true } : { path: previewFilePath(encoded), absolute: false };
}

function joinPath(root: string, relative: string): string {
  return `${root.replace(/\/+$/, '')}/${relative}`;
}

export function splitPreviewPath(encoded: string): { dir: string; name: string } {
  const path = previewFilePath(encoded);
  const cut = path.lastIndexOf('/');
  if (cut < 0) return { dir: '', name: path };
  return { dir: path.slice(0, cut + 1), name: path.slice(cut + 1) };
}
