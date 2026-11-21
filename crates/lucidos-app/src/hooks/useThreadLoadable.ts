import { useEffect, useRef, useState } from 'preact/hooks';
import type { Loadable } from '../store/types';
import { toFailed } from '../store/types';
import { useVersionedRefresh } from './useVersionedRefresh';

/** One read about a thread, re-read when the thread changes or `version`
 *  bumps. A re-read of the same thread keeps what is shown. A reply for a
 *  thread the page has since left is dropped. `null` reads nothing.
 *
 *  The setter lands a mutation's own answer, which is the same shape. */
export function useThreadLoadable<T>(
  threadId: string | null,
  version: number,
  read: (threadId: string) => Promise<T>,
): [Loadable<T>, (data: T) => void] {
  const [state, setState] = useState<{ threadId: string | null; value: Loadable<T> }>({
    threadId: null,
    value: { status: 'not-loaded' },
  });
  const current = useRef(threadId);
  current.current = threadId;
  const readRef = useRef(read);
  readRef.current = read;

  const load = (id: string) => {
    setState((prev) => (prev.threadId === id && prev.value.status === 'loaded'
      ? prev
      : { threadId: id, value: { status: 'loading' } }));
    readRef.current(id).then(
      (data) => { if (current.current === id) setState({ threadId: id, value: { status: 'loaded', data } }); },
      (error: unknown) => { if (current.current === id) setState({ threadId: id, value: toFailed(error) }); },
    );
  };

  useEffect(() => {
    if (threadId) load(threadId);
  }, [threadId]);
  useVersionedRefresh(version, false, () => { if (current.current) load(current.current); });

  const shown: Loadable<T> = threadId && state.threadId === threadId ? state.value : { status: 'not-loaded' };
  const land = (data: T) => {
    if (current.current) setState({ threadId: current.current, value: { status: 'loaded', data } });
  };
  return [shown, land];
}
