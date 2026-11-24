import { useState, useEffect } from 'preact/hooks';
import type { Inputs } from 'preact/hooks';
import type { Loadable } from '../store/types';
import { toFailed, loadingIfFresh } from '../store/types';
import { useDelayedLoading } from './useDelayedLoading';

export interface LoadableFetchResult<T> {
  loadable: Loadable<T>;
  setLoadable: (next: Loadable<T> | ((prev: Loadable<T>) => Loadable<T>)) => void;
  showLoading: boolean;
}

export function useLoadableFetch<T>(
  fetcher: () => Promise<T>,
  deps: Inputs,
  opts: {
    keepLoadedWhileRefetching?: boolean;
    onSettled?: () => void;
  } = {},
): LoadableFetchResult<T> {
  const [loadable, setLoadable] = useState<Loadable<T>>({ status: 'not-loaded' });
  const showLoading = useDelayedLoading(loadable);

  useEffect(() => {
    let canceled = false;
    if (opts.keepLoadedWhileRefetching) setLoadable(loadingIfFresh);
    else setLoadable({ status: 'loading' });
    fetcher()
      .then((data) => {
        if (canceled) return;
        setLoadable({ status: 'loaded', data });
        opts.onSettled?.();
      })
      .catch((e: unknown) => {
        if (canceled) return;
        setLoadable(toFailed(e));
        opts.onSettled?.();
      });
    return () => { canceled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  return { loadable, setLoadable, showLoading };
}
