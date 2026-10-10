/**
 * One clock for every elapsed-time label on screen.
 *
 * A label like "for 8 minutes" is painted once and then goes stale,
 * because nothing re-renders it. Through the outage this feature exists
 * to catch, an eight-hour silence would have read "for 2 minutes" all
 * night.
 *
 * Shared rather than one interval per component, so two surfaces
 * describing the same span cannot drift apart by a tick. One interval
 * serves every subscriber and stops when the last one unmounts.
 *
 * Coarse on purpose: every label it feeds is minute-granular, so a
 * faster tick would repaint for nothing.
 *
 * Not a poll and no substitute for a subscription (ADR 0118). It fetches
 * nothing and reads no server state.
 */
import { signal } from '@preact/signals';
import { useLayoutEffect } from 'preact/hooks';

/** How often the shared value advances. */
export const COARSE_TICK_MS = 30_000;

const coarseNow = signal(Date.now());
let subscribers = 0;
let ticker: ReturnType<typeof setInterval> | undefined;

function advance(): void {
  coarseNow.value = Date.now();
}

/** The current time in milliseconds, re-read every 30 seconds.
 *
 *  Reading the returned value during render IS the subscription, so a component
 *  that calls this repaints on each tick and nothing else does. */
export function useCoarseClock(): number {
  useLayoutEffect(() => {
    subscribers += 1;
    if (ticker === undefined) {
      // The value froze when the last subscriber left. Catch it up before the
      // first tick of this run, so no label paints an hour behind.
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
