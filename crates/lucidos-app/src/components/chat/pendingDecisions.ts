import { signal, type ReadonlySignal } from '@preact/signals';

/** The optimistic picks of one kind of card, keyed by the card's own id.
 *
 *  A pick lives here from the tap until the engine's persisted resolution
 *  arrives over SSE. The card renders the pick at once, and its header reads
 *  "Sending" for exactly that window. Module-level, because the header is
 *  built outside the card component. The card drains its entry when the
 *  resolution lands, and clears it when the send fails. */
export interface PendingDecisions<T> {
  map: ReadonlySignal<ReadonlyMap<string, T>>;
  set: (id: string, value: T) => void;
  clear: (id: string) => void;
}

export function pendingDecisions<T>(): PendingDecisions<T> {
  const map = signal<ReadonlyMap<string, T>>(new Map());
  return {
    map,
    set(id, value) {
      const next = new Map(map.value);
      next.set(id, value);
      map.value = next;
    },
    clear(id) {
      if (!map.value.has(id)) return;
      const next = new Map(map.value);
      next.delete(id);
      map.value = next;
    },
  };
}
