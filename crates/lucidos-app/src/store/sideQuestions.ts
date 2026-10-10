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

/** A side question and where its answer stands.
 *
 *  Recorded as side-question thread events no agent ever reads (ADR 0320), so
 *  a card survives a reload and shows on every device. */
export type SideQuestion = {
  id: string;
  threadId: string;
  question: string;
  /** Blobs the user attached to the question. */
  imageHashes: readonly string[];
  /** The card's moment on the thread's clock: its `SideQuestionAsked` seq,
   *  or the newest seq the thread held while the ask is still on its way.
   *  Rows later than it draw below the card. Null when no event had loaded. */
  afterSeq: number | null;
  /** Dismissed cards collapse to a row that reopens. */
  dismissed: boolean;
  /** How many times it was asked. A retry re-asks under the same id, and a
   *  retry this device sent counts one more than the events show so far. */
  asks: number;
} & (
  | { status: 'pending' }
  | { status: 'answered'; answer: string }
  /** `dropped`: the request failed before the engine replied, so the ask
   *  may still be running there. */
  | { status: 'failed'; error: string; dropped?: true }
);

/** One shared empty list, so an imageless card keeps a stable prop. */
const NO_IMAGES: readonly string[] = [];

/** Asks not yet recorded, keyed by id: a pending card before its event lands,
 *  and an ask whose request failed before the engine recorded anything. */
export const localSideQuestions = signal<ReadonlyMap<string, SideQuestion>>(new Map());

/** Ids this device dismissed, drawn dismissed before the event lands. */
export const dismissingSideQuestions = signal<ReadonlySet<string>>(new Set());

/** Dismissed cards this device opened again. Local only: reopening records
 *  nothing, so another device keeps its own view. */
export const reopenedSideQuestions = signal<ReadonlySet<string>>(new Set());

/** Whether the composer for `threadId` is in side-question mode, where Send
 *  asks the box as a side question. It lives in the draft's compose selection,
 *  so a reload and every device keep it. */
export function sideQuestionModeOn(threadId: string | null | undefined): boolean {
  return !!threadId && getComposeSelectionOverride(threadId).sideQuestionMode === true;
}

/** What the composer does with a submitted message. */
export type SideQuestionRoute =
  /** Not a side question: send it as usual. */
  | { kind: 'message' }
  /** A side question that cannot be asked here. The draft stays. */
  | { kind: 'refuse'; toast: string }
  | { kind: 'ask'; question: string };

export const SIDE_QUESTION_NOT_STARTED =
  'Side questions work once this thread has started. Send a normal message first.';
export const SIDE_QUESTION_EMPTY = 'Type the side question first.';
export const SIDE_QUESTION_CODEX =
  'Side questions are not available in Codex threads. Send it as a normal message instead.';

/** Route a submit. `asked` is true when the user chose a side question: the
 *  composer is in side-question mode, or was when an upload queued it. A side
 *  question never becomes a turn: it is asked, or refused with the draft kept.
 *  A Codex thread takes none, and refusing here keeps the draft and its images. */
export function routeSideQuestion(
  message: string,
  thread: { started: boolean; codex: boolean },
  asked: boolean,
): SideQuestionRoute {
  if (!asked) return { kind: 'message' };
  if (!thread.started) return { kind: 'refuse', toast: SIDE_QUESTION_NOT_STARTED };
  if (thread.codex) return { kind: 'refuse', toast: SIDE_QUESTION_CODEX };
  const question = message.trim();
  if (question === '') return { kind: 'refuse', toast: SIDE_QUESTION_EMPTY };
  return { kind: 'ask', question };
}

/** The highest persisted seq the thread holds, or null before any has
 *  loaded. An unsent message is not in `events`, so it always sorts after. */
export function latestEventSeq(thread: ThreadState | undefined): number | null {
  let latest: number | null = null;
  for (const seq of thread?.events.keys() ?? []) latest = Math.max(latest ?? seq, seq);
  return latest;
}

/** The thread's recorded side questions, in asking order. Rebuilt only when
 *  its append-only event map grows, so each card keeps its identity and a
 *  memoised turn re-renders only when its own card changes. */
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
      // A retry: the card keeps its place and waits again.
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

/** The dismissed flag a card is drawn with, from this device's own layer. */
const drawnCache = new WeakMap<SideQuestion, SideQuestion>();

function drawn(card: SideQuestion, dismissed: boolean): SideQuestion {
  if (card.dismissed === dismissed) return card;
  const hit = drawnCache.get(card);
  if (hit) return hit;
  const flipped = { ...card, dismissed };
  drawnCache.set(card, flipped);
  return flipped;
}

/** What this device knows ahead of the recorded events: a retry whose ask
 *  is not recorded yet, or the engine's reply to an ask still recorded as
 *  pending. A dropped request never settles a recorded ask, since the engine
 *  may still be answering it, and nothing outranks a recorded answer. Drawn at
 *  the recorded moment. */
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

/** Every side question on a thread, as this device draws it: the recorded
 *  cards, then asks still on their way. */
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

/** Only a failure that came back quicker than this is retried: a dropped
 *  connection or a hiccup, never an ask that already spent its time. */
export const QUICK_FAILURE_MS = 20_000;

/** A refusal (4xx) fails the same way every time, so only a dropped request
 *  or an engine error is worth asking again. */
function retryable(err: unknown): boolean {
  return !(err instanceof ApiError) || err.httpCode >= 500;
}

/** The next ask under a card's id: one past those recorded. A failure the
 *  engine never recorded is still its first. */
function nextAsks(threadId: string, id: string): number {
  const recorded = recordedSideQuestions(threadMap.value.get(threadId)).find((c) => c.id === id);
  return (recorded?.asks ?? 0) + 1;
}

/** Post one ask and draw its card pending at once, retrying a quick failure
 *  on the schedule every send shares before the card shows it. Never throws.
 *  A 409 means the engine already holds this ask, so its recorded events
 *  settle the card. */
async function postAsk(first: Ask): Promise<void> {
  let ask = first;
  setLocal({ ...ask, status: 'pending' });
  const result = await withQuietRetries(
    () => postSideQuestion(ask.threadId, ask.id, ask.question, ask.imageHashes),
    {
      path: 'side-question',
      retryable: (err, attemptMs) => retryable(err) && attemptMs < QUICK_FAILURE_MS,
      beforeRetry: (err) => {
        // An engine reply counts this attempt before its events arrive. A
        // dropped request counts only what is recorded: it may never have arrived.
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
    // Count no ask past those recorded, so the engine's events rule.
    setLocal({ ...ask, asks: nextAsks(ask.threadId, ask.id) - 1, status: 'pending' });
    return;
  }
  setLocal(err instanceof ApiError
    ? { ...ask, status: 'failed', error: err.reason }
    : { ...ask, status: 'failed', error: errorDetail(err), dropped: true });
}

/** Ask a side question with any uploaded images and show its card at once.
 *  The recorded events take over when they arrive. */
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

/** Ask a failed side question again under its own id, so the card keeps its
 *  place on every device. */
export function retrySideQuestion(card: SideQuestion): Promise<void> {
  const { id, threadId, question, imageHashes, afterSeq, dismissed } = card;
  return postAsk({ id, threadId, question, imageHashes, afterSeq, dismissed, asks: nextAsks(threadId, id) });
}

/** Dismiss a card and record the dismissal. A 404 means the engine never
 *  recorded the ask (it refused it), so the card just leaves this device. */
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

/** Open a dismissed card again, on this device. */
export function reopenSideQuestion(id: string): void {
  reopenedSideQuestions.value = withId(reopenedSideQuestions.value, id, true);
}
