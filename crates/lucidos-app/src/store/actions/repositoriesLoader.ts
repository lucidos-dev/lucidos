/**
 * `loadRepositories()` — extracted from `chat.ts` so the SSE-handler module
 * (`entityReferences.ts`) can refresh the registered-repositories cache on
 * `RepositoryAdded`/`RepositoryRemoved` without pulling in chat.ts's heavy
 * transitive dependency tree (connection.ts, chat-changes.ts, …).
 * (`RepositoryImported` is a `git_clone` into `data/artifacts/`, not a
 * registered repo — `entityReferences.ts` refreshes the `artifacts` cache for
 * it instead.)
 *
 * `chat.ts` re-exports this so existing importers
 * (`import { loadRepositories } from '../store/actions/chat'`) keep working
 * — the function lives here, callers see no change.
 */
import { repositories } from '../store';
import type { Repository } from '../store';
import { setLoadingIfFresh, toFailed } from '../types';
import { API, json as apiJson } from '../../api/client';

// Shared in-flight load so concurrent callers await the SAME fetch instead of
// racing duplicate GETs: the compose destination picker's render-path kick-off
// and useStartup's eager load both fire on a cold start. An early `return`
// would resolve immediately while the real fetch is still in flight, so an
// `await loadRepositories()` caller (loadChangeContext, viewThreadCcDiff) would
// then read a still-'loading' Loadable — hence sharing the promise, not skipping.
let repositoriesLoadInFlight: Promise<void> | null = null;

export function loadRepositories(): Promise<void> {
  if (repositoriesLoadInFlight) return repositoriesLoadInFlight;
  repositoriesLoadInFlight = loadRepositoriesInner()
    .finally(() => { repositoriesLoadInFlight = null; });
  return repositoriesLoadInFlight;
}

async function loadRepositoriesInner(): Promise<void> {
  setLoadingIfFresh(repositories);
  try {
    const data = await apiJson<Repository[]>(`${API}/repositories`);
    repositories.value = { status: 'loaded', data };
  } catch (e) {
    repositories.value = toFailed(e);
  }
}
