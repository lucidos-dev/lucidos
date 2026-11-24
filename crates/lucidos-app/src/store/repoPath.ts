
export type RepoLocator =
  | {
      repoId: string;
      mode: 'file';
      ref?: string;
      path: string;
    }
  | { repoId: string; mode: 'diff'; changeId?: string; path: string };

export function encodeRepoPath(locator: RepoLocator): string {
  const qualifier = locator.mode === 'file' ? locator.ref : locator.changeId;
  const modeSeg = qualifier ? `${locator.mode}#${qualifier}` : locator.mode;
  return `repo:${locator.repoId}:${modeSeg}:${locator.path}`;
}

export function parseRepoPath(encoded: string): RepoLocator | null {
  if (!encoded.startsWith('repo:')) return null;
  const [, repoId, modeSeg, ...rest] = encoded.split(':');
  const path = rest.join(':');
  if (!repoId || !path || !modeSeg) return null;

  const hash = modeSeg.indexOf('#');
  const mode = hash === -1 ? modeSeg : modeSeg.slice(0, hash);
  const qualifier = hash === -1 ? undefined : modeSeg.slice(hash + 1);
  if (qualifier === '') return null;

  if (mode === 'file') return { repoId, mode, ref: qualifier, path };
  if (mode === 'diff') return { repoId, mode, changeId: qualifier, path };
  return null;
}
