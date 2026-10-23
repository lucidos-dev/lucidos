import { useState } from 'preact/hooks';

/** A form field whose value is server state until the user edits it.
 *
 *  An **untouched** field holds no copy at all. It returns `serverValue`
 *  itself, so every SSE frame that moves the underlying entity repaints it.
 *  A **touched** field holds the user's draft and ignores incoming frames,
 *  because unsaved work is not the server's to overwrite. A setter call that
 *  lands back on the served value returns the field to untouched, so undoing
 *  an edit re-arms the subscription.
 *
 *  This replaces seeding a `useState` from an entity, which reads correctly
 *  and is wrong. That initializer runs once per mount. A component keyed on
 *  an entity id keeps its first snapshot forever, and no store update can
 *  reach it. See ADR 0118.
 *
 *  `isEqual` defaults to `Object.is`. Pass a comparator for a field whose
 *  server value is rebuilt each render (an array, an object), or the field
 *  could never return to untouched.
 */
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

/** Structural equality for a field whose value is JSON-shaped: a string list,
 *  a record of drafts, a list of subscription rows. Order-sensitive, which is
 *  what a list the user reorders wants. */
export function sameJson<T>(a: T, b: T): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/** Order-insensitive equality for a field the user toggles as a SET. Toggling
 *  a category off and back on appends it at the end, so an order-sensitive
 *  compare would call that edited when it is not. */
export function sameSet<T extends string>(a: readonly T[], b: readonly T[]): boolean {
  if (a.length !== b.length) return false;
  const inB = new Set(b);
  return a.every((item) => inB.has(item));
}
