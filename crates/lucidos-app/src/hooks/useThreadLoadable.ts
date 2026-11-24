import { useEffect, useRef, useState } from 'preact/hooks';
import type { Loadable } from '../store/types';
import { toFailed } from '../store/types';
import { useVersionedRefresh } from './useVersionedRefresh';

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
