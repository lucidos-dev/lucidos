import { signal } from '@preact/signals';
import { useLayoutEffect } from 'preact/hooks';

export const COARSE_TICK_MS = 30_000;

const coarseNow = signal(Date.now());
let subscribers = 0;
let ticker: ReturnType<typeof setInterval> | undefined;

function advance(): void {
  coarseNow.value = Date.now();
}

export function useCoarseClock(): number {
  useLayoutEffect(() => {
    subscribers += 1;
    if (ticker === undefined) {
      advance();
      ticker = setInterval(advance, COARSE_TICK_MS);
    }
    return () => {
      subscribers -= 1;
      if (subscribers === 0 && ticker !== undefined) {
        clearInterval(ticker);
        ticker = undefined;
      }
    };
  }, []);
  return coarseNow.value;
}
