import { signal } from '@preact/signals';
import {
  askSideQuestion as postSideQuestion,
  dismissSideQuestion as postDismissal,
  ApiError,
} from '../api/client';
import { errorDetail } from '../utils/errorDetail';
import { withQuietRetries } from './actions/sendRetry';
import { getComposeSelectionOverride } from './composeSelections';
import { showToast, threadMap } from './store';
import { isSideQuestionEvent } from './thread-events/thread-event-types';
import type { ThreadState } from './thread-events/thread-meta';

export type SideQuestion = {
  id: string;
  threadId: string;
  question: string;
  imageHashes: readonly string[];
  afterSeq: number | null;
  dismissed: boolean;
  asks: number;
} & (
  | { status: 'pending' }
  | { status: 'answered'; answer: string }
  | { status: 'failed'; error: string; dropped?: true }
);

const NO_IMAGES: readonly string[] = [];

export const localSideQuestions = signal<ReadonlyMap<string, SideQuestion>>(new Map());

export const dismissingSideQuestions = signal<ReadonlySet<string>>(new Set());

export const reopenedSideQuestions = signal<ReadonlySet<string>>(new Set());

export function sideQuestionModeOn(threadId: string | null | undefined): boolean {
  return !!threadId && getComposeSelectionOverride(threadId).sideQuestionMode === true;
}

export type SideQuestionRoute =
  | { kind: 'message' }
  | { kind: 'refuse'; toast: string }
  | { kind: 'ask'; question: string };

export const SIDE_QUESTION_NOT_STARTED =
  'Side questions work once this thread has started. Send a normal message first.';
export const SIDE_QUESTION_EMPTY = 'Type the side question first.';

export function routeSideQuestion(
  message: string,
  thread: { started: boolean },
  asked: boolean,
): SideQuestionRoute {
  if (!asked) return { kind: 'message' };
  if (!thread.started) return { kind: 'refuse', toast: SIDE_QUESTION_NOT_STARTED };
  const question = message.trim();
  if (question === '') return { kind: 'refuse', toast: SIDE_QUESTION_EMPTY };
  return { kind: 'ask', question };
}

export function latestEventSeq(thread: ThreadState | undefined): number | null {
  let latest: number | null = null;
  for (const seq of thread?.events.keys() ?? []) latest = Math.max(latest ?? seq, seq);
  return latest;
}

const recordedCache = new WeakMap<ThreadState['events'], { size: number; cards: SideQuestion[] }>();

export function recordedSideQuestions(thread: ThreadState | undefined): SideQuestion[] {
  if (!thread) return [];
  const cached = recordedCache.get(thread.events);
  if (cached && cached.size === thread.events.size) return cached.cards;
  const previous = new Map((cached?.cards ?? []).map((card) => [card.id, card]));
  const byId = new Map<string, SideQuestion>();
  const inOrder = [...thread.events].filter(([, event]) => isSideQuestionEvent(event)).sort(([a], [b]) => a - b);
  for (const [seq, event] of inOrder) {
    if (!isSideQuestionEvent(event)) continue;
    const id = event.side_question_id;
    const card = byId.get(id);
    if (event.type === 'SideQuestionAsked' && card) {
      byId.set(id, { ...card, status: 'pending', asks: card.asks + 1 });
    } else if (event.type === 'SideQuestionAsked') {
      byId.set(id, {
        id,
        threadId: thread.meta.id,
        question: event.question,
        imageHashes: event.image_hashes ?? NO_IMAGES,
        afterSeq: seq,
        dismissed: false,
        asks: 1,
        status: 'pending',
      });
    } else if (!card) {
      continue;
    } else if (event.type === 'SideQuestionAnswered') {
      byId.set(id, { ...card, status: 'answered', answer: event.answer });
    } else if (event.type === 'SideQuestionFailed') {
      byId.set(id, { ...card, status: 'failed', error: event.error });
    } else {
      byId.set(id, { ...card, dismissed: true });
    }
  }
  const cards = [...byId.values()].map((card) => {
    const old = previous.get(card.id);
    return old && sameCard(old, card) ? old : card;
  });
  recordedCache.set(thread.events, { size: thread.events.size, cards });
  return cards;
}

function sameCard(a: SideQuestion, b: SideQuestion): boolean {
  return a.status === b.status && a.dismissed === b.dismissed && a.afterSeq === b.afterSeq && a.asks === b.asks
    && (a.status !== 'answered' || (b.status === 'answered' && a.answer === b.answer))
    && (a.status !== 'failed' || (b.status === 'failed' && a.error === b.error));
}

const drawnCache = new WeakMap<SideQuestion, SideQuestion>();

function drawn(card: SideQuestion, dismissed: boolean): SideQuestion {
  if (card.dismissed === dismissed) return card;
  const hit = drawnCache.get(card);
  if (hit) return hit;
  const flipped = { ...card, dismissed };
  drawnCache.set(card, flipped);
  return flipped;
}

const aheadCache = new WeakMap<SideQuestion, SideQuestion>();

