import { useState, useEffect } from 'preact/hooks';
import type { Inputs } from 'preact/hooks';
import type { Loadable } from '../store/types';
import { toFailed } from '../store/types';
import { useDelayedLoading } from './useDelayedLoading';

export interface LoadableFetchResult<T> {
  loadable: Loadable<T>;
  /** Imperative setter — for in-place mutations of the loaded data. */
  setLoadable: (next: Loadable<T> | ((prev: Loadable<T>) => Loadable<T>)) => void;
  /** True only after the fetch has been pending for the spinner-delay window. */
  showLoading: boolean;
}

/** Run `fetcher` whenever `deps` change, tracking the result as a `Loadable<T>`.
 *  Cancels stale fetches on unmount or dep change so late responses don't clobber
 *  the next fetch's state. `fetcher` itself is intentionally NOT in the dep list —
 *  list every closed-over value the fetch depends on (URL, ID, ref, etc.) in `deps`. */
export function useLoadableFetch<T>(
  fetcher: () => Promise<T>,
  deps: Inputs,
): LoadableFetchResult<T> {
  const [loadable, setLoadable] = useState<Loadable<T>>({ status: 'not-loaded' });
  const showLoading = useDelayedLoading(loadable);

  useEffect(() => {
    let canceled = false;
    setLoadable({ status: 'loading' });
    fetcher()
      .then((data) => { if (!canceled) setLoadable({ status: 'loaded', data }); })
      .catch((e: unknown) => { if (!canceled) setLoadable(toFailed(e)); });
    return () => { canceled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  return { loadable, setLoadable, showLoading };
}
