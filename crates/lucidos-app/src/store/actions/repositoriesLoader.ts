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
import { sharedSliceLoader, type SliceOf } from '../bootstrapSliceLoads';
import { API, json as apiJson, retryTransient } from '../../api/client';

// Concurrent callers share one read rather than racing duplicate GETs: the
// compose destination picker's render-path kick-off and startClient's eager
// load both fire on a cold start. An early `return` would resolve while the
// fetch is still out, so an `await loadRepositories()` caller
// (loadChangeContext, viewThreadCcDiff) would read a still-'loading' Loadable.
export function loadRepositories(): Promise<void> {
  return repositoriesLoader.load();
}

/** Load the list with a request sent after this call. A load already in flight
 *  may predate what the caller just saw, so wait it out and send another. */
export async function refreshRepositories(): Promise<void> {
  const inFlight = repositoriesLoader.inFlight();
  if (inFlight) await inFlight;
  return loadRepositories();
}

export const repositoriesSlice: SliceOf<'repositories'> = {
  begin: () => setLoadingIfFresh(repositories),
  apply: (data) => { repositories.value = { status: 'loaded', data }; },
  fail: (e) => { repositories.value = toFailed(e); },
};

// Retry a transient rejection before flipping to `failed`. Nothing refetches a
// failed list: the compose picker only starts a `not-loaded` one, and the SSE
// refresh only re-reads a `loaded` one. One cancelled fetch, or a deadline
// that fired while the engine booted, would otherwise leave a permanent
// "Failed to load repositories" row.
const repositoriesLoader = sharedSliceLoader(
  'repositories',
  repositoriesSlice,
  () => retryTransient(() => apiJson<Repository[]>(`${API}/repositories`)),
);