function withLocal(recorded: SideQuestion, local: SideQuestion | undefined): SideQuestion {
  const replied = local !== undefined && local.status !== 'pending' && !(local.status === 'failed' && local.dropped);
  const ahead = local !== undefined && recorded.status !== 'answered'
    && (local.asks > recorded.asks || (replied && recorded.status === 'pending'));
  if (!ahead) return recorded;
  const hit = aheadCache.get(local);
  if (hit && hit.afterSeq === recorded.afterSeq && hit.dismissed === recorded.dismissed) return hit;
  const merged = { ...local, afterSeq: recorded.afterSeq, dismissed: recorded.dismissed };
  aheadCache.set(local, merged);
  return merged;
}

export function sideQuestionsFor(threadId: string): SideQuestion[] {
  const localCards = localSideQuestions.value;
  const recorded = recordedSideQuestions(threadMap.value.get(threadId))
    .map((card) => withLocal(card, localCards.get(card.id)));
  const recordedIds = new Set(recorded.map((card) => card.id));
  const local = [...localCards.values()]
    .filter((card) => card.threadId === threadId && !recordedIds.has(card.id));
  const dismissing = dismissingSideQuestions.value;
  const reopened = reopenedSideQuestions.value;
  return [...recorded, ...local].map((card) => {
    const dismissed = (card.dismissed || dismissing.has(card.id)) && !reopened.has(card.id);
    return drawn(card, dismissed);
  });
}

function setLocal(card: SideQuestion): void {
  const next = new Map(localSideQuestions.value);
  next.set(card.id, card);
  localSideQuestions.value = next;
}

function withId(set: ReadonlySet<string>, id: string, present: boolean): ReadonlySet<string> {
  const next = new Set(set);
  if (present) next.add(id);
  else next.delete(id);
  return next;
}

type Ask = Omit<SideQuestion, 'status' | 'answer' | 'error'>;

export const QUICK_FAILURE_MS = 20_000;

function retryable(err: unknown): boolean {
  return !(err instanceof ApiError) || err.httpCode >= 500;
}

function nextAsks(threadId: string, id: string): number {
  const recorded = recordedSideQuestions(threadMap.value.get(threadId)).find((c) => c.id === id);
  return (recorded?.asks ?? 0) + 1;
}

async function postAsk(first: Ask): Promise<void> {
  let ask = first;
  setLocal({ ...ask, status: 'pending' });
  const result = await withQuietRetries(
    () => postSideQuestion(ask.threadId, ask.id, ask.question, ask.imageHashes),
    {
      path: 'side-question',
      retryable: (err, attemptMs) => retryable(err) && attemptMs < QUICK_FAILURE_MS,
      beforeRetry: (err) => {
        const recorded = nextAsks(ask.threadId, ask.id);
        ask = { ...ask, asks: err instanceof ApiError ? Math.max(ask.asks + 1, recorded) : recorded };
        setLocal({ ...ask, status: 'pending' });
      },
    },
  );
  if (result.kind === 'done') {
    setLocal({ ...ask, status: 'answered', answer: result.value });
    return;
  }
  if (result.kind === 'landed') return;
  const err = result.error;
  if (err instanceof ApiError && err.httpCode === 409) {
    setLocal({ ...ask, asks: nextAsks(ask.threadId, ask.id) - 1, status: 'pending' });
    return;
  }
  setLocal(err instanceof ApiError
    ? { ...ask, status: 'failed', error: err.reason }
    : { ...ask, status: 'failed', error: errorDetail(err), dropped: true });
}

export function askSideQuestion(
  threadId: string,
  question: string,
  imageHashes: readonly string[] = NO_IMAGES,
): Promise<void> {
  return postAsk({
    id: crypto.randomUUID(),
    threadId,
    question,
    imageHashes,
    afterSeq: latestEventSeq(threadMap.value.get(threadId)),
    dismissed: false,
    asks: 1,
  });
}

export function retrySideQuestion(card: SideQuestion): Promise<void> {
  const { id, threadId, question, imageHashes, afterSeq, dismissed } = card;
  return postAsk({ id, threadId, question, imageHashes, afterSeq, dismissed, asks: nextAsks(threadId, id) });
}

export async function dismissSideQuestion(card: SideQuestion): Promise<void> {
  if (reopenedSideQuestions.value.has(card.id)) {
    reopenedSideQuestions.value = withId(reopenedSideQuestions.value, card.id, false);
    return;
  }
  dismissingSideQuestions.value = withId(dismissingSideQuestions.value, card.id, true);
  try {
    await postDismissal(card.threadId, card.id);
  } catch (err) {
    dismissingSideQuestions.value = withId(dismissingSideQuestions.value, card.id, false);
    if (err instanceof ApiError && err.httpCode === 404 && card.status !== 'pending') {
      const next = new Map(localSideQuestions.value);
      next.delete(card.id);
      localSideQuestions.value = next;
      return;
    }
    reopenedSideQuestions.value = withId(reopenedSideQuestions.value, card.id, false);
    const reason = err instanceof ApiError ? err.reason : errorDetail(err);
    showToast(`Could not dismiss the side question "${card.question}": ${reason}`, 'error');
  }
}

export function reopenSideQuestion(id: string): void {
  reopenedSideQuestions.value = withId(reopenedSideQuestions.value, id, true);
}
