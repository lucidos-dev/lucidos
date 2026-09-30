import { repositories, showToast, encodeRepoPath, type Repository } from '../store';
import type { RepoFileHrefTarget } from '../../utils/linkifyPaths';
import { refreshRepositories } from './repositoriesLoader';
import { handleNavigationRequest } from './navigation-request';

/** How the repository segment of a `repo:` link resolved against the registry. */
export type RepositoryMatch =
  | { kind: 'found'; repo: Repository }
  | { kind: 'missing' }
  | { kind: 'ambiguous'; count: number };

/** Resolve the repository segment of a `repo:` link: an id, then an exact
 *  name, then a name in any case. The first tier with a hit decides, so an id
 *  never loses to a same-spelled name. Names are not unique in the registry,
 *  so two hits in one tier are ambiguous rather than a guess. */
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

/** Match `ref` after re-reading the registry. The cached list lags a
 *  repository registered moments ago, so a miss off the cache alone would
 *  report a live repository as unregistered. The read is a fresh request,
 *  since a load already in flight can predate that registration. A failed
 *  re-read keeps the previous list for every other surface. */
async function rereadAndMatch(ref: string): Promise<RepositoryMatch | { kind: 'failed'; error: string }> {
  const snapshot = repositories.value;
  await refreshRepositories();
  const reread = repositories.value;
  if (reread.status === 'loaded') return matchRepository(reread.data, ref);
  if (snapshot.status === 'loaded') repositories.value = snapshot;
  return { kind: 'failed', error: reread.status === 'failed' ? reread.error : 'the list did not load' };
}

/** Open a `repo:` link from rendered markdown in the Files panel's repository
 *  file preview. The link may name its repository by id or by name.
 *
 *  A cache hit opens before the first `await`, inside the click's own task.
 *  An unresolvable repository toasts, naming the file, the repository and the
 *  link's origin. A missing file or ref is the preview's own load error. */
export async function openRepoFileLink(target: RepoFileHrefTarget, source?: string): Promise<void> {
  const { locator, line, lineEnd } = target;
  const cached = repositories.value.status === 'loaded'
    ? matchRepository(repositories.value.data, locator.repoId)
    : null;
  const match = cached?.kind === 'found' ? cached : await rereadAndMatch(locator.repoId);
  if (match.kind === 'found') {
    handleNavigationRequest(
      { target: 'file', file_path: encodeRepoPath({ ...locator, repoId: match.repo.id }), line, line_end: lineEnd },
      { source },
    );
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
