import { signal, type ReadonlySignal } from '@preact/signals';
import { useDelayedFlag } from '../hooks/useDelayedLoading';
import type { AnswerKind } from './thread-events';
import type { SendFailure } from './actions/sendRetry';

/** The optimistic picks of one kind of card, keyed by the card's own id.
 *
 *  A pick lives here from the tap until the engine's persisted resolution
 *  arrives over SSE. The card highlights the pick at once, and its header reads
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

/** What the picked choice draws in its mark slot: its radio dot or check. */
export type PickMark = 'none' | 'spinner' | 'mark';

/** The picked choice's mark only moves forward: nothing while the pick is
 *  unconfirmed, a spinner once that wait passes the delay gate, then the mark.
 *  Never pass back over a mark the reader has seen, or a confirm landing just
 *  past the gate reads as a blink. */
export function usePickMark(confirmed: boolean, pending: boolean): PickMark {
  const sending = useDelayedFlag(!confirmed && pending);
  return confirmed ? 'mark' : sending ? 'spinner' : 'none';
}

/** The question cards' optimistic picks, keyed by tool-use id. In the store
 *  because a send reads it too: a typed message is no answer to a card whose
 *  pick is already on its way (`questionTheSendAnswers`). */
export const pendingAnswers = pendingDecisions<AnswerKind>();

/** A question card's pick whose quiet retries all went unanswered. The card
 *  draws it as Not sent on the picked option, and Retry sends it again. */
export interface UnsentPick {
  threadId: string;
  answer: AnswerKind;
  failure: SendFailure;
}

/** Unsent picks, keyed by tool-use id. In memory only, so a reload leaves the
 *  card live with nothing marked. */
export const unsentPicks = pendingDecisions<UnsentPick>();
