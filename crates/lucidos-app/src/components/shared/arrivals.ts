/**
 * Which rows of a list just arrived: the input behind `<ArrivalList>`
 * (`docs/glossary.md` § Arrival marker).
 *
 * A view shows its current state as it loads, and only what changes after
 * that is an arrival. So the first loaded render, a reopened view and a
 * reload arrive nothing.
 *
 * Call `useArrivals` from the component that stays mounted while the list can
 * change, not from inside a collapsible section. A section with no rows draws
 * no list, and its first row must still count as an arrival.
 */
import { useLayoutEffect, useRef } from 'preact/hooks';

/** A row in draw order. */
export interface Entry<T> { key: string; item: T }

/** The live rows in their live order. Each departing row of `previous` stays
 *  just after the row it followed, so it rolls out where it stood. */
export function mergeOrder<T>(previous: readonly Entry<T>[], live: readonly Entry<T>[]): Entry<T>[] {
  const merged = [...live];
  const liveKeys = new Set(live.map(e => e.key));
  let after = -1;
  for (const entry of previous) {
    if (liveKeys.has(entry.key)) {
      after = merged.findIndex(e => e.key === entry.key);
    } else {
      merged.splice(++after, 0, entry);
    }
  }
  return merged;
}

/** The keys new since the last loaded render. `keys` is null while the list
 *  is not loaded, which changes nothing: a reload is not an arrival. */
export function useArrivals(keys: readonly string[] | null): ReadonlySet<string> {
  const previous = useRef<ReadonlySet<string> | null>(null);
  const before = previous.current;
  const arrived = keys && before ? new Set(keys.filter(k => !before.has(k))) : new Set<string>();
  useLayoutEffect(() => {
    if (keys) previous.current = new Set(keys);
  });
  return arrived;
}
