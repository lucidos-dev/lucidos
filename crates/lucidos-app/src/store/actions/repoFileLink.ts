import { repositories, showToast, encodeRepoPath, type Repository } from '../store';
import type { RepoFileHrefTarget } from '../../utils/linkifyPaths';
import { refreshRepositories } from './repositoriesLoader';
import { handleNavigationRequest } from './navigation-dispatch';

export type RepositoryMatch =
  | { kind: 'found'; repo: Repository }
  | { kind: 'missing' }
  | { kind: 'ambiguous'; count: number };

export function matchRepository(repos: readonly Repository[], ref: string): RepositoryMatch {
  const folded = ref.toLowerCase();
  const tiers: ((r: Repository) => boolean)[] = [
    (r) => r.id === ref,
    (r) => r.name === ref,
    (r) => r.name.toLowerCase() === folded,
  ];
  for (const inTier of tiers) {
    const hits = repos.filter(inTier);
    if (hits.length === 1) return { kind: 'found', repo: hits[0] };
    if (hits.length > 1) return { kind: 'ambiguous', count: hits.length };
  }
  return { kind: 'missing' };
}

export async function rereadAndMatch(ref: string): Promise<RepositoryMatch | { kind: 'failed'; error: string }> {
  const snapshot = repositories.value;
  await refreshRepositories();
  const reread = repositories.value;
  if (reread.status === 'loaded') return matchRepository(reread.data, ref);
  if (snapshot.status === 'loaded') repositories.value = snapshot;
  return { kind: 'failed', error: reread.status === 'failed' ? reread.error : 'the list did not load' };
}

export async function openRepoFileLink(
  target: RepoFileHrefTarget,
  source?: string,
  open: (request: { file_path: string; line?: number; line_end?: number }) => void = (request) =>
    handleNavigationRequest({ target: 'file', ...request }, { source }),
): Promise<void> {
  const { locator, line, lineEnd } = target;
  const cached = repositories.value.status === 'loaded'
    ? matchRepository(repositories.value.data, locator.repoId)
    : null;
  const match = cached?.kind === 'found' ? cached : await rereadAndMatch(locator.repoId);
  if (match.kind === 'found') {
    open({ file_path: encodeRepoPath({ ...locator, repoId: match.repo.id }), line, line_end: lineEnd });
    return;
  }
  const reason = match.kind === 'missing'
    ? 'no repository with that id or name is registered in this workspace'
    : match.kind === 'ambiguous'
      ? `${match.count} registered repositories share that name, so link it by id`
      : `failed to load repositories: ${match.error}`;
  const from = source ? ` (requested by ${source})` : '';
  showToast(
    `Can't open "${locator.path}" in repository "${locator.repoId}"${from}: ${reason}`,
    'error',
    { key: `repo-file-link-${locator.repoId}` },
  );
}
