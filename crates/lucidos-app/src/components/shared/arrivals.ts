import { useLayoutEffect, useRef } from 'preact/hooks';

export interface Entry<T> { key: string; item: T }

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

export function useArrivals(keys: readonly string[] | null): ReadonlySet<string> {
  const previous = useRef<ReadonlySet<string> | null>(null);
  const before = previous.current;
  const arrived = keys && before ? new Set(keys.filter(k => !before.has(k))) : new Set<string>();
  useLayoutEffect(() => {
    if (keys) previous.current = new Set(keys);
  });
  return arrived;
}
