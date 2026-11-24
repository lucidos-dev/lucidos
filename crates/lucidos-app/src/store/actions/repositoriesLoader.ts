import { repositories } from '../store';
import type { Repository } from '../store';
import { setLoadingIfFresh, toFailed } from '../types';
import { sharedSliceLoader, type SliceOf } from '../bootstrapSliceLoads';
import { API, json as apiJson, retryTransient } from '../../api/client';

export function loadRepositories(): Promise<void> {
  return repositoriesLoader.load();
}

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

const repositoriesLoader = sharedSliceLoader(
  'repositories',
  repositoriesSlice,
  () => retryTransient(() => apiJson<Repository[]>(`${API}/repositories`)),
);
