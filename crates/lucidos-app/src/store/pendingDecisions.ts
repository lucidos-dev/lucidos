import { signal, type ReadonlySignal } from '@preact/signals';
import { effectiveThreadStatus } from './store';
import { awaitedQuestionCard, computeExchanges, type AnswerKind, type ThreadState } from './thread-events';
import type { SendFailure } from './actions/sendRetry';
import type { AnswerPlacement } from '../api/client/chat';
import { forgetUnsentMessageRecord } from './unsentMessageRecords';
import type { UnsentMessage } from './unsentMessages';

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

export const pendingAnswers = pendingDecisions<AnswerKind>();

export function questionTheSendAnswers(thread: ThreadState): string | undefined {
  if (effectiveThreadStatus(thread) !== 'waiting_for_user_answer') return undefined;
  const card = awaitedQuestionCard(computeExchanges(thread));
  return card && !pendingAnswers.map.peek().has(card) ? card : undefined;
}

export interface UnsentPick {
  threadId: string;
  answer: AnswerKind;
  failure?: SendFailure;
  placement?: AnswerPlacement;
  refusal?: UnsentMessage['refusal'];
}

const picks = pendingDecisions<UnsentPick>();
export const unsentPicks: PendingDecisions<UnsentPick> = {
  ...picks,
  clear(id) {
    const eventId = picks.map.peek().get(id)?.placement?.event_id;
    if (eventId) forgetUnsentMessageRecord(eventId);
    picks.clear(id);
  },
};
