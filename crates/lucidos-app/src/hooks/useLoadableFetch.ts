import { useState, useEffect } from 'preact/hooks';
import type { Inputs } from 'preact/hooks';
import type { Loadable } from '../store/types';
import { toFailed, loadingIfFresh } from '../store/types';
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
  opts: {
    /** Keep the loaded data visible while a dep change re-reads it, so the
     *  content swaps in place instead of blanking. Right when the deps carry
     *  a REFRESH of the same thing, e.g. an SSE epoch. Wrong when they name a
     *  different thing, e.g. another file, where the old content is a lie. */
    keepLoadedWhileRefetching?: boolean;
    /** Checked when the reply lands, not when the fetch starts. Return false
     *  to drop it. The dep-change cancel cannot cover this: what makes a reply
     *  unwanted here is something that did NOT change the deps, such as the
     *  user typing into the very content being re-read. Read it from a ref;
     *  state read at render time is a render behind. */
    stillWanted?: () => boolean;
  } = {},
): LoadableFetchResult<T> {
  const [loadable, setLoadable] = useState<Loadable<T>>({ status: 'not-loaded' });
  const showLoading = useDelayedLoading(loadable);

  useEffect(() => {
    let canceled = false;
    const wanted = () => !canceled && (opts.stillWanted?.() ?? true);
    if (opts.keepLoadedWhileRefetching) setLoadable(loadingIfFresh);
    else setLoadable({ status: 'loading' });
    fetcher()
      .then((data) => { if (wanted()) setLoadable({ status: 'loaded', data }); })
      .catch((e: unknown) => { if (wanted()) setLoadable(toFailed(e)); });
    return () => { canceled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  return { loadable, setLoadable, showLoading };
}
