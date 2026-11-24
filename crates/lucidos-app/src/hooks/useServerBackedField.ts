import { useState } from 'preact/hooks';

export function useServerBackedField<T>(
  serverValue: T,
  isEqual: (a: T, b: T) => boolean = Object.is,
): [T, (next: T) => void] {
  const [draft, setDraft] = useState<{ touched: false } | { touched: true; value: T }>({
    touched: false,
  });
  const set = (next: T): void => {
    setDraft(isEqual(next, serverValue) ? { touched: false } : { touched: true, value: next });
  };
  return [draft.touched ? draft.value : serverValue, set];
}

export function sameJson<T>(a: T, b: T): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

export function sameSet<T extends string>(a: readonly T[], b: readonly T[]): boolean {
  if (a.length !== b.length) return false;
  const inB = new Set(b);
  return a.every((item) => inB.has(item));
}
