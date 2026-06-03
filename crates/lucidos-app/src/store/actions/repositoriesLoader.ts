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

export async function loadRepositories(): Promise<void> {
  setLoadingIfFresh(repositories);
  try {
    const data = await apiJson<Repository[]>(`${API}/repositories`);
    repositories.value = { status: 'loaded', data };
  } catch (e) {
    repositories.value = toFailed(e);
  }
}
