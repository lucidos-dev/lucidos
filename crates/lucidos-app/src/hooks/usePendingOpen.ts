import { useEffect, useRef } from 'preact/hooks';
import { whenLoaded, type ChunkGate, type PendingOpen } from '../utils/lazyComponent';

export function usePendingOpen() {
  const pending = useRef<PendingOpen | null>(null);
  useEffect(() => () => pending.current?.cancel(), []);
  return {
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
