import { signal } from '@preact/signals';
import {
  askSideQuestion as postSideQuestion,
  dismissSideQuestion as postDismissal,
  ApiError,
} from '../api/client';
import { errorDetail } from '../utils/errorDetail';
import { showToast, threadMap } from './store';
import { isSideQuestionEvent } from './thread-events/thread-event-types';
import type { ThreadState } from './thread-events/thread-meta';

/** A `/btw` side question and where its answer stands.
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
} & (
  | { status: 'pending' }
  | { status: 'answered'; answer: string }
  | { status: 'failed'; error: string }
);

/** The menu entry and composer prefix for a side question. */
export const SIDE_QUESTION_COMMAND = 'btw';

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

/** The question in `message` when it is a side question (`/btw` as its first
 *  word), else `null`. An empty string is a bare `/btw`. Mirrors the engine's
 *  `is_side_question`, which refuses the same text on the normal chat route. */
export function sideQuestionText(message: string): string | null {
  const match = /^\s*\/btw(?:\s+([\s\S]*))?$/.exec(message);
  return match ? (match[1] ?? '').trim() : null;
}

/** The filter text that turns into a side question as the user types it in
 *  the command menu: `btw` and a space. */
export function isSideQuestionFilter(filter: string): boolean {
  return /^btw\s/.test(filter);
}

/** The composer text once the command menu hands back `handoff` (`/btw …`).
 *  A draft is never lost: it becomes the question. */
export function withSideQuestionPrefix(handoff: string, draft: string): string {
  if (draft.trim() === '') return handoff;
  if (sideQuestionText(draft) !== null) return draft;
  return `${handoff.trimEnd()} ${draft.trimStart()}`;
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
export const SIDE_QUESTION_EMPTY = 'Type a question after /btw.';
export const SIDE_QUESTION_CODEX =
  'Side questions are not available in Codex threads. Send it as a normal message instead.';

/** Route a submit. A side question never becomes a turn: it is asked, or
 *  refused with the draft kept. `asked` is true when the user chose to ask
 *  (the Send button's long press), so no `/btw` is needed. A Codex thread
 *  takes none, and refusing here keeps the draft and its images. */
export function routeSideQuestion(
  message: string,
  thread: { started: boolean; codex: boolean },
  asked = false,
): SideQuestionRoute {
  const question = sideQuestionText(message) ?? (asked ? message.trim() : null);
  if (question === null) return { kind: 'message' };
  if (!thread.started) return { kind: 'refuse', toast: SIDE_QUESTION_NOT_STARTED };
  if (thread.codex) return { kind: 'refuse', toast: SIDE_QUESTION_CODEX };
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
    if (event.type === 'SideQuestionAsked') {
      byId.set(id, {
        id,
        threadId: thread.meta.id,
        question: event.question,
        imageHashes: event.image_hashes ?? NO_IMAGES,
        afterSeq: seq,
        dismissed: false,
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
  return a.status === b.status && a.dismissed === b.dismissed && a.afterSeq === b.afterSeq
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

/** A recorded card still pending while this device already holds the
 *  answer, drawn with that answer at the recorded moment. */
const settledCache = new WeakMap<SideQuestion, SideQuestion>();

function settledEarly(recorded: SideQuestion, local: SideQuestion | undefined): SideQuestion {
  if (recorded.status !== 'pending' || !local || local.status === 'pending') return recorded;
  const hit = settledCache.get(local);
  if (hit && hit.afterSeq === recorded.afterSeq && hit.dismissed === recorded.dismissed) return hit;
  const merged = { ...local, afterSeq: recorded.afterSeq, dismissed: recorded.dismissed };
  settledCache.set(local, merged);
  return merged;
}

/** Every side question on a thread, as this device draws it: the recorded
 *  cards, then asks still on their way. */
export function sideQuestionsFor(threadId: string): SideQuestion[] {
  const localCards = localSideQuestions.value;
  const recorded = recordedSideQuestions(threadMap.value.get(threadId))
    .map((card) => settledEarly(card, localCards.get(card.id)));
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

/** Ask a side question with any uploaded images and show its card at once.
 *  Never throws: a failure lands on the card. The recorded events take over
 *  when they arrive. */
export async function askSideQuestion(
  threadId: string,
  question: string,
  imageHashes: readonly string[] = NO_IMAGES,
): Promise<void> {
  const id = crypto.randomUUID();
  const asked = {
    id,
    threadId,
    question,
    imageHashes,
    afterSeq: latestEventSeq(threadMap.value.get(threadId)),
    dismissed: false,
  };
  setLocal({ ...asked, status: 'pending' });
  try {
    const answer = await postSideQuestion(threadId, id, question, imageHashes);
    setLocal({ ...asked, status: 'answered', answer });
  } catch (err) {
    const error = err instanceof ApiError ? err.reason : errorDetail(err);
    setLocal({ ...asked, status: 'failed', error });
  }
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
