import { useEffect, useRef } from 'preact/hooks';
import { whenLoaded, type ChunkGate, type PendingOpen } from '../utils/lazyComponent';

/** One component's open that waits on a chunk (`whenLoaded`). A second press
 *  takes it back, and unmounting cancels it. */
export function usePendingOpen() {
  const pending = useRef<PendingOpen | null>(null);
  useEffect(() => () => pending.current?.cancel(), []);
  return {
    /** Cancel a waiting open. True when there was one, so the press is spent. */
    takeBack(): boolean {
      if (!pending.current?.pending) return false;
      pending.current.cancel();
      return true;
    },
    open(chunk: ChunkGate, run: () => void, trigger: Element | null): void {
      pending.current?.cancel();
      pending.current = whenLoaded(chunk, run, trigger);
    },
  };
}
